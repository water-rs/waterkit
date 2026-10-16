//! The native document realization: Apple Vision's
//! `RecognizeDocumentsRequest` bridged through `swift-bridge` like the
//! crate's other Apple bridges.
//!
//! The shared pass handler and bridge come from
//! [`crate::sys::apple_vision`]; this module owns what the document request
//! asks of them: the supported language set, the offer, and the decode of
//! `DocumentObservation`'s container tree into [`Block`]s. The bridge emits
//! each container's blocks in the reading order of its transcript; nested
//! containers' blocks decode in that order here.

use std::{ops::Range, sync::OnceLock};

use icu_locale_core::LanguageIdentifier;

use crate::{
    Block, DataKind, DetectedData, Document, List, ListItem, Paragraph, Quad, Table, TableCell,
    VisionError,
    document::{DocumentPlan, RecognizeDocument},
    sealed::{Offer, Pass},
    sys::apple_vision::{WireBarcode, WireTextLine, ffi, ffi_outcome, wire_quad},
};

/// Languages `RecognizeDocumentsRequest` serves, fetched once.
fn supported_languages() -> &'static [LanguageIdentifier] {
    static LANGUAGES: OnceLock<Vec<LanguageIdentifier>> = OnceLock::new();
    LANGUAGES.get_or_init(|| {
        let tags: Vec<String> = serde_json::from_str(&ffi::vision_supported_document_languages())
            .expect("the bridge reports a JSON string array");
        tags.iter()
            .filter_map(|tag| match tag.parse::<LanguageIdentifier>() {
                Ok(language) => Some(language),
                Err(error) => {
                    tracing::warn!(
                        tag,
                        %error,
                        "Apple Vision returned an unparsable language tag"
                    );
                    None
                }
            })
            .collect()
    })
}

/// `supportedRecognitionLanguages` `RecognizeDocumentsRequest` serves,
/// listed exactly: the native set [`crate::Vision::capabilities`]
/// advertises.
pub fn recognizer_languages() -> Vec<LanguageIdentifier> {
    supported_languages().to_vec()
}

/// Whether Vision serves `request` exactly: every language it names, and no
/// formulas — Apple Vision has no formula recognition.
pub fn offer(request: &RecognizeDocument) -> Offer {
    let supported = supported_languages();
    if supported.is_empty() {
        return Offer::Absent;
    }
    if request.formulas {
        return Offer::Lacks("formulas".to_owned());
    }
    let missing: Vec<String> = request
        .languages
        .iter()
        .filter(|language| !supported.contains(language))
        .map(ToString::to_string)
        .collect();
    if missing.is_empty() {
        Offer::Serves
    } else {
        Offer::Lacks(format!("recognizer languages {}", missing.join(", ")))
    }
}

/// Verifies that Vision serves every language the selected plan names.
pub fn prepare(plan: &DocumentPlan) -> Result<(), VisionError> {
    let supported = supported_languages();
    let missing: Vec<String> = plan
        .languages
        .iter()
        .filter(|language| !supported.contains(language))
        .map(ToString::to_string)
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(VisionError::Unsupported(format!(
            "recognizer languages {}",
            missing.join(", ")
        )))
    }
}

/// Runs document recognition through Vision on the pass's shared handler.
pub async fn recognize(pass: &mut Pass<'_>, plan: &DocumentPlan) -> Result<Document, VisionError> {
    let handler = pass
        .prepared::<crate::sys::apple_vision::AppleImage>()
        .await?
        .handler;
    let tags: Vec<String> = plan.languages.iter().map(ToString::to_string).collect();
    let json = serde_json::to_string(&tags).expect("serializing strings cannot fail");
    let documents = ffi_outcome::<WireDocument>(|callback| {
        ffi::vision_recognize_document(handler, &json, callback);
    })
    .await?;
    let bounds = union_bounds(&documents);
    let blocks: Vec<Block> = documents
        .into_iter()
        .flat_map(|document| document.blocks)
        .map(WireBlock::into_block)
        .collect::<Result<_, _>>()?;
    Ok(Document { blocks, bounds })
}

/// The bounds spanning every reported document; the full image when the
/// page held none.
fn union_bounds(documents: &[WireDocument]) -> Quad {
    if documents.is_empty() {
        return Quad([
            crate::Point { x: 0.0, y: 0.0 },
            crate::Point { x: 1.0, y: 0.0 },
            crate::Point { x: 1.0, y: 1.0 },
            crate::Point { x: 0.0, y: 1.0 },
        ]);
    }
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for document in documents {
        for point in document.corners.as_chunks::<2>().0 {
            min_x = min_x.min(point[0]);
            min_y = min_y.min(point[1]);
            max_x = max_x.max(point[0]);
            max_y = max_y.max(point[1]);
        }
    }
    Quad([
        crate::Point { x: min_x, y: min_y },
        crate::Point { x: max_x, y: min_y },
        crate::Point { x: max_x, y: max_y },
        crate::Point { x: min_x, y: max_y },
    ])
}

/// A recognized document as the bridge reports it.
#[derive(Debug, serde::Deserialize)]
struct WireDocument {
    /// The four upright corners of the document, x/y interleaved.
    corners: [f32; 8],
    /// The document container's children.
    blocks: Vec<WireBlock>,
}

/// A document container's child as the bridge reports it; the `kind` tag
/// names each node so a kind this vocabulary does not map fails decoding —
/// and the request — rather than dropping the node.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireBlock {
    /// A `DocumentObservation.Container.Text` paragraph.
    Paragraph(WireParagraph),
    /// A `DocumentObservation.Container.Table`.
    Table(WireTable),
    /// A `DocumentObservation.Container.List`.
    List(WireList),
    /// A `BarcodeObservation` in the container.
    Barcode(WireBarcode),
}

impl WireBlock {
    fn into_block(self) -> Result<Block, VisionError> {
        Ok(match self {
            Self::Paragraph(paragraph) => Block::Paragraph(paragraph.into_paragraph()?),
            Self::Table(table) => Block::Table(table.into_table()?),
            Self::List(list) => Block::List(list.into_list()?),
            Self::Barcode(barcode) => Block::Barcode(barcode.into_barcode()),
        })
    }
}

/// A container's children mapped into blocks; the bridge already reports
/// them in the transcript's reading order.
fn into_blocks(blocks: Vec<WireBlock>) -> Result<Vec<Block>, VisionError> {
    blocks
        .into_iter()
        .map(WireBlock::into_block)
        .collect::<Result<Vec<_>, _>>()
}

/// A `Container.Text` paragraph as the bridge reports it.
#[derive(Debug, serde::Deserialize)]
struct WireParagraph {
    /// The paragraph's transcript.
    text: String,
    /// The paragraph's lines.
    lines: Vec<WireTextLine>,
    /// The paragraph's detected data.
    data: Vec<WireDetectedData>,
    /// The four upright corners in reading order, x/y interleaved.
    corners: [f32; 8],
}

impl WireParagraph {
    fn into_paragraph(self) -> Result<Paragraph, VisionError> {
        Ok(Paragraph {
            text: self.text,
            lines: self
                .lines
                .into_iter()
                .map(WireTextLine::into_line)
                .collect(),
            data: self
                .data
                .into_iter()
                .map(WireDetectedData::into_data)
                .collect::<Result<_, _>>()?,
            bounds: wire_quad(self.corners),
        })
    }
}

/// A `DataDetectorMatch` as the bridge reports it.
#[derive(Debug, serde::Deserialize)]
struct WireDetectedData {
    /// The canonical data-kind name.
    kind: String,
    /// The detected value.
    value: String,
    /// The `[start, end)` byte range within the paragraph's text.
    range: [usize; 2],
    /// The four upright corners in reading order, x/y interleaved.
    corners: [f32; 8],
}

impl WireDetectedData {
    fn into_data(self) -> Result<DetectedData, VisionError> {
        let kind = match self.kind.as_str() {
            "url" => DataKind::Url,
            "email-address" => DataKind::EmailAddress,
            "phone-number" => DataKind::PhoneNumber,
            "postal-address" => DataKind::PostalAddress,
            other => {
                return Err(VisionError::Platform(format!(
                    "the bridge reported a data kind this vocabulary does not map: {other}"
                )));
            }
        };
        Ok(DetectedData {
            kind,
            value: self.value,
            range: Range {
                start: self.range[0],
                end: self.range[1],
            },
            bounds: wire_quad(self.corners),
        })
    }
}

/// A `Container.Table` as the bridge reports it.
#[derive(Debug, serde::Deserialize)]
struct WireTable {
    /// The number of rows.
    rows: u32,
    /// The number of columns.
    columns: u32,
    /// The cells in row-major order, each once regardless of its span.
    cells: Vec<WireTableCell>,
    /// The four upright corners in reading order, x/y interleaved.
    corners: [f32; 8],
}

impl WireTable {
    fn into_table(self) -> Result<Table, VisionError> {
        Ok(Table {
            rows: self.rows,
            columns: self.columns,
            cells: self
                .cells
                .into_iter()
                .map(WireTableCell::into_cell)
                .collect::<Result<_, _>>()?,
            bounds: wire_quad(self.corners),
        })
    }
}

/// A `Container.Table.Cell` as the bridge reports it.
#[derive(Debug, serde::Deserialize)]
struct WireTableCell {
    /// The `[start, end)` row range the cell spans.
    rows: [u32; 2],
    /// The `[start, end)` column range the cell spans.
    columns: [u32; 2],
    /// The cell's container children.
    content: Vec<WireBlock>,
    /// The four upright corners in reading order, x/y interleaved.
    corners: [f32; 8],
}

impl WireTableCell {
    fn into_cell(self) -> Result<TableCell, VisionError> {
        Ok(TableCell {
            rows: Range {
                start: self.rows[0],
                end: self.rows[1],
            },
            columns: Range {
                start: self.columns[0],
                end: self.columns[1],
            },
            content: into_blocks(self.content)?,
            bounds: wire_quad(self.corners),
        })
    }
}

/// A `Container.List` as the bridge reports it.
#[derive(Debug, serde::Deserialize)]
struct WireList {
    /// The list's items.
    items: Vec<WireListItem>,
    /// The four upright corners in reading order, x/y interleaved.
    corners: [f32; 8],
}

impl WireList {
    fn into_list(self) -> Result<List, VisionError> {
        Ok(List {
            items: self
                .items
                .into_iter()
                .map(WireListItem::into_item)
                .collect::<Result<_, _>>()?,
            bounds: wire_quad(self.corners),
        })
    }
}

/// A `Container.List.Item` as the bridge reports it.
#[derive(Debug, serde::Deserialize)]
struct WireListItem {
    /// The item's marker text, when it carries one.
    marker: Option<String>,
    /// The item's container children.
    content: Vec<WireBlock>,
    /// The four upright corners in reading order, x/y interleaved.
    corners: [f32; 8],
}

impl WireListItem {
    fn into_item(self) -> Result<ListItem, VisionError> {
        Ok(ListItem {
            marker: self.marker,
            content: into_blocks(self.content)?,
            bounds: wire_quad(self.corners),
        })
    }
}

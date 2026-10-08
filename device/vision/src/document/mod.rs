//! The `document` capability: recognizing a document's structure in an
//! image.
//!
//! [`RecognizeDocument`] is served natively by Apple Vision's
//! `RecognizeDocumentsRequest` on iOS and macOS. Where [`RecognizeText`]
//! returns lines, document recognition returns the page's structure:
//! paragraphs with their detected data, tables with their cells, lists with
//! their items, and embedded barcodes, all in reading order.
//!
//! Apple Vision does not recognize formulas; a request with
//! [`formulas`](RecognizeDocument::formulas) is served by the portable
//! realization when the application carries one, and fails with
//! [`VisionError::Unsupported`] otherwise.
//!
//! [`RecognizeText`]: crate::RecognizeText

mod sys;

use std::ops::Range;

use icu_locale_core::LanguageIdentifier;

use crate::{
    Barcode, Quad, Request, TextLine, VisionError,
    sealed::{Context, Offer, Pass, Plan, Realization, Sealed},
};

/// The languages the native document realization serves on this device.
pub use sys::recognizer_languages as native_languages;

/// A document recognized in an image: its structured content and extent.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// The document's content in reading order.
    pub blocks: Vec<Block>,
    /// Normalized corners in reading order: top-left, top-right,
    /// bottom-right, bottom-left of the upright image.
    pub bounds: Quad,
}

/// A document node, in reading order.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// A paragraph of text.
    Paragraph(Paragraph),
    /// A table of cells in a row/column grid.
    Table(Table),
    /// A list of items.
    List(List),
    /// A barcode embedded in the page.
    Barcode(Barcode),
    /// A formula the serving realization typeset as LaTeX.
    Formula(Formula),
}

/// A paragraph of text within a document.
#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    /// The paragraph's text.
    pub text: String,
    /// The paragraph's lines in reading order.
    pub lines: Vec<TextLine>,
    /// Structured data detected in the paragraph's text.
    pub data: Vec<DetectedData>,
    /// Normalized corners in reading order: top-left, top-right,
    /// bottom-right, bottom-left of the upright image.
    pub bounds: Quad,
}

/// A table within a document.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// The number of rows the grid spans.
    pub rows: u32,
    /// The number of columns the grid spans.
    pub columns: u32,
    /// The table's cells; a cell spanning several rows or columns appears
    /// once with its full range.
    pub cells: Vec<TableCell>,
    /// Normalized corners in reading order: top-left, top-right,
    /// bottom-right, bottom-left of the upright image.
    pub bounds: Quad,
}

/// A single cell of a [`Table`].
#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    /// The rows the cell spans, as a range of row indices.
    pub rows: Range<u32>,
    /// The columns the cell spans, as a range of column indices.
    pub columns: Range<u32>,
    /// The cell's content in reading order.
    pub content: Vec<Block>,
    /// Normalized corners in reading order: top-left, top-right,
    /// bottom-right, bottom-left of the upright image.
    pub bounds: Quad,
}

/// A list within a document.
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    /// The list's items in reading order.
    pub items: Vec<ListItem>,
    /// Normalized corners in reading order: top-left, top-right,
    /// bottom-right, bottom-left of the upright image.
    pub bounds: Quad,
}

/// A single item of a [`List`].
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// The item's marker text — the bullet or number rendered before it —
    /// when the item carries one.
    pub marker: Option<String>,
    /// The item's content in reading order.
    pub content: Vec<Block>,
    /// Normalized corners in reading order: top-left, top-right,
    /// bottom-right, bottom-left of the upright image.
    pub bounds: Quad,
}

/// A formula recognized in a document, typeset as LaTeX.
#[derive(Debug, Clone, PartialEq)]
pub struct Formula {
    /// The formula in LaTeX.
    pub latex: String,
    /// Normalized corners in reading order: top-left, top-right,
    /// bottom-right, bottom-left of the upright image.
    pub bounds: Quad,
}

/// Structured data detected in a [`Paragraph`]'s text.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedData {
    /// The kind of data detected.
    pub kind: DataKind,
    /// The detected value, normalized by the serving realization: a URL's
    /// absolute form, a phone number's digits, an address's full text.
    pub value: String,
    /// The byte range of the detected text within [`Paragraph::text`].
    pub range: Range<usize>,
    /// Normalized corners in reading order: top-left, top-right,
    /// bottom-right, bottom-left of the upright image.
    pub bounds: Quad,
}

/// The kinds of structured data a document recognizer detects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataKind {
    /// A URL.
    Url,
    /// An email address.
    EmailAddress,
    /// A phone number.
    PhoneNumber,
    /// A postal address.
    PostalAddress,
}

/// A document structure recognition request over one image.
///
/// Without [`languages`](Self::languages) the request follows the user's
/// profile languages. With them, the serving realization must support every
/// requested language exactly.
#[derive(Debug, Clone, Default)]
pub struct RecognizeDocument {
    languages: Vec<LanguageIdentifier>,
    formulas: bool,
}

impl RecognizeDocument {
    /// Creates a request in the user's profile languages.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts recognition to `languages`.
    ///
    /// The serving realization must support every requested language;
    /// languages a realization lacks are served by the portable realization
    /// when the application carries it, and fail with
    /// [`VisionError::Unsupported`] otherwise.
    #[must_use]
    pub fn languages(mut self, languages: impl IntoIterator<Item = LanguageIdentifier>) -> Self {
        self.languages = languages.into_iter().collect();
        self
    }

    /// Requests LaTeX for formulas.
    ///
    /// Apple Vision does not recognize formulas; a request with
    /// `formulas(true)` is served by the portable realization when the
    /// application carries it, and fails with [`VisionError::Unsupported`]
    /// otherwise.
    #[must_use]
    pub const fn formulas(mut self, formulas: bool) -> Self {
        self.formulas = formulas;
        self
    }
}

impl Request for RecognizeDocument {
    type Output = Document;
}

impl Sealed for RecognizeDocument {
    type Plan = DocumentPlan;

    fn plan(&self, context: Context<'_>) -> Result<Self::Plan, VisionError> {
        Ok(DocumentPlan {
            languages: self.languages.clone(),
            realization: context.select("document", &sys::offer(self), &Offer::Absent)?,
        })
    }
}

/// A document request's selected realization.
///
/// Public only because the sealed [`crate::Request`] contract names it;
/// realization code constructs it.
#[doc(hidden)]
#[derive(Debug)]
#[cfg_attr(
    not(any(target_os = "ios", target_os = "macos")),
    expect(
        dead_code,
        reason = "only a native realization reads the languages, and this platform has none yet"
    )
)]
pub struct DocumentPlan {
    languages: Vec<LanguageIdentifier>,
    realization: Realization,
}

impl Plan<RecognizeDocument> for DocumentPlan {
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
        )
    )]
    async fn prepare(&self, _context: Context<'_>) -> Result<(), VisionError> {
        match self.realization {
            Realization::Native => sys::prepare(self),
            Realization::Portable => {
                unreachable!("no portable document realization exists yet")
            }
        }
    }

    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
        )
    )]
    async fn run(self, pass: &mut Pass<'_>) -> Result<Document, VisionError> {
        match self.realization {
            Realization::Native => sys::recognize(pass, &self).await,
            Realization::Portable => {
                unreachable!("no portable document realization exists yet")
            }
        }
    }
}

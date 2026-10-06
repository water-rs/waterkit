//! Shared serde wire format for the native translation backends.

use serde::{Deserialize, de::DeserializeOwned};

use crate::{
    LanguageIdentifier,
    translation::{
        AssetStatus, LanguagePair, PairCapability, TranslationCapabilities, TranslationError,
    },
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Envelope<T> {
    Ok(T),
    Error(WireError),
}

#[derive(Debug, Deserialize)]
struct WireError {
    kind: ErrorKind,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ErrorKind {
    Unavailable,
    UnsupportedPair,
    NeedsDownload,
    Downloading,
    Platform,
}

#[derive(Debug, Deserialize)]
struct CapabilitiesReply {
    pairs: Vec<PairReply>,
}

#[derive(Debug, Deserialize)]
struct PairReply {
    source: String,
    target: String,
    #[serde(default)]
    status: Option<WireStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireStatus {
    Installed,
    NeedsDownload,
    Downloading,
}

#[cfg(target_os = "android")]
#[derive(Debug, Deserialize)]
struct CapabilityUpdateReply {
    source: String,
    target: String,
    #[serde(default)]
    status: Option<WireStatus>,
}

const fn parse_status(status: &WireStatus) -> AssetStatus {
    match status {
        WireStatus::Installed => AssetStatus::Installed,
        WireStatus::NeedsDownload => AssetStatus::NeedsDownload,
        WireStatus::Downloading => AssetStatus::Downloading,
    }
}

fn parse_language(tag: &str) -> Result<LanguageIdentifier, TranslationError> {
    LanguageIdentifier::try_from_str(tag).map_err(|error| {
        TranslationError::Platform(format!("invalid language tag {tag:?}: {error}"))
    })
}

fn parse_pair(source: &str, target: &str) -> Result<LanguagePair, TranslationError> {
    Ok(LanguagePair::new(
        parse_language(source)?,
        parse_language(target)?,
    ))
}

pub(super) fn decode<T: DeserializeOwned>(
    json: &str,
    pair: Option<&LanguagePair>,
) -> Result<T, TranslationError> {
    let envelope = serde_json::from_str::<Envelope<T>>(json).map_err(|error| {
        TranslationError::Platform(format!("invalid translation response: {error}"))
    })?;
    match envelope {
        Envelope::Ok(value) => Ok(value),
        Envelope::Error(error) => Err(map_error(error, pair)),
    }
}

fn map_error(error: WireError, pair: Option<&LanguagePair>) -> TranslationError {
    match error.kind {
        ErrorKind::Unavailable => TranslationError::Unavailable,
        kind @ (ErrorKind::UnsupportedPair | ErrorKind::NeedsDownload | ErrorKind::Downloading) => {
            let Some(pair) = pair.cloned() else {
                return TranslationError::Platform(format!(
                    "{kind:?} reply without a language pair"
                ));
            };
            match kind {
                ErrorKind::UnsupportedPair => TranslationError::UnsupportedPair(pair),
                ErrorKind::NeedsDownload => TranslationError::NeedsDownload(pair),
                ErrorKind::Downloading => TranslationError::Downloading(pair),
                _ => unreachable!(),
            }
        }
        ErrorKind::Platform => TranslationError::Platform(
            error
                .message
                .unwrap_or_else(|| "native translation service failed".into()),
        ),
    }
}

pub(super) fn decode_capabilities(json: &str) -> Result<TranslationCapabilities, TranslationError> {
    let reply: CapabilitiesReply = match decode(json, None) {
        Ok(reply) => reply,
        Err(TranslationError::Unavailable) => return Ok(TranslationCapabilities::default()),
        Err(error) => return Err(error),
    };

    let pairs = reply
        .pairs
        .into_iter()
        .map(|pair| {
            let language_pair = parse_pair(&pair.source, &pair.target)?;
            Ok(pair
                .status
                .map(|status| PairCapability::new(language_pair, parse_status(&status))))
        })
        .collect::<Result<Vec<_>, TranslationError>>()?
        .into_iter()
        .flatten()
        .collect();

    Ok(TranslationCapabilities::new(pairs))
}

#[cfg(any(target_os = "ios", target_os = "macos", test))]
pub(super) fn decode_pair_status(
    json: &str,
    pair: &LanguagePair,
) -> Result<Option<AssetStatus>, TranslationError> {
    decode::<Option<WireStatus>>(json, Some(pair)).map(|status| status.as_ref().map(parse_status))
}

#[cfg(target_os = "android")]
pub(super) fn decode_capability_update(
    json: &str,
) -> Result<(LanguagePair, Option<AssetStatus>), TranslationError> {
    let update: CapabilityUpdateReply = decode(json, None)?;
    let pair = parse_pair(&update.source, &update.target)?;
    Ok((pair, update.status.as_ref().map(parse_status)))
}

#[cfg(any(target_os = "android", target_os = "ios", target_os = "macos"))]
pub(super) fn encode_texts(texts: &[&str]) -> Result<String, TranslationError> {
    serde_json::to_string(texts)
        .map_err(|error| TranslationError::Platform(format!("encode translation request: {error}")))
}

pub(super) fn decode_translations(
    json: &str,
    pair: &LanguagePair,
    expected_len: usize,
) -> Result<Vec<String>, TranslationError> {
    let translations: Vec<String> = decode(json, Some(pair))?;
    if translations.len() != expected_len {
        return Err(TranslationError::Platform(format!(
            "translation service returned {} results for {expected_len} inputs",
            translations.len()
        )));
    }
    Ok(translations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::langid;

    #[test]
    fn decodes_capabilities_and_omits_null_status() {
        let capabilities = decode_capabilities(
            r#"{"ok":{"pairs":[{"source":"en","target":"de","status":"installed"},{"source":"en","target":"fr","status":null}]}}"#,
        )
        .expect("valid capabilities");
        assert_eq!(capabilities.pairs().len(), 1);
        assert_eq!(
            capabilities.status(&LanguagePair::new(langid!("en"), langid!("de"))),
            Some(AssetStatus::Installed)
        );
    }

    #[test]
    fn unavailable_reply_is_empty_for_capabilities_and_an_error_for_pair_status() {
        let unavailable = r#"{"error":{"kind":"unavailable"}}"#;
        assert_eq!(
            decode_capabilities(unavailable)
                .expect("unavailable capabilities")
                .pairs(),
            []
        );
        assert_eq!(
            decode_pair_status(
                unavailable,
                &LanguagePair::new(langid!("en"), langid!("de"))
            ),
            Err(TranslationError::Unavailable)
        );
    }

    #[test]
    fn maps_error_kinds_to_the_requested_pair() {
        let pair = LanguagePair::new(langid!("en"), langid!("de"));
        let cases = [
            (
                r#"{"error":{"kind":"unavailable"}}"#,
                TranslationError::Unavailable,
            ),
            (
                r#"{"error":{"kind":"unsupported_pair"}}"#,
                TranslationError::UnsupportedPair(pair.clone()),
            ),
            (
                r#"{"error":{"kind":"needs_download"}}"#,
                TranslationError::NeedsDownload(pair.clone()),
            ),
            (
                r#"{"error":{"kind":"downloading"}}"#,
                TranslationError::Downloading(pair.clone()),
            ),
            (
                r#"{"error":{"kind":"platform","message":"native failure"}}"#,
                TranslationError::Platform("native failure".into()),
            ),
        ];
        for (json, expected) in cases {
            let error = decode::<serde_json::Value>(json, Some(&pair)).expect_err("error reply");
            assert_eq!(error, expected);
        }
    }

    #[test]
    fn unknown_error_kind_becomes_platform_error() {
        assert!(matches!(
            decode::<serde_json::Value>(r#"{"error":{"kind":"future_kind"}}"#, None),
            Err(TranslationError::Platform(_))
        ));
    }

    #[test]
    fn invalid_language_tag_names_the_tag() {
        let error = decode_capabilities(
            r#"{"ok":{"pairs":[{"source":"not_a_tag!","target":"de","status":"installed"}]}}"#,
        )
        .expect_err("invalid tag");
        assert!(error.to_string().contains("not_a_tag!"));
    }

    #[test]
    fn invalid_language_tag_is_rejected_even_when_status_is_null() {
        let error = decode_capabilities(
            r#"{"ok":{"pairs":[{"source":"not_a_tag!","target":"de","status":null}]}}"#,
        )
        .expect_err("invalid tag with null status");
        assert!(error.to_string().contains("not_a_tag!"));
    }

    #[test]
    fn translation_length_mismatch_is_a_platform_error() {
        let pair = LanguagePair::new(langid!("en"), langid!("de"));
        let error = decode_translations(r#"{"ok":["Guten Morgen"]}"#, &pair, 2)
            .expect_err("result length mismatch");
        assert!(matches!(error, TranslationError::Platform(_)));
    }
}

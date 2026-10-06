//! Text recognition request and its result types.
//!
//! [`RecognizeText`] asks for the lines of text readable in the image.
//! Requested languages are grouped by the writing system they resolve to —
//! Latin, Chinese, Devanagari, Japanese or Korean — because the realizations
//! serving this request are script engines: one engine reads every language
//! written in its script, and none mixes scripts in one call.

use icu_locale::{LanguageIdentifier, LocaleExpander};

use crate::{
    Quad, VisionError,
    sealed::{Context, Sealed},
    sys,
};

/// How fast a recognition trades accuracy for speed where the serving
/// realization offers the choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum RecognitionLevel {
    /// Favor speed over completeness.
    Fast,
    /// Favor the most accurate reading the realization offers.
    #[default]
    Accurate,
}

/// A writing system the realizations name. Languages resolve to their
/// script; a language that resolves nowhere is unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextScript {
    /// Latin script (`Latn`).
    Latin,
    /// Chinese script (`Hani`, both Hanzi forms).
    Chinese,
    /// Devanagari script (`Deva`).
    Devanagari,
    /// Japanese script (`Jpan`: Han, Hiragana and Katakana together).
    Japanese,
    /// Korean script (`Kore`: Hangul and Han together).
    Korean,
}

impl TextScript {
    /// The script's `und-<Script>` identifier, as reported in
    /// [`VisionCapabilities`](crate::VisionCapabilities).
    #[cfg(target_os = "android")]
    pub fn identifier(self) -> LanguageIdentifier {
        let code = match self {
            Self::Latin => "und-Latn",
            Self::Chinese => "und-Hani",
            Self::Devanagari => "und-Deva",
            Self::Japanese => "und-Jpan",
            Self::Korean => "und-Kore",
        };
        code.parse()
            .expect("the script identifiers are well-formed")
    }

    /// The script a language identifier resolves to, if it is one the
    /// realizations name. Likely-subtags data supplies the script a bare
    /// language carries (`ja` → `Jpan`, `zh-Hans` → `Hani`); an explicit
    /// script subtag always wins.
    fn of(language: &LanguageIdentifier) -> Option<Self> {
        thread_local! {
            static EXPANDER: LocaleExpander = const { LocaleExpander::new_common() };
        }

        let mut resolved = language.clone();
        if resolved.script.is_none() {
            EXPANDER.with(|expander| expander.maximize(&mut resolved));
        }
        match resolved.script.map(|script| script.to_string()).as_deref() {
            Some("Latn") => Some(Self::Latin),
            // `Hani` covers all Han text; `Hans`/`Hant` are its simplified
            // and traditional forms, which the same engines read.
            Some("Hani" | "Hans" | "Hant") => Some(Self::Chinese),
            Some("Deva") => Some(Self::Devanagari),
            // `Jpan` covers Han, Hiragana and Katakana; a request naming a
            // kana script alone still lands on the Japanese engine.
            Some("Jpan" | "Hira" | "Kana") => Some(Self::Japanese),
            // `Kore` covers Hangul and Han; `Hang` names Hangul alone.
            Some("Kore" | "Hang") => Some(Self::Korean),
            _ => None,
        }
    }
}

/// Resolves `languages` to their one shared script.
///
/// `Err` lists the language tags that failed: those that resolved to no
/// served script, or to a script other than the rest. An empty set resolves
/// to [`TextScript::Latin`], the script covering unmarked text.
fn route_languages(languages: &[LanguageIdentifier]) -> Result<TextScript, Vec<String>> {
    if languages.is_empty() {
        return Ok(TextScript::Latin);
    }
    let mut script = None;
    let mut failed = Vec::new();
    for language in languages {
        match (TextScript::of(language), script) {
            (Some(found), None) => script = Some(found),
            (Some(found), Some(same)) if found == same => {}
            (Some(_), Some(_)) | (None, _) => failed.push(language.to_string()),
        }
    }
    if failed.is_empty() {
        Ok(script.expect("a nonempty language list resolves a script"))
    } else {
        Err(failed)
    }
}

/// Recognize the text readable in an image.
///
/// `languages` names the writing system to read: all of them must resolve to
/// the same script — Latin, Chinese, Devanagari, Japanese or Korean. An
/// empty set defaults to Latin, the script covering unmarked text. Planning
/// fails with [`VisionError::Unsupported`] when no serving realization reads
/// the resolved script or the languages resolve to several.
#[derive(Debug)]
pub struct RecognizeText {
    languages: Vec<LanguageIdentifier>,
    level: RecognitionLevel,
}

impl Default for RecognizeText {
    fn default() -> Self {
        Self::new()
    }
}

impl RecognizeText {
    /// A request for the default language set — Latin script.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            languages: Vec::new(),
            level: RecognitionLevel::Accurate,
        }
    }

    /// The languages to recognize, resolved to their scripts at planning.
    #[must_use]
    pub fn languages(mut self, languages: impl IntoIterator<Item = LanguageIdentifier>) -> Self {
        self.languages = languages.into_iter().collect();
        self
    }

    /// The speed/accuracy trade-off, where the realization offers it.
    #[must_use]
    pub const fn with_level(mut self, level: RecognitionLevel) -> Self {
        self.level = level;
        self
    }

    /// The requested speed/accuracy trade-off.
    #[must_use]
    pub const fn level(&self) -> RecognitionLevel {
        self.level
    }

    /// Resolves the requested languages to their one script.
    ///
    /// # Errors
    ///
    /// `Err` carries the language tags that failed to route — those that
    /// resolve to no served script, or to a script other than the rest.
    pub fn script(&self) -> Result<TextScript, Vec<String>> {
        route_languages(&self.languages)
    }
}

impl Sealed for RecognizeText {
    type Plan = sys::TextPlan;

    fn plan(&self, context: Context<'_>) -> Result<Self::Plan, VisionError> {
        sys::plan_text(context, self)
    }
}

impl crate::Request for RecognizeText {
    type Output = Vec<TextLine>;
}

/// One line of text found in the image.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TextLine {
    pub(crate) text: String,
    pub(crate) confidence: f32,
    pub(crate) bounds: Quad,
}

impl TextLine {
    /// The recognized text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The engine's confidence in the line, from `0.0` to `1.0`.
    #[must_use]
    pub const fn confidence(&self) -> f32 {
        self.confidence
    }

    /// Where in the image the line sits, normalized into the upright
    /// image's frame.
    #[must_use]
    pub const fn bounds(&self) -> Quad {
        self.bounds
    }
}

#[cfg(test)]
mod tests {
    use icu_locale::LanguageIdentifier;

    use super::{TextScript, route_languages};

    fn routes(tags: &[&str]) -> Result<TextScript, Vec<String>> {
        route_languages(
            &tags
                .iter()
                .map(|tag| tag.parse::<LanguageIdentifier>().unwrap())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn languages_route_to_their_script() {
        assert_eq!(routes(&[]), Ok(TextScript::Latin));
        assert_eq!(routes(&["en", "fr"]), Ok(TextScript::Latin));
        assert_eq!(routes(&["ja"]), Ok(TextScript::Japanese));
        assert_eq!(routes(&["ko-KR"]), Ok(TextScript::Korean));
        assert_eq!(routes(&["zh-Hans", "zh-Hant"]), Ok(TextScript::Chinese));
        assert_eq!(routes(&["hi", "mr"]), Ok(TextScript::Devanagari));
        assert_eq!(routes(&["und-Kana"]), Ok(TextScript::Japanese));
        assert_eq!(routes(&["und-Hang"]), Ok(TextScript::Korean));
    }

    #[test]
    fn mixed_or_unknown_scripts_fail_with_the_offending_tags() {
        assert_eq!(routes(&["en", "ja"]), Err(vec!["ja".to_string()]));
        assert!(routes(&["ar"]).is_err());
        assert!(routes(&["und-Arab"]).is_err());
    }
}

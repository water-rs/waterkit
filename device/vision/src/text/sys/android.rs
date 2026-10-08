//! Android native text realization: Play services ML Kit's script
//! recognizers, unbundled.
//!
//! The `play-services-mlkit-text-recognition*` artifacts are thin clients;
//! the engines live in modules Play services delivers on demand and
//! [`recognize`] installs. ML Kit serves writing systems, not languages —
//! one engine reads every language written in its script — so a request's
//! languages route through likely-subtags expansion to their script, and
//! languages resolving to several scripts decline the offer.

use std::sync::Arc;

use icu_locale::LocaleExpander;
use icu_locale_core::LanguageIdentifier;
use jni::objects::{JFloatArray, JIntArray, JObject, JObjectArray, JString, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{
    AndroidError, FromJava, NativeCallback, decode_string, describe_jni_error, with_android_context,
};

use crate::sys::android::mlkit::{
    MODULE_CHINESE, MODULE_DEVANAGARI, MODULE_JAPANESE, MODULE_KOREAN, MODULE_LATIN,
};
use crate::sys::android::mlkit::{MlInput, SharedInput, TEXT_HELPER, prepare_module, quad};
use crate::sys::android::play_services;
use crate::{
    TextLine, TextWord, VisionError,
    sealed::{Offer, Pass},
    text::{RecognizeText, TextPlan},
};

/// A writing system ML Kit's recognizers name. Languages resolve to their
/// script; a language that resolves nowhere is unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    Latin,
    Chinese,
    Devanagari,
    Japanese,
    Korean,
}

impl Script {
    /// The `VisionTextHelper` module code for the script.
    const fn module(self) -> i32 {
        match self {
            Self::Latin => MODULE_LATIN,
            Self::Chinese => MODULE_CHINESE,
            Self::Devanagari => MODULE_DEVANAGARI,
            Self::Japanese => MODULE_JAPANESE,
            Self::Korean => MODULE_KOREAN,
        }
    }

    /// The script's `und-<Script>` identifier, as reported by
    /// [`recognizer_languages`].
    fn identifier(self) -> LanguageIdentifier {
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
    /// recognizers name. Likely-subtags data supplies the script a bare
    /// language carries (`ja` → `Jpan`, `zh-Hans` → `Hani`); an explicit
    /// script subtag always wins.
    fn of(language: &LanguageIdentifier, expander: &LocaleExpander) -> Option<Self> {
        let mut resolved = language.clone();
        if resolved.script.is_none() {
            expander.maximize(&mut resolved);
        }
        match resolved.script.map(|script| script.to_string()).as_deref() {
            Some("Latn") => Some(Self::Latin),
            // `Hani` covers all Han text; `Hans`/`Hant` are its simplified
            // and traditional forms, which the same engine reads.
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

/// The scripts ML Kit's text recognizers serve.
const SCRIPTS: [Script; 5] = [
    Script::Latin,
    Script::Chinese,
    Script::Devanagari,
    Script::Japanese,
    Script::Korean,
];

/// Resolves `languages` to their one shared script.
///
/// `Err` lists the language tags that failed: those that resolved to no
/// served script, or to a script other than the rest. An empty set resolves
/// to [`Script::Latin`], the script covering unmarked text.
fn route(languages: &[LanguageIdentifier]) -> Result<Script, Vec<String>> {
    let Some((first, rest)) = languages.split_first() else {
        return Ok(Script::Latin);
    };
    let expander = LocaleExpander::new_common();
    let mut script = Script::of(first, &expander);
    let mut failed = Vec::new();
    if script.is_none() {
        failed.push(first.to_string());
    }
    for language in rest {
        match (Script::of(language, &expander), script) {
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

/// The scripts the native text realization serves on this device, as
/// `und-<Script>` identifiers — every one when Google Play services is
/// usable, none when it is not.
///
/// # Panics
///
/// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet, or the
/// JNI probe fails — a probe failure is a bug, not an absent realization.
pub fn recognizer_languages() -> Vec<LanguageIdentifier> {
    match play_services() {
        Ok(true) => SCRIPTS.iter().map(|script| script.identifier()).collect(),
        Ok(false) => Vec::new(),
        Err(error) => panic!("waterkit-vision: {error}"),
    }
}

/// What the native realization offers for `request`: the languages resolve
/// to one served script, or [`Offer::Lacks`] names the tags that routed
/// elsewhere; [`Offer::Absent`] when Play services is unavailable.
pub fn offer(request: &RecognizeText) -> Offer {
    if let Err(failed) = route(&request.languages) {
        return Offer::Lacks(format!(
            "languages [{}] resolving to one served script",
            failed.join(", ")
        ));
    }
    match play_services() {
        Ok(true) => Offer::Serves,
        Ok(false) => Offer::Absent,
        Err(error) => Offer::Lacks(error.to_string()),
    }
}

/// The script's recognizer module is installed on the worker thread that
/// runs the request, so plan preparation is pure routing — nothing to do.
#[expect(
    clippy::unnecessary_wraps,
    reason = "keeps the signature every platform's native realization shares"
)]
pub const fn prepare(_plan: &TextPlan) -> Result<(), VisionError> {
    Ok(())
}

/// The script the plan's languages resolve to; `offer` already declined a
/// request that resolves to none or several.
fn script(plan: &TextPlan) -> Script {
    route(&plan.languages).expect("the offer declined unservable languages")
}

/// One element of a `TextRow`, decoded into owned data: the word's text,
/// confidence and stored-space corner points.
struct TextWordRow {
    text: String,
    confidence: f32,
    points: Vec<i32>,
}

/// One `TextRow` element, decoded into owned data.
struct TextRowData {
    text: String,
    confidence: f32,
    points: Vec<i32>,
    words: Vec<TextWordRow>,
}

impl TextRowData {
    /// The row's typed [`TextLine`]: the flat corner points normalized into
    /// the upright image.
    fn into_line(self, input: &MlInput) -> TextLine {
        TextLine {
            text: self.text,
            confidence: Some(self.confidence),
            bounds: quad(
                input.rotation_degrees,
                input.width,
                input.height,
                &self.points,
            ),
            words: self
                .words
                .into_iter()
                .map(|word| TextWord {
                    text: word.text,
                    confidence: Some(word.confidence),
                    bounds: quad(
                        input.rotation_degrees,
                        input.width,
                        input.height,
                        &word.points,
                    ),
                })
                .collect(),
        }
    }
}

/// What the Kotlin helper completes the request's `NativeCallback` with:
/// the `TextRow[]` read field-by-field.
struct TextRows(Vec<TextRowData>);

impl FromJava for TextRows {
    fn from_java(env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        let rows = env.as_cast::<JObjectArray<JObject>>(object)?;
        let count = rows.len(env)?;
        let mut lines = Vec::with_capacity(count);
        for index in 0..count {
            let row = rows.get_element(env, index)?;
            let text = env
                .get_field(&row, jni_str!("text"), jni_sig!("Ljava/lang/String;"))
                .and_then(JValueOwned::l)?;
            let text = decode_string(env, &text)?;
            let confidence = env
                .get_field(&row, jni_str!("confidence"), jni_sig!("F"))
                .and_then(JValueOwned::f)?;
            let points = int_array(env, &row, jni_str!("points"))?;

            let texts = env
                .get_field(&row, jni_str!("wordTexts"), jni_sig!("[Ljava/lang/String;"))
                .and_then(JValueOwned::l)?;
            let texts = env.cast_local::<JObjectArray<JString>>(texts)?;
            let confidences = env
                .get_field(&row, jni_str!("wordConfidences"), jni_sig!("[F"))
                .and_then(JValueOwned::l)?;
            let confidences = env.cast_local::<JFloatArray>(confidences)?;
            let mut confidence_values = vec![0f32; confidences.len(env)?];
            confidences.get_region(env, 0, &mut confidence_values)?;
            let word_points = env
                .get_field(&row, jni_str!("wordPoints"), jni_sig!("[[I"))
                .and_then(JValueOwned::l)?;
            let word_points = env.cast_local::<JObjectArray<JIntArray>>(word_points)?;

            // `wordTexts`, `wordConfidences` and `wordPoints` are parallel:
            // indexing them together keeps a disagreeing helper a bounds
            // error instead of a silent truncation.
            let word_count = texts.len(env)?;
            let mut words = Vec::with_capacity(word_count);
            for index in 0..word_count {
                let word_text = texts.get_element(env, index)?;
                let word_text = decode_string(env, word_text.as_ref())?;
                let confidence = confidence_values
                    .get(index)
                    .copied()
                    .ok_or(jni::errors::Error::IndexOutOfBounds)?;
                let word_points_row = word_points.get_element(env, index)?;
                let mut flat = vec![0i32; word_points_row.len(env)?];
                word_points_row.get_region(env, 0, &mut flat)?;
                words.push(TextWordRow {
                    text: word_text,
                    confidence,
                    points: flat,
                });
            }
            lines.push(TextRowData {
                text,
                confidence,
                points,
                words,
            });
        }
        Ok(Self(lines))
    }
}

/// Reads an `[I` field off a `TextRow` into an owned `Vec<i32>`.
fn int_array(
    env: &mut Env<'_>,
    row: &JObject<'_>,
    name: &'static jni::strings::JNIStr,
) -> Result<Vec<i32>, AndroidError> {
    let points = env
        .get_field(row, name, jni_sig!("[I"))
        .and_then(JValueOwned::l)?;
    let points = env.cast_local::<JIntArray>(points)?;
    let mut flat = vec![0i32; points.len(env)?];
    points.get_region(env, 0, &mut flat)?;
    Ok(flat)
}

/// Runs the script's recognizer over the pass's shared [`MlInput`],
/// installing the script's module first when Play services lacks it. The
/// module install and the recognition both answer through `NativeCallback`s
/// the helper completes from its task listeners, so nothing is parked
/// waiting.
pub async fn recognize(pass: &mut Pass<'_>, plan: &TextPlan) -> Result<Vec<TextLine>, VisionError> {
    tracing::debug!(
        level = ?plan.level,
        "recognizing text with ML Kit"
    );
    let input = Arc::clone(&pass.prepared::<SharedInput>().await?.0);
    let script = script(plan).module();
    prepare_module(&TEXT_HELPER, script).await?;
    let rx = with_android_context(|env, context| -> Result<_, VisionError> {
        let class = TEXT_HELPER.class(env, context)?;
        let (callback, rx) = NativeCallback::<TextRows>::new(env).map_err(|error| {
            VisionError::Platform(format!("create the text callback failed: {error}"))
        })?;
        env.call_static_method(
            class,
            jni_str!("recognizeText"),
            jni_sig!(
                "(Lcom/google/mlkit/vision/common/InputImage;Lwaterkit/build/NativeCallback;I)V"
            ),
            &[
                JValue::Object(input.input.as_obj()),
                JValue::Object(callback.as_obj()),
                JValue::Int(script),
            ],
        )
        .map_err(|error| {
            VisionError::Platform(format!(
                "recognize text: {}",
                describe_jni_error(env, error)
            ))
        })?;
        Ok(rx)
    })?;
    let rows = rx
        .await
        .map_err(|_| {
            VisionError::Platform(String::from(
                "the text recognition callback was collected unanswered",
            ))
        })?
        .map_err(|error| VisionError::Platform(error.to_string()))?;
    Ok(rows
        .0
        .into_iter()
        .map(|row| row.into_line(&input))
        .collect())
}

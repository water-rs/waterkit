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
use jni::objects::{JIntArray, JObject, JObjectArray, JString, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{decode_string, describe_jni_error};

use crate::sys::android::mlkit::{
    MODULE_CHINESE, MODULE_DEVANAGARI, MODULE_JAPANESE, MODULE_KOREAN, MODULE_LATIN,
};
use crate::sys::android::mlkit::{
    MlInput, SharedInput, TEXT_HELPER, on_vision_thread, prepare_module, quad,
};
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

/// One element of a `TextRow`: the word's text, confidence and stored-space
/// corner points.
fn text_word(
    env: &Env<'_>,
    text: &JObject<'_>,
    confidence: f32,
    points: JObject<'_>,
    input: &MlInput,
) -> Result<TextWord, VisionError> {
    let word = decode_string(env, text)
        .map_err(|error| VisionError::Platform(format!("word text: {error}")))?;
    let points = env
        .cast_local::<JIntArray>(points)
        .map_err(|error| VisionError::Platform(format!("word points cast: {error}")))?;
    let length = points
        .len(env)
        .map_err(|error| VisionError::Platform(format!("word points len: {error}")))?;
    let mut flat = vec![0i32; length];
    points
        .get_region(env, 0, &mut flat)
        .map_err(|error| VisionError::Platform(format!("word points read: {error}")))?;
    Ok(TextWord {
        text: word,
        confidence: Some(confidence),
        bounds: quad(input.rotation_degrees, input.width, input.height, &flat),
    })
}

/// Reads one `TextRow` into a [`TextLine`].
fn text_line(
    env: &mut Env<'_>,
    row: &JObject<'_>,
    input: &MlInput,
) -> Result<TextLine, VisionError> {
    let text = env
        .get_field(row, jni_str!("text"), jni_sig!("Ljava/lang/String;"))
        .and_then(JValueOwned::l)
        .map_err(|error| VisionError::Platform(format!("line text: {error}")))?;
    let text = decode_string(env, &text)
        .map_err(|error| VisionError::Platform(format!("line text: {error}")))?;
    let confidence = env
        .get_field(row, jni_str!("confidence"), jni_sig!("F"))
        .and_then(JValueOwned::f)
        .map_err(|error| VisionError::Platform(format!("line confidence: {error}")))?;
    let points = env
        .get_field(row, jni_str!("points"), jni_sig!("[I"))
        .and_then(JValueOwned::l)
        .map_err(|error| VisionError::Platform(format!("line points: {error}")))?;
    let points = env
        .cast_local::<JIntArray>(points)
        .map_err(|error| VisionError::Platform(format!("line points cast: {error}")))?;
    let length = points
        .len(env)
        .map_err(|error| VisionError::Platform(format!("line points len: {error}")))?;
    let mut flat = vec![0i32; length];
    points
        .get_region(env, 0, &mut flat)
        .map_err(|error| VisionError::Platform(format!("line points read: {error}")))?;

    let texts = env
        .get_field(row, jni_str!("wordTexts"), jni_sig!("[Ljava/lang/String;"))
        .and_then(JValueOwned::l)
        .map_err(|error| VisionError::Platform(format!("word texts: {error}")))?;
    let texts = env
        .cast_local::<JObjectArray<JString>>(texts)
        .map_err(|error| VisionError::Platform(format!("word texts cast: {error}")))?;
    let confidences = env
        .get_field(row, jni_str!("wordConfidences"), jni_sig!("[F"))
        .and_then(JValueOwned::l)
        .map_err(|error| VisionError::Platform(format!("word confidences: {error}")))?;
    let confidences = env
        .cast_local::<jni::objects::JFloatArray>(confidences)
        .map_err(|error| VisionError::Platform(format!("word confidences cast: {error}")))?;
    let confidence_count = confidences
        .len(env)
        .map_err(|error| VisionError::Platform(format!("word confidences len: {error}")))?;
    let mut confidence_values = vec![0f32; confidence_count];
    confidences
        .get_region(env, 0, &mut confidence_values)
        .map_err(|error| VisionError::Platform(format!("word confidences read: {error}")))?;
    let word_points = env
        .get_field(row, jni_str!("wordPoints"), jni_sig!("[[I"))
        .and_then(JValueOwned::l)
        .map_err(|error| VisionError::Platform(format!("word points: {error}")))?;
    let word_points = env
        .cast_local::<JObjectArray<JObject>>(word_points)
        .map_err(|error| VisionError::Platform(format!("word points cast: {error}")))?;

    let count = texts
        .len(env)
        .map_err(|error| VisionError::Platform(format!("word texts len: {error}")))?;
    let points_count = word_points
        .len(env)
        .map_err(|error| VisionError::Platform(format!("word points len: {error}")))?;
    if confidence_values.len() != count || points_count != count {
        return Err(VisionError::Platform(format!(
            "word rows disagree: {count} texts, {} confidences, {points_count} point sets",
            confidence_values.len()
        )));
    }
    let mut words = Vec::with_capacity(count);
    for (index, &confidence) in confidence_values.iter().enumerate() {
        let word_text = texts
            .get_element(env, index)
            .map_err(|error| VisionError::Platform(format!("word text {index}: {error}")))?;
        let points = word_points
            .get_element(env, index)
            .map_err(|error| VisionError::Platform(format!("word points {index}: {error}")))?;
        words.push(text_word(
            env,
            word_text.as_ref(),
            confidence,
            points,
            input,
        )?);
    }
    Ok(TextLine {
        text,
        confidence: Some(confidence),
        bounds: quad(input.rotation_degrees, input.width, input.height, &flat),
        words,
    })
}

/// Runs the script's recognizer over the pass's shared [`MlInput`].
pub async fn recognize(pass: &mut Pass<'_>, plan: &TextPlan) -> Result<Vec<TextLine>, VisionError> {
    tracing::debug!(
        level = ?plan.level,
        "recognizing text with ML Kit"
    );
    let input = Arc::clone(&pass.prepared::<SharedInput>().await?.0);
    let script = script(plan).module();
    on_vision_thread("waterkit-vision-text", move |env, context| {
        prepare_module(env, context, &TEXT_HELPER, script)?;
        let class = TEXT_HELPER.class(env, context)?;
        let rows = env
            .call_static_method(
                class,
                jni_str!("recognizeText"),
                jni_sig!(
                    "(Lcom/google/mlkit/vision/common/InputImage;I)[Lwaterkit/vision/VisionTextHelper$TextRow;"
                ),
                &[JValue::Object(input.input.as_obj()), JValue::Int(script)],
            )
            .and_then(JValueOwned::l)
            .map_err(|error| {
                VisionError::Platform(format!(
                    "recognize text: {}",
                    describe_jni_error(env, error)
                ))
            })?;
        let rows = env
            .cast_local::<JObjectArray<JObject>>(rows)
            .map_err(|error| VisionError::Platform(format!("text rows cast: {error}")))?;
        let count = rows
            .len(env)
            .map_err(|error| VisionError::Platform(format!("text rows len: {error}")))?;
        let mut lines = Vec::with_capacity(count);
        for index in 0..count {
            let row = rows.get_element(env, index).map_err(|error| {
                VisionError::Platform(format!("text row {index}: {error}"))
            })?;
            lines.push(text_line(env, &row, &input)?);
        }
        Ok(lines)
    })
    .await
}

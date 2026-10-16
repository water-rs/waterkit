//! Apple Translation framework implementation.

use futures::channel::oneshot;

use crate::{
    sys::wire,
    translation::{AssetStatus, LanguagePair, TranslationCapabilities, TranslationError},
};

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn language_capabilities(callback: Box<dyn FnOnce(String) -> ()>);
        fn language_pair_status(
            source: &str,
            target: &str,
            callback: Box<dyn FnOnce(String) -> ()>,
        );
        fn language_translator_create(
            source: &str,
            target: &str,
            callback: Box<dyn FnOnce(String) -> ()>,
        );
        fn language_translate(id: u64, texts_json: &str, callback: Box<dyn FnOnce(String) -> ()>);
        fn language_translator_release(id: u64);
    }
}

pub async fn capabilities() -> Result<TranslationCapabilities, TranslationError> {
    let (sender, receiver) = oneshot::channel();
    ffi::language_capabilities(Box::new(move |json| {
        let _ = sender.send(json);
    }));
    let json = receiver
        .await
        .map_err(|_| TranslationError::Platform("Apple capability callback was dropped".into()))?;
    wire::decode_capabilities(&json)
}

pub async fn pair_status(pair: &LanguagePair) -> Result<Option<AssetStatus>, TranslationError> {
    let (sender, receiver) = oneshot::channel();
    ffi::language_pair_status(
        &pair.source().to_string(),
        &pair.target().to_string(),
        Box::new(move |json| {
            let _ = sender.send(json);
        }),
    );
    let json = receiver
        .await
        .map_err(|_| TranslationError::Platform("Apple pair-status callback was dropped".into()))?;
    wire::decode_pair_status(&json, pair)
}

#[derive(Debug)]
pub struct Translator {
    id: u64,
    pair: LanguagePair,
}

impl Translator {
    pub async fn create(pair: &LanguagePair) -> Result<Self, TranslationError> {
        let (sender, receiver) = oneshot::channel();
        ffi::language_translator_create(
            &pair.source().to_string(),
            &pair.target().to_string(),
            Box::new(move |json| {
                let _ = sender.send(json);
            }),
        );
        let json = receiver.await.map_err(|_| {
            TranslationError::Platform("Apple translator-create callback was dropped".into())
        })?;
        let id = wire::decode::<u64>(&json, Some(pair))?;
        Ok(Self {
            id,
            pair: pair.clone(),
        })
    }

    pub async fn translate(&self, texts: &[&str]) -> Result<Vec<String>, TranslationError> {
        let texts_json = wire::encode_texts(texts)?;
        let (sender, receiver) = oneshot::channel();
        ffi::language_translate(
            self.id,
            &texts_json,
            Box::new(move |json| {
                let _ = sender.send(json);
            }),
        );
        let json = receiver.await.map_err(|_| {
            TranslationError::Platform("Apple translation callback was dropped".into())
        })?;
        wire::decode_translations(&json, &self.pair, texts.len())
    }
}

impl Drop for Translator {
    fn drop(&mut self) {
        ffi::language_translator_release(self.id);
    }
}

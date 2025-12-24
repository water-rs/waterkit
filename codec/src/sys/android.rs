//! Android MediaCodec implementation.

use crate::{CodecError, CodecType, Frame, VideoDecoder, VideoEncoder};

pub struct AndroidEncoder;

impl AndroidEncoder {
    pub fn new(_codec: CodecType) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

impl VideoEncoder for AndroidEncoder {
    fn encode(&mut self, _frame: &Frame) -> Result<Vec<u8>, CodecError> {
        Err(CodecError::Unknown("Not implemented".into()))
    }
}

pub struct AndroidDecoder;

impl AndroidDecoder {
    pub fn new(_codec: CodecType) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

impl VideoDecoder for AndroidDecoder {
    fn decode(&mut self, _data: &[u8]) -> Result<Vec<Frame>, CodecError> {
        Err(CodecError::Unknown("Not implemented".into()))
    }
}

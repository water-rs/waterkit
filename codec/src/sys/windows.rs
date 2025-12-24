//! Windows Media Foundation implementation.

use crate::{CodecError, CodecType, Frame, VideoDecoder, VideoEncoder};

pub struct WindowsEncoder;

impl WindowsEncoder {
    pub fn new(_codec: CodecType) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

impl VideoEncoder for WindowsEncoder {
    fn encode(&mut self, _frame: &Frame) -> Result<Vec<u8>, CodecError> {
        Err(CodecError::Unknown("Not implemented".into()))
    }
}

pub struct WindowsDecoder;

impl WindowsDecoder {
    pub fn new(_codec: CodecType) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

impl VideoDecoder for WindowsDecoder {
    fn decode(&mut self, _data: &[u8]) -> Result<Vec<Frame>, CodecError> {
        Err(CodecError::Unknown("Not implemented".into()))
    }
}

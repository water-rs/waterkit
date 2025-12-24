//! Stub implementation for unsupported platforms (e.g. Linux for now).
use crate::{VideoEncoder, VideoDecoder, CodecError, Frame, CodecType};

pub struct StubEncoder;

impl StubEncoder {
    pub fn new(_codec: CodecType) -> Result<Self, CodecError> {
        Err(CodecError::NotSupported)
    }
}

impl VideoEncoder for StubEncoder {
    fn encode(&mut self, _frame: &Frame) -> Result<Vec<u8>, CodecError> {
        Err(CodecError::NotSupported)
    }
}

pub struct StubDecoder;

impl StubDecoder {
    pub fn new(_codec: CodecType) -> Result<Self, CodecError> {
        Err(CodecError::NotSupported)
    }
}

impl VideoDecoder for StubDecoder {
    fn decode(&mut self, _data: &[u8]) -> Result<Vec<Frame>, CodecError> {
        Err(CodecError::NotSupported)
    }
}

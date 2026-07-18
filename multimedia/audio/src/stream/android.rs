//! Android AAC packet decoding through the platform `MediaCodec` pipeline.

use std::{mem::MaybeUninit, num::NonZeroU16, time::Duration};

use ndk::media::{
    media_codec::{
        DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection,
    },
    media_format::MediaFormat,
};

use super::{
    AacDecoderConfig, DecodedAudioFrame, EncodedAudioPacket, PacketAudioError, PcmFrameError,
};

const AAC_MIME_TYPE: &str = "audio/mp4a-latm";
const PCM_16_BIT: i32 = 2;
const PCM_FLOAT: i32 = 4;
const INPUT_TIMEOUT: Duration = Duration::from_secs(1);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const END_OF_STREAM_FLAG: u32 = ndk_sys::AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM;

pub(super) struct AndroidAacDecoder {
    codec: MediaCodec,
    channels: NonZeroU16,
    sample_rate: std::num::NonZeroU32,
    pcm_encoding: i32,
}

impl std::fmt::Debug for AndroidAacDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidAacDecoder")
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .field("pcm_encoding", &self.pcm_encoding)
            .finish_non_exhaustive()
    }
}

// SAFETY: `AndroidAacDecoder` has exclusive ownership of the codec and every
// NDK call is serialized through `&mut self` on the decode worker.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for AndroidAacDecoder {}

impl AndroidAacDecoder {
    pub(super) fn new(config: AacDecoderConfig) -> Result<Self, String> {
        let AacDecoderConfig {
            audio_specific_config,
            channels,
            sample_rate,
        } = config;
        let codec = MediaCodec::from_decoder_type(AAC_MIME_TYPE)
            .ok_or_else(|| String::from("Android MediaCodec has no AAC decoder"))?;
        let mut format = MediaFormat::new();
        format.set_str("mime", AAC_MIME_TYPE);
        format.set_i32("channel-count", i32::from(channels.get()));
        format.set_i32(
            "sample-rate",
            i32::try_from(sample_rate.get())
                .expect("validated AAC sample rate must fit Android MediaFormat i32"),
        );
        format.set_i32("is-adts", 0);
        format.set_i32("pcm-encoding", PCM_FLOAT);
        format.set_buffer("csd-0", &audio_specific_config);
        codec
            .configure(&format, None, MediaCodecDirection::Decoder)
            .map_err(|error| format!("MediaCodec AAC configure failed: {error}"))?;
        codec
            .start()
            .map_err(|error| format!("MediaCodec AAC start failed: {error}"))?;
        Ok(Self {
            codec,
            channels,
            sample_rate,
            pcm_encoding: PCM_16_BIT,
        })
    }

    pub(super) fn decode(
        &mut self,
        packet: &EncodedAudioPacket,
    ) -> Result<Vec<DecodedAudioFrame>, PacketAudioError> {
        if packet.is_discontinuity() {
            self.reset();
        }
        let presentation_time = packet.presentation_time();
        let mut input = match self
            .codec
            .dequeue_input_buffer(INPUT_TIMEOUT)
            .map_err(|error| decode_error(presentation_time, error))?
        {
            DequeuedInputBufferResult::Buffer(buffer) => buffer,
            DequeuedInputBufferResult::TryAgainLater => {
                return Err(decode_message(
                    presentation_time,
                    "MediaCodec did not provide an AAC input buffer before the timeout",
                ));
            }
        };
        let input_capacity = input.buffer_mut().len();
        if packet.data().len() > input_capacity {
            return Err(decode_message(
                presentation_time,
                format!(
                    "AAC access unit is {} bytes but MediaCodec input capacity is {input_capacity}",
                    packet.data().len()
                ),
            ));
        }
        for (destination, source) in input.buffer_mut().iter_mut().zip(packet.data()) {
            *destination = MaybeUninit::new(*source);
        }
        self.codec
            .queue_input_buffer(
                input,
                0,
                packet.data().len(),
                u64::try_from(presentation_time.as_micros())
                    .expect("AAC presentation timestamp must fit Android MediaCodec u64"),
                0,
            )
            .map_err(|error| decode_error(presentation_time, error))?;

        self.collect_output(Duration::ZERO, false, presentation_time)
    }

    pub(super) fn finish(&mut self) -> Result<Vec<DecodedAudioFrame>, PacketAudioError> {
        let presentation_time = Duration::ZERO;
        let input = match self
            .codec
            .dequeue_input_buffer(INPUT_TIMEOUT)
            .map_err(|error| decode_error(presentation_time, error))?
        {
            DequeuedInputBufferResult::Buffer(buffer) => buffer,
            DequeuedInputBufferResult::TryAgainLater => {
                return Err(decode_message(
                    presentation_time,
                    "MediaCodec did not provide an AAC end-of-stream buffer before the timeout",
                ));
            }
        };
        self.codec
            .queue_input_buffer(input, 0, 0, 0, END_OF_STREAM_FLAG)
            .map_err(|error| decode_error(presentation_time, error))?;
        self.collect_output(DRAIN_TIMEOUT, true, presentation_time)
    }

    fn collect_output(
        &mut self,
        timeout: Duration,
        require_end_of_stream: bool,
        submitted_time: Duration,
    ) -> Result<Vec<DecodedAudioFrame>, PacketAudioError> {
        let mut frames = Vec::new();
        loop {
            match self
                .codec
                .dequeue_output_buffer(timeout)
                .map_err(|error| decode_error(submitted_time, error))?
            {
                DequeuedOutputBufferInfoResult::Buffer(output) => {
                    let flags = output.info().flags();
                    let output_time = Duration::from_micros(
                        u64::try_from(output.info().presentation_time_us()).map_err(|_| {
                            decode_message(
                                submitted_time,
                                "MediaCodec emitted a negative AAC presentation timestamp",
                            )
                        })?,
                    );
                    let offset = usize::try_from(output.info().offset())
                        .expect("MediaCodec AAC output offset must be non-negative");
                    let size = usize::try_from(output.info().size())
                        .expect("MediaCodec AAC output size must be non-negative");
                    let bytes = &output.buffer()[offset..offset + size];
                    let samples = decode_pcm(bytes, self.pcm_encoding, submitted_time)?;
                    self.codec
                        .release_output_buffer(output, false)
                        .map_err(|error| decode_error(submitted_time, error))?;
                    if !samples.is_empty() {
                        frames.push(
                            DecodedAudioFrame::from_interleaved_pcm(
                                output_time,
                                self.channels,
                                self.sample_rate,
                                samples,
                            )
                            .map_err(|error| pcm_error(submitted_time, &error))?,
                        );
                    }
                    if flags & END_OF_STREAM_FLAG != 0 {
                        return Ok(frames);
                    }
                }
                DequeuedOutputBufferInfoResult::OutputFormatChanged => {
                    self.apply_output_format(submitted_time)?;
                }
                DequeuedOutputBufferInfoResult::OutputBuffersChanged => {}
                DequeuedOutputBufferInfoResult::TryAgainLater => {
                    if require_end_of_stream {
                        return Err(decode_message(
                            submitted_time,
                            "MediaCodec did not signal AAC end-of-stream before the drain timeout",
                        ));
                    }
                    return Ok(frames);
                }
            }
        }
    }

    pub(super) fn reset(&self) {
        self.codec
            .flush()
            .expect("Android MediaCodec AAC flush must succeed");
    }

    fn apply_output_format(&mut self, presentation_time: Duration) -> Result<(), PacketAudioError> {
        let format = self.codec.output_format();
        let channels = format.i32("channel-count").ok_or_else(|| {
            decode_message(
                presentation_time,
                "MediaCodec AAC output omitted channel-count",
            )
        })?;
        let sample_rate = format.i32("sample-rate").ok_or_else(|| {
            decode_message(
                presentation_time,
                "MediaCodec AAC output omitted sample-rate",
            )
        })?;
        let actual_channels = u16::try_from(channels).map_err(|_| {
            decode_message(
                presentation_time,
                format!("MediaCodec AAC output has invalid channel count {channels}"),
            )
        })?;
        let actual_sample_rate = u32::try_from(sample_rate).map_err(|_| {
            decode_message(
                presentation_time,
                format!("MediaCodec AAC output has invalid sample rate {sample_rate}"),
            )
        })?;
        if actual_channels != self.channels.get() || actual_sample_rate != self.sample_rate.get() {
            return Err(PacketAudioError::UnexpectedFormatChange {
                expected_channels: self.channels.get(),
                expected_sample_rate: self.sample_rate.get(),
                actual_channels,
                actual_sample_rate,
            });
        }
        self.pcm_encoding = format.i32("pcm-encoding").unwrap_or(PCM_16_BIT);
        if !matches!(self.pcm_encoding, PCM_16_BIT | PCM_FLOAT) {
            return Err(decode_message(
                presentation_time,
                format!(
                    "MediaCodec AAC output uses unsupported PCM encoding {}",
                    self.pcm_encoding
                ),
            ));
        }
        Ok(())
    }
}

fn decode_pcm(
    bytes: &[u8],
    encoding: i32,
    presentation_time: Duration,
) -> Result<Vec<f32>, PacketAudioError> {
    match encoding {
        PCM_16_BIT => {
            let chunks = bytes.chunks_exact(2);
            if !chunks.remainder().is_empty() {
                return Err(decode_message(
                    presentation_time,
                    "MediaCodec AAC emitted misaligned PCM16 bytes",
                ));
            }
            Ok(chunks
                .map(|chunk| {
                    f32::from(i16::from_ne_bytes([chunk[0], chunk[1]])) / f32::from(i16::MAX)
                })
                .collect())
        }
        PCM_FLOAT => {
            let chunks = bytes.chunks_exact(4);
            if !chunks.remainder().is_empty() {
                return Err(decode_message(
                    presentation_time,
                    "MediaCodec AAC emitted misaligned float PCM bytes",
                ));
            }
            Ok(chunks
                .map(|chunk| f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect())
        }
        _ => unreachable!("PCM encoding must be validated on output-format change"),
    }
}

fn decode_error(presentation_time: Duration, error: impl std::fmt::Display) -> PacketAudioError {
    decode_message(presentation_time, error.to_string())
}

fn pcm_error(presentation_time: Duration, error: &PcmFrameError) -> PacketAudioError {
    decode_message(presentation_time, error.to_string())
}

fn decode_message(presentation_time: Duration, message: impl Into<String>) -> PacketAudioError {
    PacketAudioError::Decode {
        codec: "AAC",
        presentation_time,
        message: message.into(),
    }
}

//! Windows AAC packet decoding through Media Foundation.

use std::{mem::ManuallyDrop, num::NonZeroU16, ptr, time::Duration};

use windows::Win32::{
    Media::MediaFoundation::{
        CMSAACDecMFT, IMFMediaType, IMFSample, IMFTransform, MF_E_TRANSFORM_NEED_MORE_INPUT,
        MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION,
        MF_MT_AAC_PAYLOAD_TYPE, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
        MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_MT_USER_DATA, MF_VERSION, MFAudioFormat_AAC,
        MFAudioFormat_Float, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
        MFMediaType_Audio, MFSTARTUP_NOSOCKET, MFShutdown, MFStartup, MFT_MESSAGE_COMMAND_DRAIN,
        MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
        MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
        MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES, MFT_OUTPUT_STREAM_INFO,
        MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    },
    System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    },
};

use super::{
    AacDecoderConfig, DecodedAudioFrame, EncodedAudioPacket, PacketAudioError, PcmFrameError,
};

const RAW_AAC_PAYLOAD: u32 = 0;
const UNSPECIFIED_AAC_PROFILE_LEVEL: u32 = 0xfe;
const HE_AAC_USER_DATA_PREFIX_LEN: usize = 12;
const BYTES_PER_FLOAT_SAMPLE: u32 = 4;

struct MediaFoundationSession;

impl MediaFoundationSession {
    fn new() -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|error| format!("CoInitializeEx failed: {error}"))?;
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) {
                CoUninitialize();
                return Err(format!("MFStartup failed: {error}"));
            }
        }
        Ok(Self)
    }
}

impl Drop for MediaFoundationSession {
    fn drop(&mut self) {
        unsafe {
            if let Err(error) = MFShutdown() {
                tracing::error!(%error, "MFShutdown failed for AAC decoder");
            }
            CoUninitialize();
        }
    }
}

pub(super) struct WindowsAacDecoder {
    transform: IMFTransform,
    output_stream_info: MFT_OUTPUT_STREAM_INFO,
    channels: NonZeroU16,
    sample_rate: std::num::NonZeroU32,
    _media_foundation: MediaFoundationSession,
}

impl std::fmt::Debug for WindowsAacDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowsAacDecoder")
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

// SAFETY: the decoder owns its COM transform exclusively and all calls are
// serialized through `&mut self` on one decode worker.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for WindowsAacDecoder {}

impl WindowsAacDecoder {
    pub(super) fn new(config: AacDecoderConfig) -> Result<Self, String> {
        let AacDecoderConfig {
            audio_specific_config,
            channels,
            sample_rate,
        } = config;
        let media_foundation = MediaFoundationSession::new()?;

        unsafe {
            let transform: IMFTransform =
                CoCreateInstance(&CMSAACDecMFT, None, CLSCTX_INPROC_SERVER)
                    .map_err(|error| format!("create Media Foundation AAC decoder: {error}"))?;
            let input_type =
                create_aac_input_type(&audio_specific_config, channels.get(), sample_rate.get())?;

            transform
                .SetInputType(0, &input_type, 0)
                .map_err(|error| format!("set AAC input type: {error}"))?;
            // The transform rejects hand-built float PCM media types: its offered
            // type carries attributes like MF_MT_ALL_SAMPLES_INDEPENDENT that a
            // manual media type lacks. Negotiate the offered type instead.
            let (output_type, offered_channels, offered_sample_rate) =
                offered_float_output_type(&transform)?;
            if offered_channels != channels.get() || offered_sample_rate != sample_rate.get() {
                return Err(format!(
                    "Media Foundation AAC decoder offered {offered_channels}ch/{offered_sample_rate}Hz float PCM for a declared {}ch/{}Hz stream",
                    channels.get(),
                    sample_rate.get(),
                ));
            }
            transform
                .SetOutputType(0, &output_type, 0)
                .map_err(|error| format!("set AAC float output type: {error}"))?;
            let output_stream_info = transform
                .GetOutputStreamInfo(0)
                .map_err(|error| format!("query AAC output stream: {error}"))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|error| format!("begin AAC streaming: {error}"))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|error| format!("start AAC stream: {error}"))?;

            Ok(Self {
                transform,
                output_stream_info,
                channels,
                sample_rate,
                _media_foundation: media_foundation,
            })
        }
    }

    pub(super) fn decode(
        &mut self,
        packet: &EncodedAudioPacket,
    ) -> Result<Vec<DecodedAudioFrame>, PacketAudioError> {
        if packet.is_discontinuity() {
            self.reset();
        }
        let presentation_time = packet.presentation_time();
        unsafe {
            let input = create_sample(packet.data(), presentation_time, packet.declared_duration())
                .map_err(|message| decode_message(presentation_time, message))?;
            self.transform.ProcessInput(0, &input, 0).map_err(|error| {
                decode_message(presentation_time, format!("ProcessInput: {error}"))
            })?;
            // The AAC decoder holds back one access unit: it emits a unit only
            // once the next unit's timestamp bounds its duration. Draining after
            // every packet forces the queued unit out so each packet produces
            // its PCM frame.
            self.transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)
                .map_err(|error| {
                    decode_message(presentation_time, format!("drain AAC decoder: {error}"))
                })?;
        }
        let frames = self.collect_output(presentation_time);
        // Drop the decoder's lookahead state so the drained access unit is not
        // emitted a second time when the next packet arrives.
        self.reset();
        frames
    }

    pub(super) fn finish(&mut self) -> Result<Vec<DecodedAudioFrame>, PacketAudioError> {
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                .map_err(|error| {
                    decode_message(Duration::ZERO, format!("signal end-of-stream: {error}"))
                })?;
            self.transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)
                .map_err(|error| {
                    decode_message(Duration::ZERO, format!("start AAC drain: {error}"))
                })?;
        }
        self.collect_output(Duration::ZERO)
    }

    pub(super) fn reset(&mut self) {
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)
                .expect("Media Foundation AAC flush must succeed");
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .expect("Media Foundation AAC stream restart must succeed");
        }
    }

    fn collect_output(
        &mut self,
        submitted_time: Duration,
    ) -> Result<Vec<DecodedAudioFrame>, PacketAudioError> {
        let mut frames = Vec::new();
        loop {
            let output_sample = if self.output_stream_info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                    | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                != 0
            {
                None
            } else {
                Some(
                    create_empty_sample(self.output_stream_info.cbSize)
                        .map_err(|message| decode_message(submitted_time, message))?,
                )
            };
            let mut output = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(output_sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            };
            let mut status = 0_u32;
            let result = unsafe {
                self.transform
                    .ProcessOutput(0, std::slice::from_mut(&mut output), &raw mut status)
            };
            let sample = unsafe { ManuallyDrop::take(&mut output.pSample) };
            let events = unsafe { ManuallyDrop::take(&mut output.pEvents) };
            drop(events);

            match result {
                Ok(()) => {
                    let sample = sample.ok_or_else(|| {
                        decode_message(
                            submitted_time,
                            "Media Foundation reported AAC output without a sample",
                        )
                    })?;
                    frames.push(self.decode_output_sample(&sample, submitted_time)?);
                }
                Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.renegotiate_output(submitted_time)?;
                }
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                    return Ok(frames);
                }
                Err(error) => {
                    return Err(decode_message(
                        submitted_time,
                        format!("ProcessOutput: {error}"),
                    ));
                }
            }
        }
    }

    /// Handles a stream-change notification: the decoder re-offers the media
    /// types for the format it actually decoded, and decoding may continue only
    /// if that real format still matches the declared configuration.
    fn renegotiate_output(&mut self, submitted_time: Duration) -> Result<(), PacketAudioError> {
        let (output_type, actual_channels, actual_sample_rate) =
            offered_float_output_type(&self.transform)
                .map_err(|message| decode_message(submitted_time, message))?;
        if actual_channels != self.channels.get() || actual_sample_rate != self.sample_rate.get() {
            return Err(PacketAudioError::UnexpectedFormatChange {
                expected_channels: self.channels.get(),
                expected_sample_rate: self.sample_rate.get(),
                actual_channels,
                actual_sample_rate,
            });
        }
        unsafe {
            self.transform
                .SetOutputType(0, &output_type, 0)
                .map_err(|error| {
                    decode_message(
                        submitted_time,
                        format!("renegotiate AAC output type: {error}"),
                    )
                })?;
            self.output_stream_info = self.transform.GetOutputStreamInfo(0).map_err(|error| {
                decode_message(
                    submitted_time,
                    format!("query renegotiated AAC output stream: {error}"),
                )
            })?;
        }
        Ok(())
    }

    fn decode_output_sample(
        &self,
        sample: &IMFSample,
        submitted_time: Duration,
    ) -> Result<DecodedAudioFrame, PacketAudioError> {
        let presentation_time = unsafe { sample.GetSampleTime() }
            .map(duration_from_media_time)
            .map_err(|error| {
                decode_message(
                    submitted_time,
                    format!("AAC output has no timestamp: {error}"),
                )
            })?;
        let bytes = extract_sample_bytes(sample)
            .map_err(|message| decode_message(submitted_time, message))?;
        let (chunks, remainder) = bytes.as_chunks::<{ BYTES_PER_FLOAT_SAMPLE as usize }>();
        if !remainder.is_empty() {
            return Err(decode_message(
                submitted_time,
                format!(
                    "Media Foundation emitted {} bytes not aligned to float PCM",
                    bytes.len()
                ),
            ));
        }
        let samples = chunks
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect::<Vec<_>>();
        DecodedAudioFrame::from_interleaved_pcm(
            presentation_time,
            self.channels,
            self.sample_rate,
            samples,
        )
        .map_err(|error| pcm_error(submitted_time, &error))
    }
}

fn create_aac_input_type(
    audio_specific_config: &[u8],
    channels: u16,
    sample_rate: u32,
) -> Result<IMFMediaType, String> {
    let mut user_data =
        Vec::with_capacity(HE_AAC_USER_DATA_PREFIX_LEN + audio_specific_config.len());
    user_data.extend_from_slice(
        &u16::try_from(RAW_AAC_PAYLOAD)
            .expect("raw AAC payload fits u16")
            .to_le_bytes(),
    );
    user_data.extend_from_slice(
        &u16::try_from(UNSPECIFIED_AAC_PROFILE_LEVEL)
            .expect("AAC profile indication fits u16")
            .to_le_bytes(),
    );
    user_data.extend_from_slice(&0_u16.to_le_bytes());
    user_data.extend_from_slice(&0_u16.to_le_bytes());
    user_data.extend_from_slice(&0_u32.to_le_bytes());
    user_data.extend_from_slice(audio_specific_config);

    unsafe {
        let media_type = audio_media_type(MFAudioFormat_AAC, channels, sample_rate)?;
        media_type
            .SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, RAW_AAC_PAYLOAD)
            .map_err(|error| format!("set raw AAC payload type: {error}"))?;
        media_type
            .SetUINT32(
                &MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION,
                UNSPECIFIED_AAC_PROFILE_LEVEL,
            )
            .map_err(|error| format!("set AAC profile level: {error}"))?;
        media_type
            .SetBlob(&MF_MT_USER_DATA, &user_data)
            .map_err(|error| format!("set AAC AudioSpecificConfig: {error}"))?;
        Ok(media_type)
    }
}

/// Returns the first float-PCM media type the transform currently offers for
/// its output stream, together with the channel count and sample rate that
/// type carries. The offered set mirrors the declared input type before
/// decoding starts, and the stream's real format after a stream change.
fn offered_float_output_type(transform: &IMFTransform) -> Result<(IMFMediaType, u16, u32), String> {
    unsafe {
        for index in 0.. {
            let Ok(media_type) = transform.GetOutputAvailableType(0, index) else {
                break;
            };
            let subtype = media_type
                .GetGUID(&MF_MT_SUBTYPE)
                .map_err(|error| format!("read offered output subtype: {error}"))?;
            if subtype != MFAudioFormat_Float {
                continue;
            }
            let channels = media_type
                .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
                .map_err(|error| format!("read offered channel count: {error}"))?;
            let sample_rate = media_type
                .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                .map_err(|error| format!("read offered sample rate: {error}"))?;
            let channels = u16::try_from(channels)
                .map_err(|_| format!("offered channel count {channels} exceeds u16"))?;
            return Ok((media_type, channels, sample_rate));
        }
        Err(String::from(
            "Media Foundation AAC decoder offered no float PCM output type",
        ))
    }
}

fn audio_media_type(
    subtype: windows::core::GUID,
    channels: u16,
    sample_rate: u32,
) -> Result<IMFMediaType, String> {
    unsafe {
        let media_type =
            MFCreateMediaType().map_err(|error| format!("create audio media type: {error}"))?;
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|error| format!("set audio major type: {error}"))?;
        media_type
            .SetGUID(&MF_MT_SUBTYPE, &raw const subtype)
            .map_err(|error| format!("set audio subtype: {error}"))?;
        media_type
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, u32::from(channels))
            .map_err(|error| format!("set audio channel count: {error}"))?;
        media_type
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate)
            .map_err(|error| format!("set audio sample rate: {error}"))?;
        Ok(media_type)
    }
}

fn create_sample(
    data: &[u8],
    presentation_time: Duration,
    declared_duration: Duration,
) -> Result<IMFSample, String> {
    unsafe {
        let sample = MFCreateSample().map_err(|error| format!("create AAC sample: {error}"))?;
        let length = u32::try_from(data.len())
            .map_err(|_| String::from("AAC access unit exceeds Media Foundation u32 length"))?;
        let buffer = MFCreateMemoryBuffer(length)
            .map_err(|error| format!("create AAC input buffer: {error}"))?;
        let mut destination = ptr::null_mut();
        buffer
            .Lock(&raw mut destination, None, None)
            .map_err(|error| format!("lock AAC input buffer: {error}"))?;
        ptr::copy_nonoverlapping(data.as_ptr(), destination, data.len());
        let set_length = buffer.SetCurrentLength(length);
        let unlock = buffer.Unlock();
        set_length.map_err(|error| format!("set AAC input length: {error}"))?;
        unlock.map_err(|error| format!("unlock AAC input buffer: {error}"))?;
        sample
            .AddBuffer(&buffer)
            .map_err(|error| format!("attach AAC input buffer: {error}"))?;
        sample
            .SetSampleTime(media_time_from_duration(presentation_time)?)
            .map_err(|error| format!("set AAC sample timestamp: {error}"))?;
        sample
            .SetSampleDuration(media_time_from_duration(declared_duration)?)
            .map_err(|error| format!("set AAC sample duration: {error}"))?;
        Ok(sample)
    }
}

fn create_empty_sample(size: u32) -> Result<IMFSample, String> {
    unsafe {
        let sample =
            MFCreateSample().map_err(|error| format!("create AAC output sample: {error}"))?;
        let buffer = MFCreateMemoryBuffer(size)
            .map_err(|error| format!("create AAC output buffer: {error}"))?;
        sample
            .AddBuffer(&buffer)
            .map_err(|error| format!("attach AAC output buffer: {error}"))?;
        Ok(sample)
    }
}

fn extract_sample_bytes(sample: &IMFSample) -> Result<Vec<u8>, String> {
    unsafe {
        let buffer = sample
            .ConvertToContiguousBuffer()
            .map_err(|error| format!("coalesce AAC output buffers: {error}"))?;
        let mut source = ptr::null_mut();
        let mut length = 0_u32;
        buffer
            .Lock(&raw mut source, None, Some(&raw mut length))
            .map_err(|error| format!("lock AAC output buffer: {error}"))?;
        let mut bytes = vec![0_u8; length as usize];
        ptr::copy_nonoverlapping(source, bytes.as_mut_ptr(), bytes.len());
        buffer
            .Unlock()
            .map_err(|error| format!("unlock AAC output buffer: {error}"))?;
        Ok(bytes)
    }
}

fn media_time_from_duration(duration: Duration) -> Result<i64, String> {
    i64::try_from(duration.as_nanos() / 100)
        .map_err(|_| String::from("AAC timestamp exceeds Media Foundation i64 range"))
}

fn duration_from_media_time(time: i64) -> Duration {
    let time = u64::try_from(time).expect("Media Foundation AAC timestamp must be non-negative");
    Duration::from_nanos(time.saturating_mul(100))
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

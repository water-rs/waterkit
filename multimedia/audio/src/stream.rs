//! Packet-oriented audio decoding for segmented media playback.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(all(
    not(any(target_os = "android", target_os = "windows", target_arch = "wasm32")),
    feature = "he-aac"
))]
use symphonia_adapter_fdk_aac::AacDecoder as FdkAacDecoder;
#[cfg(all(
    not(any(target_os = "android", target_os = "windows")),
    any(not(feature = "he-aac"), target_arch = "wasm32")
))]
use symphonia_codec_aac::AacDecoder;
#[cfg(not(any(target_os = "android", target_os = "windows")))]
use symphonia_core::codecs::audio::well_known::CODEC_ID_AAC;
#[cfg(not(any(target_os = "android", target_os = "windows")))]
use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
#[cfg(not(any(target_os = "android", target_os = "windows")))]
use symphonia_core::packet::Packet;
#[cfg(not(any(target_os = "android", target_os = "windows")))]
use symphonia_core::units::{Duration as SymphoniaDuration, Timestamp};

#[cfg(target_os = "android")]
type SelectedAacDecoder = android::AndroidAacDecoder;
#[cfg(target_os = "windows")]
type SelectedAacDecoder = windows::WindowsAacDecoder;
#[cfg(not(any(target_os = "android", target_os = "windows")))]
type SelectedAacDecoder = Box<dyn AudioDecoder>;

/// Errors raised by packet-oriented audio decoding.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PacketAudioError {
    /// Decoder initialization failed.
    #[error("failed to initialize {codec} packet decoder: {message}")]
    Initialization {
        /// Codec name.
        codec: &'static str,
        /// Decoder error detail.
        message: String,
    },
    /// One encoded access unit could not be decoded.
    #[error("failed to decode {codec} audio packet at {presentation_time:?}: {message}")]
    Decode {
        /// Codec name.
        codec: &'static str,
        /// Packet presentation timestamp.
        presentation_time: Duration,
        /// Decoder error detail.
        message: String,
    },
    /// The decoded stream disagreed with its declared format.
    #[error(
        "decoded audio format changed from {expected_channels}ch/{expected_sample_rate}Hz to {actual_channels}ch/{actual_sample_rate}Hz without a new decoder configuration"
    )]
    UnexpectedFormatChange {
        /// Declared channel count.
        expected_channels: u16,
        /// Declared sample rate.
        expected_sample_rate: u32,
        /// Decoded channel count.
        actual_channels: u16,
        /// Decoded sample rate.
        actual_sample_rate: u32,
    },
    /// The decoded channel count is outside the public representation.
    #[error("decoded audio channel count {0} exceeds u16")]
    ChannelCountOverflow(usize),
}

/// Errors raised while constructing decoded PCM frames.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PcmFrameError {
    /// Interleaved samples do not contain a whole number of channel frames.
    #[error("PCM sample count {samples} is not aligned to {channels} interleaved channels")]
    ChannelMisalignment {
        /// Interleaved sample count.
        samples: usize,
        /// Channel count.
        channels: u16,
    },
}

/// Immutable AAC decoder configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AacDecoderConfig {
    audio_specific_config: Box<[u8]>,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
}

impl AacDecoderConfig {
    /// Creates an AAC decoder configuration from ISO/IEC 14496-3 `AudioSpecificConfig` bytes.
    #[must_use]
    pub fn new(
        audio_specific_config: impl Into<Box<[u8]>>,
        channels: NonZeroU16,
        sample_rate: NonZeroU32,
    ) -> Self {
        Self {
            audio_specific_config: audio_specific_config.into(),
            channels,
            sample_rate,
        }
    }

    /// Returns the `AudioSpecificConfig` bytes.
    #[must_use]
    pub fn audio_specific_config(&self) -> &[u8] {
        &self.audio_specific_config
    }

    /// Returns the declared channel count.
    #[must_use]
    pub const fn channels(&self) -> NonZeroU16 {
        self.channels
    }

    /// Returns the declared sample rate in hertz.
    #[must_use]
    pub const fn sample_rate(&self) -> NonZeroU32 {
        self.sample_rate
    }
}

/// One owned encoded audio access unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedAudioPacket {
    presentation_time: Duration,
    declared_duration: Duration,
    data: Box<[u8]>,
    discontinuity: bool,
}

impl EncodedAudioPacket {
    /// Creates an encoded packet.
    #[must_use]
    pub fn new(
        presentation_time: Duration,
        declared_duration: Duration,
        data: impl Into<Box<[u8]>>,
    ) -> Self {
        Self {
            presentation_time,
            declared_duration,
            data: data.into(),
            discontinuity: false,
        }
    }

    /// Marks the packet as the first access unit after a seek or stream discontinuity.
    #[must_use]
    pub const fn with_discontinuity(mut self) -> Self {
        self.discontinuity = true;
        self
    }

    /// Returns the packet presentation timestamp.
    #[must_use]
    pub const fn presentation_time(&self) -> Duration {
        self.presentation_time
    }

    /// Returns the container-declared packet duration.
    #[must_use]
    pub const fn declared_duration(&self) -> Duration {
        self.declared_duration
    }

    /// Returns the coded access-unit bytes.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Returns whether the decoder must reset before consuming this packet.
    #[must_use]
    pub const fn is_discontinuity(&self) -> bool {
        self.discontinuity
    }
}

/// One decoded interleaved floating-point PCM frame.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudioFrame {
    presentation_time: Duration,
    duration: Duration,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    samples: Box<[f32]>,
}

impl DecodedAudioFrame {
    /// Creates a decoded frame from normalized interleaved PCM.
    ///
    /// # Errors
    ///
    /// Returns an error when the sample count is not divisible by the channel count.
    pub fn from_interleaved_pcm(
        presentation_time: Duration,
        channels: NonZeroU16,
        sample_rate: NonZeroU32,
        samples: impl Into<Box<[f32]>>,
    ) -> Result<Self, PcmFrameError> {
        let samples = samples.into();
        if !samples.len().is_multiple_of(usize::from(channels.get())) {
            return Err(PcmFrameError::ChannelMisalignment {
                samples: samples.len(),
                channels: channels.get(),
            });
        }
        let frame_count = samples.len() / usize::from(channels.get());
        Ok(Self {
            presentation_time,
            duration: pcm_duration(frame_count, sample_rate),
            channels,
            sample_rate,
            samples,
        })
    }

    /// Returns the presentation timestamp of the first PCM sample.
    #[must_use]
    pub const fn presentation_time(&self) -> Duration {
        self.presentation_time
    }

    /// Returns the decoded PCM duration.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.duration
    }

    /// Returns the number of interleaved channels.
    #[must_use]
    pub const fn channels(&self) -> NonZeroU16 {
        self.channels
    }

    /// Returns the PCM sample rate in hertz.
    #[must_use]
    pub const fn sample_rate(&self) -> NonZeroU32 {
        self.sample_rate
    }

    /// Returns the interleaved normalized PCM samples.
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Consumes the frame and returns its interleaved normalized PCM samples.
    #[must_use]
    pub fn into_samples(self) -> Box<[f32]> {
        self.samples
    }

    /// Returns the number of samples per channel.
    #[must_use]
    pub fn sample_frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.get())
    }
}

/// Decoder for independently demuxed audio access units.
pub trait PacketAudioDecoder: Send {
    /// Submits one access unit and returns every PCM frame currently available.
    ///
    /// A platform decoder may buffer the access unit and return an empty vector until it has
    /// enough input to emit a frame.
    ///
    /// # Errors
    ///
    /// Returns an error when the access unit is malformed or changes format without a new
    /// decoder configuration.
    fn decode(
        &mut self,
        packet: EncodedAudioPacket,
    ) -> Result<Vec<DecodedAudioFrame>, PacketAudioError>;

    /// Signals end-of-stream and returns every delayed PCM frame.
    ///
    /// # Errors
    ///
    /// Returns an error when the decoder cannot drain its buffered access units.
    fn finish(&mut self) -> Result<Vec<DecodedAudioFrame>, PacketAudioError>;

    /// Resets prediction and overlap state after a seek or discontinuity.
    fn reset(&mut self);
}

/// Packet decoder for AAC access units.
///
/// Android and Windows use their platform AAC decoders. On other targets, the
/// `he-aac` feature selects the complete FDK decoder for AAC-LC, HE-AAC, and
/// HE-AAC v2. Without that feature, the smaller pure-Rust decoder supports
/// AAC-LC only.
pub struct AacPacketDecoder {
    decoder: SelectedAacDecoder,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
}

impl std::fmt::Debug for AacPacketDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AacPacketDecoder")
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

impl AacPacketDecoder {
    /// Creates an AAC packet decoder.
    ///
    /// # Errors
    ///
    /// Returns an error when the `AudioSpecificConfig` is malformed or describes an unsupported
    /// AAC profile or channel layout.
    pub fn new(config: AacDecoderConfig) -> Result<Self, PacketAudioError> {
        let channels = config.channels;
        let sample_rate = config.sample_rate;
        let decoder =
            create_aac_decoder(config).map_err(|message| PacketAudioError::Initialization {
                codec: "AAC",
                message,
            })?;

        Ok(Self {
            decoder,
            channels,
            sample_rate,
        })
    }
}

#[cfg(not(any(target_os = "android", target_os = "windows")))]
fn create_aac_decoder(config: AacDecoderConfig) -> Result<SelectedAacDecoder, String> {
    let mut parameters = AudioCodecParameters::new();
    parameters
        .for_codec(CODEC_ID_AAC)
        .with_sample_rate(config.sample_rate.get())
        .with_extra_data(config.audio_specific_config);
    create_symphonia_aac_decoder(&parameters).map_err(|error| error.to_string())
}

#[cfg(target_os = "android")]
fn create_aac_decoder(config: AacDecoderConfig) -> Result<SelectedAacDecoder, String> {
    android::AndroidAacDecoder::new(config)
}

#[cfg(target_os = "windows")]
fn create_aac_decoder(config: AacDecoderConfig) -> Result<SelectedAacDecoder, String> {
    windows::WindowsAacDecoder::new(config)
}

#[cfg(all(
    not(any(target_os = "android", target_os = "windows")),
    not(feature = "he-aac")
))]
fn create_symphonia_aac_decoder(
    parameters: &AudioCodecParameters,
) -> symphonia_core::errors::Result<SelectedAacDecoder> {
    AacDecoder::try_new(parameters, &AudioDecoderOptions::default())
        .map(|decoder| Box::new(decoder) as Box<dyn AudioDecoder>)
}

#[cfg(all(
    not(any(target_os = "android", target_os = "windows")),
    feature = "he-aac"
))]
fn create_symphonia_aac_decoder(
    parameters: &AudioCodecParameters,
) -> symphonia_core::errors::Result<SelectedAacDecoder> {
    use symphonia_core::codecs::registry::CodecRegistry;

    let mut codecs = CodecRegistry::new();
    codecs.register_audio_decoder::<FdkAacDecoder>();
    codecs.make_audio_decoder(parameters, &AudioDecoderOptions::default())
}

#[cfg(not(any(target_os = "android", target_os = "windows")))]
impl PacketAudioDecoder for AacPacketDecoder {
    fn decode(
        &mut self,
        packet: EncodedAudioPacket,
    ) -> Result<Vec<DecodedAudioFrame>, PacketAudioError> {
        if packet.discontinuity {
            self.decoder.reset();
        }

        let presentation_time = packet.presentation_time;
        let encoded = Packet::new(0, Timestamp::ZERO, SymphoniaDuration::ZERO, packet.data);
        let decoded = self
            .decoder
            .decode(&encoded)
            .map_err(|error| PacketAudioError::Decode {
                codec: "AAC",
                presentation_time,
                message: error.to_string(),
            })?;

        let actual_channel_count = decoded.spec().channels().count();
        let actual_channels = u16::try_from(actual_channel_count)
            .map_err(|_| PacketAudioError::ChannelCountOverflow(actual_channel_count))?;
        let actual_sample_rate = decoded.spec().rate();
        if actual_channels != self.channels.get() || actual_sample_rate != self.sample_rate.get() {
            return Err(PacketAudioError::UnexpectedFormatChange {
                expected_channels: self.channels.get(),
                expected_sample_rate: self.sample_rate.get(),
                actual_channels,
                actual_sample_rate,
            });
        }

        let frame_count = decoded.frames();
        let mut samples = Vec::with_capacity(decoded.samples_interleaved());
        decoded.copy_to_vec_interleaved::<f32>(&mut samples);

        Ok(vec![DecodedAudioFrame {
            presentation_time,
            duration: pcm_duration(frame_count, self.sample_rate),
            channels: self.channels,
            sample_rate: self.sample_rate,
            samples: samples.into_boxed_slice(),
        }])
    }

    fn finish(&mut self) -> Result<Vec<DecodedAudioFrame>, PacketAudioError> {
        Ok(Vec::new())
    }

    fn reset(&mut self) {
        self.decoder.reset();
    }
}

#[cfg(any(target_os = "android", target_os = "windows"))]
impl PacketAudioDecoder for AacPacketDecoder {
    fn decode(
        &mut self,
        packet: EncodedAudioPacket,
    ) -> Result<Vec<DecodedAudioFrame>, PacketAudioError> {
        self.decoder.decode(&packet)
    }

    fn finish(&mut self) -> Result<Vec<DecodedAudioFrame>, PacketAudioError> {
        self.decoder.finish()
    }

    fn reset(&mut self) {
        self.decoder.reset();
    }
}

fn pcm_duration(frame_count: usize, sample_rate: NonZeroU32) -> Duration {
    let frame_count = u64::try_from(frame_count).expect("PCM frame count must fit in u64");
    let sample_rate = u64::from(sample_rate.get());
    let seconds = frame_count / sample_rate;
    let nanoseconds = (frame_count % sample_rate) * 1_000_000_000 / sample_rate;
    Duration::new(
        seconds,
        u32::try_from(nanoseconds).expect("subsecond PCM duration must fit in u32"),
    )
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};
    use std::time::Duration;

    use super::{
        AacDecoderConfig, AacPacketDecoder, EncodedAudioPacket, PacketAudioDecoder,
        PacketAudioError,
    };

    const AAC_LC_44K_STEREO_PACKET: &str = include_str!("fixtures/aac-lc-44k-stereo.hex");

    fn decoder() -> AacPacketDecoder {
        AacPacketDecoder::new(AacDecoderConfig::new(
            [0x12, 0x10],
            NonZeroU16::new(2).expect("two channels must be non-zero"),
            NonZeroU32::new(44_100).expect("sample rate must be non-zero"),
        ))
        .expect("valid AAC-LC configuration must initialize")
    }

    #[test]
    fn decodes_real_aac_lc_access_unit_to_interleaved_pcm() {
        let packet =
            hex::decode(AAC_LC_44K_STEREO_PACKET.trim()).expect("fixture must be valid hex");
        let frames = decoder()
            .decode(EncodedAudioPacket::new(
                Duration::from_secs(7),
                Duration::from_millis(23),
                packet,
            ))
            .expect("AAC-LC access unit must decode");
        let frame = frames
            .first()
            .expect("software AAC decoder must emit one frame per access unit");

        assert_eq!(frame.presentation_time(), Duration::from_secs(7));
        assert_eq!(frame.channels().get(), 2);
        assert_eq!(frame.sample_rate().get(), 44_100);
        assert_eq!(frame.sample_frames(), 1024);
        assert_eq!(frame.samples().len(), 2048);
        assert!(frame.samples().iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn rejects_undeclared_format_changes() {
        let packet =
            hex::decode(AAC_LC_44K_STEREO_PACKET.trim()).expect("fixture must be valid hex");
        let mut decoder = AacPacketDecoder::new(AacDecoderConfig::new(
            [0x12, 0x10],
            NonZeroU16::new(1).expect("one channel must be non-zero"),
            NonZeroU32::new(44_100).expect("sample rate must be non-zero"),
        ))
        .expect("AAC decoder uses AudioSpecificConfig");

        let error = decoder
            .decode(EncodedAudioPacket::new(
                Duration::ZERO,
                Duration::from_millis(23),
                packet,
            ))
            .expect_err("declared mono must not accept decoded stereo");
        assert!(matches!(
            error,
            PacketAudioError::UnexpectedFormatChange { .. }
        ));
    }

    #[cfg(feature = "he-aac")]
    #[test]
    fn decodes_real_he_aac_access_unit_with_sbr() {
        let fixture = include_str!("fixtures/heaac-48k-stereo.hex");
        let (configuration, packet) = fixture
            .trim()
            .split_once('\n')
            .expect("HE-AAC fixture must contain configuration and packet lines");
        let configuration = hex::decode(configuration).expect("HE-AAC config must be valid hex");
        let packet = hex::decode(packet).expect("HE-AAC packet must be valid hex");
        let mut decoder = AacPacketDecoder::new(AacDecoderConfig::new(
            configuration,
            NonZeroU16::new(2).expect("two channels must be non-zero"),
            NonZeroU32::new(48_000).expect("sample rate must be non-zero"),
        ))
        .expect("valid HE-AAC configuration must initialize");

        let frames = decoder
            .decode(EncodedAudioPacket::new(
                Duration::ZERO,
                Duration::from_micros(42_667),
                packet,
            ))
            .expect("HE-AAC access unit must decode with SBR");
        let frame = frames
            .first()
            .expect("software HE-AAC decoder must emit one frame per access unit");

        assert_eq!(frame.channels().get(), 2);
        assert_eq!(frame.sample_rate().get(), 48_000);
        assert_eq!(frame.sample_frames(), 2_048);
        assert!(frame.samples().iter().all(|sample| sample.is_finite()));
    }
}

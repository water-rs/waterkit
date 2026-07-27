//! Android compressed-audio hardware offload.

use std::{
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use async_channel::{Receiver, Sender};
use futures::future::{Either, select};
use jni::{
    Env, jni_sig, jni_str,
    objects::{Global, JObject, JValue},
    signature::MethodSignature,
    strings::JNIStr,
};
use waterkit_video_container::{Codec, EncodedSample, TrackInfo, TrackKind};
use waterkit_video_core::Error;

use crate::android_surface::{
    AndroidPlaybackContext, android_api_level, jni_error, with_attached_env,
};

type GlobalObjectRef = Global<JObject<'static>>;

const ANDROID_OFFLOAD_MINIMUM_API: i32 = 29;
const AUDIO_TRACK_INITIALIZED: i32 = 1;
const AUDIO_TRACK_PLAYING: i32 = 3;
const AUDIO_TRACK_MODE_STREAM: i32 = 1;
const AUDIO_TRACK_WRITE_BLOCKING: i32 = 0;
const AUDIO_ATTRIBUTES_USAGE_MEDIA: i32 = 1;
const AUDIO_ATTRIBUTES_CONTENT_TYPE_MOVIE: i32 = 3;
const AUDIO_ATTRIBUTES_FLAG_HW_AV_SYNC: i32 = 16;
const OFFLOAD_PACKET_CAPACITY: usize = 8;

/// Cloneable control surface for one Android offloaded audio track.
#[derive(Clone)]
pub struct AndroidOffloadAudioController {
    shared: Arc<OffloadAudioShared>,
    controls: Sender<OffloadControl>,
}

impl std::fmt::Debug for AndroidOffloadAudioController {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidOffloadAudioController")
            .field("audio_session_id", &self.shared.audio_session_id)
            .finish_non_exhaustive()
    }
}

impl AndroidOffloadAudioController {
    /// Starts or resumes hardware-offloaded playback.
    ///
    /// # Errors
    ///
    /// Returns a platform error when Android rejects the state transition.
    pub fn play(&self) -> Result<(), Error> {
        self.shared
            .call_void(jni_str!("play"), "start offloaded AudioTrack")?;
        self.shared.playing.store(true, Ordering::Release);
        self.controls
            .try_send(OffloadControl::PlaybackStarted)
            .map_err(|error| Error::Platform(format!("wake offload writer after play: {error}")))
    }

    /// Pauses hardware-offloaded playback.
    ///
    /// # Errors
    ///
    /// Returns a platform error when Android rejects the state transition.
    pub fn pause(&self) -> Result<(), Error> {
        self.shared
            .call_void(jni_str!("pause"), "pause offloaded AudioTrack")?;
        self.shared.playing.store(false, Ordering::Release);
        Ok(())
    }

    /// Sets the platform output gain.
    ///
    /// # Errors
    ///
    /// Returns an error when `volume` is outside `0.0..=1.0` or Android rejects it.
    pub fn set_volume(&self, volume: f32) -> Result<(), Error> {
        if !(0.0..=1.0).contains(&volume) {
            return Err(Error::Platform(format!(
                "offloaded AudioTrack volume must be in 0.0..=1.0, got {volume}"
            )));
        }
        self.shared.with_env(|env, track| {
            let result = env
                .call_method(
                    track,
                    jni_str!("setVolume"),
                    jni_sig!("(F)I"),
                    &[JValue::Float(volume)],
                )
                .and_then(jni::objects::JValueOwned::i)
                .map_err(|error| jni_error(env, "set offloaded AudioTrack volume", error))?;
            if result != 0 {
                return Err(Error::Platform(format!(
                    "AudioTrack.setVolume returned error {result}"
                )));
            }
            Ok(())
        })
    }

    /// Returns the decoded-frame playback clock reported by `AudioTrack`.
    ///
    /// # Errors
    ///
    /// Returns a platform error when the clock cannot be queried.
    pub fn position(&self) -> Result<Duration, Error> {
        self.shared.position()
    }

    /// Returns the encoded duration queued ahead of the playback clock.
    ///
    /// # Errors
    ///
    /// Returns a platform error when the clock cannot be queried.
    pub fn buffered_duration(&self) -> Result<Duration, Error> {
        let position = self.position()?;
        let queued_end = Duration::from_nanos(self.shared.queued_end_nanos.load(Ordering::Acquire));
        Ok(queued_end.saturating_sub(position))
    }

    /// Requests offload end-of-stream after the final queued access unit.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer thread is no longer available.
    pub fn finish(&self) -> Result<(), Error> {
        self.controls
            .try_send(OffloadControl::Finish)
            .map_err(|error| Error::Platform(format!("finish offloaded AudioTrack: {error}")))
    }

    /// Returns the Android audio-session id shared with a tunneled video codec.
    #[must_use]
    pub fn audio_session_id(&self) -> i32 {
        self.shared.audio_session_id
    }
}

/// Owned compressed-audio offload pipeline.
pub struct AndroidOffloadAudioPlayback {
    track: TrackInfo,
    controller: AndroidOffloadAudioController,
    packets: Sender<OffloadPacket>,
    errors: Receiver<String>,
    generation: Arc<AtomicU64>,
    writer: Option<thread::JoinHandle<()>>,
}

impl std::fmt::Debug for AndroidOffloadAudioPlayback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidOffloadAudioPlayback")
            .field("track", &self.track)
            .field("audio_session_id", &self.controller.audio_session_id())
            .finish_non_exhaustive()
    }
}

impl AndroidOffloadAudioPlayback {
    /// Creates a required compressed-audio offload path.
    ///
    /// `hardware_av_sync` requests a hardware A/V clock and timestamped writes
    /// suitable for a tunneled video decoder.
    ///
    /// # Errors
    ///
    /// Returns an error for an ambiguous format, Android below API 29, or a
    /// route that does not advertise offload support.
    pub fn new(
        context: &AndroidPlaybackContext,
        track: TrackInfo,
        hardware_av_sync: bool,
    ) -> Result<Self, Error> {
        let layout = validate_audio_track(&track)?;
        let (retained_track, audio_session_id) = with_attached_env(&context.vm, |env| {
            let api_level = android_api_level(env)?;
            if api_level < ANDROID_OFFLOAD_MINIMUM_API {
                return Err(Error::Unsupported(format!(
                    "Android audio offload requires API {ANDROID_OFFLOAD_MINIMUM_API} or newer, got {api_level}"
                )));
            }
            let encoding = android_audio_encoding(env, &track)?;
            let format = create_audio_format(env, encoding, layout.sample_rate, layout.channels)?;
            let attributes = create_audio_attributes(env, hardware_av_sync)?;
            if !supports_offload(env, &format, &attributes)? {
                return Err(Error::Unsupported(format!(
                    "Android output route does not support hardware offload for {:?} {}ch/{}Hz",
                    track.codec(),
                    layout.channels,
                    layout.sample_rate
                )));
            }
            let audio_track = create_offload_audio_track(env, &format, &attributes)?;
            let audio_session_id = env
                .call_method(
                    &audio_track,
                    jni_str!("getAudioSessionId"),
                    jni_sig!("()I"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::i)
                .map_err(|error| jni_error(env, "read offloaded audio session id", error))?;
            if audio_session_id <= 0 {
                release_audio_track(env, &audio_track);
                return Err(Error::Platform(format!(
                    "Android returned invalid offload audio session id {audio_session_id}"
                )));
            }
            let retained_track = env.new_global_ref(&audio_track).map_err(|error| {
                release_audio_track(env, &audio_track);
                jni_error(env, "retain offloaded AudioTrack", error)
            })?;
            Ok((retained_track, audio_session_id))
        })?;
        let generation = Arc::new(AtomicU64::new(0));
        let shared = Arc::new(OffloadAudioShared {
            context: context.clone(),
            audio_track: retained_track,
            audio_session_id,
            sample_rate: layout.sample_rate,
            timeline_origin_nanos: AtomicU64::new(0),
            queued_end_nanos: AtomicU64::new(0),
            playing: AtomicBool::new(false),
        });
        let (packets, packet_receiver) = async_channel::bounded(OFFLOAD_PACKET_CAPACITY);
        let (controls, control_receiver) = async_channel::unbounded();
        let (error_sender, errors) = async_channel::bounded(1);
        let writer_shared = Arc::clone(&shared);
        let writer_generation = Arc::clone(&generation);
        let writer = thread::Builder::new()
            .name(String::from("waterkit-audio-offload"))
            .spawn(move || {
                run_offload_writer(
                    &writer_shared,
                    &writer_generation,
                    hardware_av_sync,
                    &packet_receiver,
                    &control_receiver,
                    &error_sender,
                );
            })
            .map_err(|error| Error::Platform(format!("spawn offload writer: {error}")))?;
        Ok(Self {
            track,
            controller: AndroidOffloadAudioController { shared, controls },
            packets,
            errors,
            generation,
            writer: Some(writer),
        })
    }

    /// Returns the compressed track accepted by this pipeline.
    #[must_use]
    pub const fn track_info(&self) -> &TrackInfo {
        &self.track
    }

    /// Returns a cloneable renderer-facing control surface.
    #[must_use]
    pub fn controller(&self) -> AndroidOffloadAudioController {
        self.controller.clone()
    }

    /// Queues one clear compressed access unit through a bounded handoff.
    ///
    /// # Errors
    ///
    /// Returns an error for the wrong track, encrypted data, timestamp
    /// overflow, or an offload-writer failure.
    pub fn queue(&self, sample: &EncodedSample) -> Result<(), Error> {
        self.check_writer()?;
        if sample.track_id() != self.track.id() {
            return Err(Error::Codec(format!(
                "offload audio track {} received track {}",
                self.track.id().get(),
                sample.track_id().get()
            )));
        }
        if sample.encryption().is_some() {
            return Err(Error::Unsupported(format!(
                "encrypted audio track {} requires a platform CDM offload path",
                self.track.id().get()
            )));
        }
        let presentation_time = sample.presentation_time().to_duration()?;
        let duration = sample.duration().to_duration()?;
        let end = presentation_time.checked_add(duration).ok_or_else(|| {
            Error::Codec(String::from("offload audio sample end timestamp overflow"))
        })?;
        self.controller
            .shared
            .queued_end_nanos
            .fetch_max(duration_nanos(end)?, Ordering::AcqRel);
        self.packets
            .send_blocking(OffloadPacket {
                generation: self.generation.load(Ordering::Acquire),
                presentation_time,
                data: sample.data().to_vec(),
            })
            .map_err(|error| Error::Platform(format!("queue offload audio sample: {error}")))?;
        self.check_writer()
    }

    /// Flushes packets and restarts the offload clock at `position`.
    ///
    /// # Errors
    ///
    /// Returns a platform error when Android rejects pause/flush.
    pub fn flush(&self, position: Duration) -> Result<(), Error> {
        if self.controller.shared.playing.swap(false, Ordering::AcqRel) {
            self.controller.pause()?;
        }
        self.controller
            .shared
            .call_void(jni_str!("flush"), "flush offloaded AudioTrack")?;
        let position_nanos = duration_nanos(position)?;
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.controller
            .shared
            .timeline_origin_nanos
            .store(position_nanos, Ordering::Release);
        self.controller
            .shared
            .queued_end_nanos
            .store(position_nanos, Ordering::Release);
        self.controller
            .controls
            .send_blocking(OffloadControl::Reset { generation })
            .map_err(|error| Error::Platform(format!("reset offload writer: {error}")))
    }

    /// Returns the audio-session id used by Android tunneling.
    #[must_use]
    pub fn audio_session_id(&self) -> i32 {
        self.controller.audio_session_id()
    }

    fn check_writer(&self) -> Result<(), Error> {
        match self.errors.try_recv() {
            Ok(message) => Err(Error::Platform(message)),
            Err(async_channel::TryRecvError::Empty) => Ok(()),
            Err(async_channel::TryRecvError::Closed) if self.writer.is_none() => Ok(()),
            Err(async_channel::TryRecvError::Closed) => Err(Error::Platform(String::from(
                "Android offload writer exited without reporting its result",
            ))),
        }
    }
}

impl Drop for AndroidOffloadAudioPlayback {
    fn drop(&mut self) {
        let _ = self
            .controller
            .controls
            .send_blocking(OffloadControl::Shutdown);
        if let Some(writer) = self.writer.take()
            && writer.join().is_err()
        {
            tracing::error!("Android offload writer panicked during shutdown");
        }
    }
}

struct OffloadAudioShared {
    context: AndroidPlaybackContext,
    audio_track: GlobalObjectRef,
    audio_session_id: i32,
    sample_rate: NonZeroU32,
    timeline_origin_nanos: AtomicU64,
    queued_end_nanos: AtomicU64,
    playing: AtomicBool,
}

impl OffloadAudioShared {
    fn with_env<T>(
        &self,
        operation: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        with_attached_env(&self.context.vm, |env| {
            operation(env, self.audio_track.as_obj())
        })
    }

    fn call_void(&self, method: &JNIStr, operation: &str) -> Result<(), Error> {
        self.with_env(|env, track| {
            env.call_method(track, method, jni_sig!("()V"), &[])
                .map_err(|error| jni_error(env, operation, error))?;
            Ok(())
        })
    }

    fn position(&self) -> Result<Duration, Error> {
        let frames = self.with_env(|env, track| {
            let timestamp = env
                .new_object(
                    jni_str!("android/media/AudioTimestamp"),
                    jni_sig!("()V"),
                    &[],
                )
                .map_err(|error| jni_error(env, "create offload AudioTimestamp", error))?;
            let available = env
                .call_method(
                    track,
                    jni_str!("getTimestamp"),
                    jni_sig!("(Landroid/media/AudioTimestamp;)Z"),
                    &[JValue::Object(&timestamp)],
                )
                .and_then(jni::objects::JValueOwned::z)
                .map_err(|error| jni_error(env, "read offload AudioTimestamp", error))?;
            if available {
                let frames = env
                    .get_field(&timestamp, jni_str!("framePosition"), jni_sig!("J"))
                    .and_then(jni::objects::JValueOwned::j)
                    .map_err(|error| jni_error(env, "read offload frame position", error))?;
                return u64::try_from(frames).map_err(|_| {
                    Error::Platform(format!(
                        "AudioTimestamp returned negative frame position {frames}"
                    ))
                });
            }
            let head = env
                .call_method(
                    track,
                    jni_str!("getPlaybackHeadPosition"),
                    jni_sig!("()I"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::i)
                .map_err(|error| jni_error(env, "read offload playback head", error))?;
            Ok(u64::from(u32::from_ne_bytes(head.to_ne_bytes())))
        })?;
        let elapsed_nanos = u128::from(frames)
            .checked_mul(1_000_000_000)
            .ok_or_else(|| Error::Platform(String::from("offload frame clock overflow")))?
            / u128::from(self.sample_rate.get());
        let elapsed_nanos = u64::try_from(elapsed_nanos)
            .map_err(|_| Error::Platform(String::from("offload clock exceeds u64 nanoseconds")))?;
        let position = self
            .timeline_origin_nanos
            .load(Ordering::Acquire)
            .checked_add(elapsed_nanos)
            .ok_or_else(|| Error::Platform(String::from("offload timeline position overflow")))?;
        Ok(Duration::from_nanos(position))
    }
}

impl Drop for OffloadAudioShared {
    fn drop(&mut self) {
        if let Err(error) = with_attached_env(&self.context.vm, |env| {
            release_audio_track(env, self.audio_track.as_obj());
            Ok(())
        }) {
            tracing::error!(%error, "failed to attach JVM while releasing offloaded AudioTrack");
        }
    }
}

struct OffloadPacket {
    generation: u64,
    presentation_time: Duration,
    data: Vec<u8>,
}

enum OffloadControl {
    PlaybackStarted,
    Reset { generation: u64 },
    Finish,
    Shutdown,
}

fn run_offload_writer(
    shared: &OffloadAudioShared,
    generation: &AtomicU64,
    hardware_av_sync: bool,
    packets: &Receiver<OffloadPacket>,
    controls: &Receiver<OffloadControl>,
    errors: &Sender<String>,
) {
    let result = offload_writer_loop(shared, generation, hardware_av_sync, packets, controls);
    if let Err(error) = result {
        let _ = errors.send_blocking(error.to_string());
    }
}

fn offload_writer_loop(
    shared: &OffloadAudioShared,
    generation: &AtomicU64,
    hardware_av_sync: bool,
    packets: &Receiver<OffloadPacket>,
    controls: &Receiver<OffloadControl>,
) -> Result<(), Error> {
    let mut finish_pending = false;
    let mut finished = false;
    loop {
        let control = controls.recv();
        let packet = packets.recv();
        futures::pin_mut!(control, packet);
        match futures::executor::block_on(select(control, packet)) {
            Either::Left((Ok(OffloadControl::Shutdown) | Err(_), _))
            | Either::Right((Err(_), _)) => return Ok(()),
            Either::Left((Ok(OffloadControl::PlaybackStarted), _)) => {
                if finish_pending && !finished {
                    set_offload_end_of_stream(shared)?;
                    finish_pending = false;
                    finished = true;
                }
            }
            Either::Left((Ok(OffloadControl::Reset { generation: reset }), _)) => {
                while packets.try_recv().is_ok() {}
                if generation.load(Ordering::Acquire) != reset {
                    return Err(Error::Platform(String::from(
                        "offload writer observed an out-of-order reset generation",
                    )));
                }
                finish_pending = false;
                finished = false;
            }
            Either::Left((Ok(OffloadControl::Finish), _)) => {
                if finished || finish_pending {
                    continue;
                }
                drain_offload_packets(shared, generation, hardware_av_sync, packets)?;
                if shared.playing.load(Ordering::Acquire) {
                    set_offload_end_of_stream(shared)?;
                    finished = true;
                } else {
                    finish_pending = true;
                }
            }
            Either::Right((Ok(packet), _)) => {
                if packet.generation == generation.load(Ordering::Acquire) {
                    write_offload_packet(shared, &packet, hardware_av_sync)?;
                }
            }
        }
    }
}

fn drain_offload_packets(
    shared: &OffloadAudioShared,
    generation: &AtomicU64,
    hardware_av_sync: bool,
    packets: &Receiver<OffloadPacket>,
) -> Result<(), Error> {
    while let Ok(packet) = packets.try_recv() {
        if packet.generation == generation.load(Ordering::Acquire) {
            write_offload_packet(shared, &packet, hardware_av_sync)?;
        }
    }
    Ok(())
}

fn write_offload_packet(
    shared: &OffloadAudioShared,
    packet: &OffloadPacket,
    hardware_av_sync: bool,
) -> Result<(), Error> {
    shared.with_env(|env, track| {
        let data = env
            .byte_array_from_slice(&packet.data)
            .map_err(|error| jni_error(env, "create offload access unit", error))?;
        let buffer = env
            .call_static_method(
                jni_str!("java/nio/ByteBuffer"),
                jni_str!("wrap"),
                jni_sig!("([B)Ljava/nio/ByteBuffer;"),
                &[JValue::Object(&data)],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|error| jni_error(env, "wrap offload access unit", error))?;
        let size = i32::try_from(packet.data.len())
            .map_err(|_| Error::Codec(String::from("offload access unit exceeds Android jint")))?;
        let result = if hardware_av_sync {
            let timestamp = i64::try_from(packet.presentation_time.as_nanos()).map_err(|_| {
                Error::Codec(String::from("offload timestamp exceeds Android jlong"))
            })?;
            env.call_method(
                track,
                jni_str!("write"),
                jni_sig!("(Ljava/nio/ByteBuffer;IIJ)I"),
                &[
                    JValue::Object(&buffer),
                    JValue::Int(size),
                    JValue::Int(AUDIO_TRACK_WRITE_BLOCKING),
                    JValue::Long(timestamp),
                ],
            )
        } else {
            env.call_method(
                track,
                jni_str!("write"),
                jni_sig!("(Ljava/nio/ByteBuffer;II)I"),
                &[
                    JValue::Object(&buffer),
                    JValue::Int(size),
                    JValue::Int(AUDIO_TRACK_WRITE_BLOCKING),
                ],
            )
        };
        let written = result
            .and_then(jni::objects::JValueOwned::i)
            .map_err(|error| jni_error(env, "write offload access unit", error))?;
        if written != size {
            return Err(Error::Platform(format!(
                "offloaded AudioTrack wrote {written} of {size} access-unit bytes"
            )));
        }
        Ok(())
    })
}

fn set_offload_end_of_stream(shared: &OffloadAudioShared) -> Result<(), Error> {
    shared.with_env(|env, track| {
        let play_state = env
            .call_method(track, jni_str!("getPlayState"), jni_sig!("()I"), &[])
            .and_then(jni::objects::JValueOwned::i)
            .map_err(|error| jni_error(env, "read offload play state", error))?;
        if play_state != AUDIO_TRACK_PLAYING {
            return Err(Error::Platform(format!(
                "offload end-of-stream requires PLAYSTATE_PLAYING, got {play_state}"
            )));
        }
        env.call_method(
            track,
            jni_str!("setOffloadEndOfStream"),
            jni_sig!("()V"),
            &[],
        )
        .map_err(|error| jni_error(env, "set offload end-of-stream", error))?;
        Ok(())
    })
}

fn validate_audio_track(track: &TrackInfo) -> Result<waterkit_video_container::AudioLayout, Error> {
    if track.kind() != TrackKind::Audio {
        return Err(Error::Codec(format!(
            "track {} is not audio and cannot use AudioTrack offload",
            track.id().get()
        )));
    }
    if track.protection().is_some() {
        return Err(Error::Unsupported(format!(
            "protected audio track {} requires a CDM-aware offload path",
            track.id().get()
        )));
    }
    track.audio_layout().ok_or_else(|| {
        Error::Container(format!(
            "offload audio track {} has no channel or sample-rate layout",
            track.id().get()
        ))
    })
}

fn android_audio_encoding(env: &mut Env<'_>, track: &TrackInfo) -> Result<i32, Error> {
    let (field, field_label) = match track.codec() {
        Codec::Aac => aac_encoding_field(track.decoder_configuration())?,
        Codec::Ac3 => (jni_str!("ENCODING_AC3"), "ENCODING_AC3"),
        Codec::Eac3 => (jni_str!("ENCODING_E_AC3"), "ENCODING_E_AC3"),
        Codec::Ac4 => (jni_str!("ENCODING_AC4"), "ENCODING_AC4"),
        Codec::Opus => (jni_str!("ENCODING_OPUS"), "ENCODING_OPUS"),
        Codec::Flac => (jni_str!("ENCODING_FLAC"), "ENCODING_FLAC"),
        codec => {
            return Err(Error::Unsupported(format!(
                "Android offload requires an unambiguous AudioFormat encoding; {codec:?} is not mapped"
            )));
        }
    };
    env.get_static_field(jni_str!("android/media/AudioFormat"), field, jni_sig!("I"))
        .and_then(jni::objects::JValueOwned::i)
        .map_err(|error| jni_error(env, &format!("read AudioFormat.{field_label}"), error))
}

fn aac_encoding_field(config: &[u8]) -> Result<(&'static JNIStr, &'static str), Error> {
    match aac_audio_object_type(config)? {
        1 => Ok((jni_str!("ENCODING_AAC_MAIN"), "ENCODING_AAC_MAIN")),
        2 => Ok((jni_str!("ENCODING_AAC_LC"), "ENCODING_AAC_LC")),
        3 => Ok((jni_str!("ENCODING_AAC_SSR"), "ENCODING_AAC_SSR")),
        4 => Ok((jni_str!("ENCODING_AAC_LTP"), "ENCODING_AAC_LTP")),
        5 => Ok((jni_str!("ENCODING_AAC_HE_V1"), "ENCODING_AAC_HE_V1")),
        29 => Ok((jni_str!("ENCODING_AAC_HE_V2"), "ENCODING_AAC_HE_V2")),
        39 => Ok((jni_str!("ENCODING_AAC_ELD"), "ENCODING_AAC_ELD")),
        42 => Ok((jni_str!("ENCODING_AAC_XHE"), "ENCODING_AAC_XHE")),
        other => Err(Error::Unsupported(format!(
            "Android offload has no AudioFormat encoding for AAC object type {other}"
        ))),
    }
}

fn aac_audio_object_type(config: &[u8]) -> Result<u8, Error> {
    let first = *config.first().ok_or_else(|| {
        Error::Container(String::from("AAC offload track has no AudioSpecificConfig"))
    })?;
    let base = first >> 3;
    if base != 31 {
        return Ok(base);
    }
    let second = *config.get(1).ok_or_else(|| {
        Error::Container(String::from(
            "extended AAC AudioSpecificConfig is truncated",
        ))
    })?;
    Ok(32 + (((first & 0x07) << 3) | (second >> 5)))
}

fn create_audio_format<'local>(
    env: &mut Env<'local>,
    encoding: i32,
    sample_rate: NonZeroU32,
    channels: NonZeroU32,
) -> Result<JObject<'local>, Error> {
    if channels.get() > 30 {
        return Err(Error::Unsupported(format!(
            "Android channel-index masks support at most 30 channels, got {channels}"
        )));
    }
    let sample_rate = i32::try_from(sample_rate.get())
        .map_err(|_| Error::Unsupported(String::from("audio sample rate exceeds Android jint")))?;
    let index_mask = i32::try_from((1_u32 << channels.get()) - 1)
        .expect("validated channel-index mask must fit Android jint");
    let builder = env
        .new_object(
            jni_str!("android/media/AudioFormat$Builder"),
            jni_sig!("()V"),
            &[],
        )
        .map_err(|error| jni_error(env, "create AudioFormat.Builder", error))?;
    call_builder_int(
        env,
        &builder,
        "AudioFormat",
        jni_str!("setEncoding"),
        jni_sig!("(I)Landroid/media/AudioFormat$Builder;"),
        encoding,
    )?;
    call_builder_int(
        env,
        &builder,
        "AudioFormat",
        jni_str!("setSampleRate"),
        jni_sig!("(I)Landroid/media/AudioFormat$Builder;"),
        sample_rate,
    )?;
    call_builder_int(
        env,
        &builder,
        "AudioFormat",
        jni_str!("setChannelIndexMask"),
        jni_sig!("(I)Landroid/media/AudioFormat$Builder;"),
        index_mask,
    )?;
    env.call_method(
        &builder,
        jni_str!("build"),
        jni_sig!("()Landroid/media/AudioFormat;"),
        &[],
    )
    .and_then(jni::objects::JValueOwned::l)
    .map_err(|error| jni_error(env, "build AudioFormat", error))
}

fn create_audio_attributes<'local>(
    env: &mut Env<'local>,
    hardware_av_sync: bool,
) -> Result<JObject<'local>, Error> {
    let builder = env
        .new_object(
            jni_str!("android/media/AudioAttributes$Builder"),
            jni_sig!("()V"),
            &[],
        )
        .map_err(|error| jni_error(env, "create AudioAttributes.Builder", error))?;
    call_builder_int(
        env,
        &builder,
        "AudioAttributes",
        jni_str!("setUsage"),
        jni_sig!("(I)Landroid/media/AudioAttributes$Builder;"),
        AUDIO_ATTRIBUTES_USAGE_MEDIA,
    )?;
    call_builder_int(
        env,
        &builder,
        "AudioAttributes",
        jni_str!("setContentType"),
        jni_sig!("(I)Landroid/media/AudioAttributes$Builder;"),
        AUDIO_ATTRIBUTES_CONTENT_TYPE_MOVIE,
    )?;
    if hardware_av_sync {
        call_builder_int(
            env,
            &builder,
            "AudioAttributes",
            jni_str!("setFlags"),
            jni_sig!("(I)Landroid/media/AudioAttributes$Builder;"),
            AUDIO_ATTRIBUTES_FLAG_HW_AV_SYNC,
        )?;
    }
    env.call_method(
        &builder,
        jni_str!("build"),
        jni_sig!("()Landroid/media/AudioAttributes;"),
        &[],
    )
    .and_then(jni::objects::JValueOwned::l)
    .map_err(|error| jni_error(env, "build AudioAttributes", error))
}

fn supports_offload(
    env: &mut Env<'_>,
    format: &JObject<'_>,
    attributes: &JObject<'_>,
) -> Result<bool, Error> {
    env.call_static_method(
        jni_str!("android/media/AudioManager"),
        jni_str!("isOffloadedPlaybackSupported"),
        jni_sig!("(Landroid/media/AudioFormat;Landroid/media/AudioAttributes;)Z"),
        &[JValue::Object(format), JValue::Object(attributes)],
    )
    .and_then(jni::objects::JValueOwned::z)
    .map_err(|error| jni_error(env, "query Android audio offload support", error))
}

fn create_offload_audio_track<'local>(
    env: &mut Env<'local>,
    format: &JObject<'_>,
    attributes: &JObject<'_>,
) -> Result<JObject<'local>, Error> {
    let builder = env
        .new_object(
            jni_str!("android/media/AudioTrack$Builder"),
            jni_sig!("()V"),
            &[],
        )
        .map_err(|error| jni_error(env, "create AudioTrack.Builder", error))?;
    call_audio_track_builder_object(
        env,
        &builder,
        jni_str!("setAudioFormat"),
        jni_sig!("(Landroid/media/AudioFormat;)Landroid/media/AudioTrack$Builder;"),
        format,
    )?;
    call_audio_track_builder_object(
        env,
        &builder,
        jni_str!("setAudioAttributes"),
        jni_sig!("(Landroid/media/AudioAttributes;)Landroid/media/AudioTrack$Builder;"),
        attributes,
    )?;
    call_builder_int(
        env,
        &builder,
        "AudioTrack",
        jni_str!("setTransferMode"),
        jni_sig!("(I)Landroid/media/AudioTrack$Builder;"),
        AUDIO_TRACK_MODE_STREAM,
    )?;
    env.call_method(
        &builder,
        jni_str!("setOffloadedPlayback"),
        jni_sig!("(Z)Landroid/media/AudioTrack$Builder;"),
        &[JValue::Bool(true)],
    )
    .map_err(|error| jni_error(env, "require AudioTrack offload", error))?;
    let track = env
        .call_method(
            &builder,
            jni_str!("build"),
            jni_sig!("()Landroid/media/AudioTrack;"),
            &[],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| jni_error(env, "build offloaded AudioTrack", error))?;
    let state = env
        .call_method(&track, jni_str!("getState"), jni_sig!("()I"), &[])
        .and_then(jni::objects::JValueOwned::i)
        .map_err(|error| jni_error(env, "read offloaded AudioTrack state", error))?;
    if state != AUDIO_TRACK_INITIALIZED {
        release_audio_track(env, &track);
        return Err(Error::Platform(format!(
            "offloaded AudioTrack initialized in state {state}"
        )));
    }
    Ok(track)
}

fn call_builder_int(
    env: &mut Env<'_>,
    builder: &JObject<'_>,
    class: &str,
    method: &JNIStr,
    signature: MethodSignature<'_, '_>,
    value: i32,
) -> Result<(), Error> {
    env.call_method(builder, method, signature, &[JValue::Int(value)])
        .map_err(|error| jni_error(env, &format!("{class} builder method"), error))?;
    Ok(())
}

fn call_audio_track_builder_object(
    env: &mut Env<'_>,
    builder: &JObject<'_>,
    method: &JNIStr,
    signature: MethodSignature<'_, '_>,
    value: &JObject<'_>,
) -> Result<(), Error> {
    env.call_method(builder, method, signature, &[JValue::Object(value)])
        .map_err(|error| jni_error(env, "AudioTrack builder method", error))?;
    Ok(())
}

fn release_audio_track(env: &mut Env<'_>, track: &JObject<'_>) {
    if let Err(error) = env.call_method(track, jni_str!("release"), jni_sig!("()V"), &[]) {
        tracing::error!(%error, "failed to release Android offloaded AudioTrack");
        let _ = env.exception_clear();
    }
}

fn duration_nanos(duration: Duration) -> Result<u64, Error> {
    u64::try_from(duration.as_nanos())
        .map_err(|_| Error::Codec(String::from("media duration exceeds u64 nanoseconds")))
}

#[cfg(test)]
mod tests {
    use super::{aac_audio_object_type, aac_encoding_field};

    #[test]
    fn parses_standard_and_extended_aac_object_types() {
        assert_eq!(aac_audio_object_type(&[0x12, 0x10]).unwrap(), 2);
        assert_eq!(aac_audio_object_type(&[0xf9, 0x48]).unwrap(), 42);
    }

    #[test]
    fn maps_supported_aac_profiles_without_guessing() {
        assert_eq!(
            aac_encoding_field(&[0x12, 0x10]).unwrap(),
            "ENCODING_AAC_LC"
        );
        assert!(aac_encoding_field(&[0x08, 0x00]).is_ok());
        assert!(aac_encoding_field(&[0x30, 0x00]).is_err());
    }
}

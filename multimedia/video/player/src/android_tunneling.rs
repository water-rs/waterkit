//! Android hardware A/V tunneling through `AudioTrack` and `MediaCodec`.

use std::time::Duration;

use jni::{
    JNIEnv,
    objects::{GlobalRef, JObject, JValue},
};
use waterkit_codec::NalStreamConverter;
use waterkit_video_container::{Codec, EncodedSample, TrackInfo, TrackKind};
use waterkit_video_core::Error;

use crate::{
    AndroidOffloadAudioController, AndroidOffloadAudioPlayback, AndroidVideoSurface,
    android_surface::jni_error,
};

const MEDIA_CODEC_LIST_ALL_CODECS: i32 = 1;
const MEDIA_CODEC_INFO_TRY_AGAIN_LATER: i32 = -1;
const MEDIA_CODEC_INFO_OUTPUT_FORMAT_CHANGED: i32 = -2;
const MEDIA_CODEC_INFO_OUTPUT_BUFFERS_CHANGED: i32 = -3;
const MEDIA_CODEC_BUFFER_FLAG_END_OF_STREAM: i32 = 4;
const INPUT_WAIT_FOREVER_MICROS: i64 = -1;

/// Required Android hardware A/V tunnel.
///
/// Encoded audio remains on an offloaded `AudioTrack`. Its session id is
/// installed into a tunneled video decoder whose output stays on the retained
/// Android `Surface`; video pixels never enter the application GPU pipeline.
pub struct AndroidTunneledPlayback {
    audio: AndroidOffloadAudioPlayback,
    video: AndroidTunneledVideoDecoder,
}

impl std::fmt::Debug for AndroidTunneledPlayback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidTunneledPlayback")
            .field("audio_track", self.audio.track_info())
            .field("video_track", self.video.track_info())
            .field("audio_session_id", &self.audio.audio_session_id())
            .finish_non_exhaustive()
    }
}

impl AndroidTunneledPlayback {
    /// Creates a required clear-content hardware A/V tunnel.
    ///
    /// # Errors
    ///
    /// Returns an error unless audio offload and a tunneled decoder are both
    /// supported for the exact selected formats and current output route.
    pub fn new(
        surface: AndroidVideoSurface,
        video_track: TrackInfo,
        audio_track: TrackInfo,
    ) -> Result<Self, Error> {
        let audio = AndroidOffloadAudioPlayback::new(surface.context(), audio_track, true)?;
        let video =
            AndroidTunneledVideoDecoder::new(surface, video_track, audio.audio_session_id())?;
        Ok(Self { audio, video })
    }

    /// Returns a renderer-facing audio transport and clock controller.
    #[must_use]
    pub fn audio_controller(&self) -> AndroidOffloadAudioController {
        self.audio.controller()
    }

    /// Returns the tunneled video track.
    #[must_use]
    pub const fn video_track_info(&self) -> &TrackInfo {
        self.video.track_info()
    }

    /// Returns the offloaded audio track.
    #[must_use]
    pub const fn audio_track_info(&self) -> &TrackInfo {
        self.audio.track_info()
    }

    /// Queues one selected audio or video sample.
    ///
    /// # Errors
    ///
    /// Returns an error for a foreign track or a platform queue failure.
    pub fn queue(&mut self, sample: &EncodedSample) -> Result<Vec<Duration>, Error> {
        if sample.track_id() == self.audio.track_info().id() {
            self.audio.queue(sample)?;
            return Ok(Vec::new());
        }
        if sample.track_id() == self.video.track_info().id() {
            return self.video.queue(sample);
        }
        Err(Error::Codec(format!(
            "Android A/V tunnel received unrelated track {}",
            sample.track_id().get()
        )))
    }

    /// Flushes both sides of the tunnel after seek or discontinuity.
    ///
    /// # Errors
    ///
    /// Returns a platform error when either endpoint rejects the flush.
    pub fn flush(&mut self, position: Duration) -> Result<(), Error> {
        self.audio.flush(position)?;
        self.video.flush()
    }

    /// Signals end-of-stream to both tunneled endpoints.
    ///
    /// # Errors
    ///
    /// Returns a platform error when either endpoint rejects EOS.
    pub fn finish_input(&mut self) -> Result<Vec<Duration>, Error> {
        self.audio.controller().finish()?;
        self.video.finish_input()
    }
}

struct AndroidTunneledVideoDecoder {
    surface: AndroidVideoSurface,
    codec: GlobalRef,
    track: TrackInfo,
    converter: Option<NalStreamConverter>,
    input_finished: bool,
}

impl std::fmt::Debug for AndroidTunneledVideoDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidTunneledVideoDecoder")
            .field("track", &self.track)
            .field("input_finished", &self.input_finished)
            .finish_non_exhaustive()
    }
}

impl AndroidTunneledVideoDecoder {
    fn new(
        surface: AndroidVideoSurface,
        track: TrackInfo,
        audio_session_id: i32,
    ) -> Result<Self, Error> {
        validate_video_track(&track)?;
        let dimensions = track
            .video_dimensions()
            .expect("validated tunneled video track must retain dimensions");
        let width = i32::try_from(dimensions.width.get())
            .map_err(|_| Error::Codec(String::from("tunneled video width exceeds Android jint")))?;
        let height = i32::try_from(dimensions.height.get()).map_err(|_| {
            Error::Codec(String::from("tunneled video height exceeds Android jint"))
        })?;
        let (mime, mut converter) = video_codec_description(&track)?;
        let (primary_csd, secondary_csd) = converter.as_ref().map_or_else(
            || (track.decoder_configuration().to_vec(), Vec::new()),
            |converter| {
                let (primary, secondary) = converter.codec_specific_data();
                (
                    primary.map_or_else(Vec::new, <[u8]>::to_vec),
                    secondary.map_or_else(Vec::new, <[u8]>::to_vec),
                )
            },
        );
        let mut env = surface
            .context
            .vm
            .attach_current_thread()
            .map_err(|error| Error::Platform(format!("attach tunnel setup to JVM: {error}")))?;
        let format = create_tunneled_video_format(
            &mut env,
            mime,
            width,
            height,
            audio_session_id,
            &primary_csd,
            &secondary_csd,
        )?;
        let codec = create_tunneled_media_codec(&mut env, mime, &format, surface.surface.as_obj())?;
        let retained_codec = env.new_global_ref(&codec).map_err(|error| {
            release_codec(&mut env, &codec, false);
            jni_error(&mut env, "retain tunneled MediaCodec", error)
        })?;
        drop(env);
        Ok(Self {
            surface,
            codec: retained_codec,
            track,
            converter: converter.take(),
            input_finished: false,
        })
    }

    const fn track_info(&self) -> &TrackInfo {
        &self.track
    }

    fn queue(&mut self, sample: &EncodedSample) -> Result<Vec<Duration>, Error> {
        if self.input_finished {
            return Err(Error::Codec(String::from(
                "cannot queue tunneled video after end-of-stream",
            )));
        }
        if sample.track_id() != self.track.id() {
            return Err(Error::Codec(format!(
                "tunneled video track {} received track {}",
                self.track.id().get(),
                sample.track_id().get()
            )));
        }
        if sample.encryption().is_some() {
            return Err(Error::Unsupported(format!(
                "encrypted tunneled video track {} requires a secure CDM tunnel",
                self.track.id().get()
            )));
        }
        if sample.is_discontinuity() {
            self.flush()?;
        }
        let data = self.converter.as_mut().map_or_else(
            || Ok(sample.data().to_vec()),
            |converter| {
                converter
                    .convert_sample(sample.data())
                    .map_err(|error| Error::Codec(error.to_string()))
            },
        )?;
        let presentation_time = sample.presentation_time().to_duration()?;
        self.with_env(|env, codec| {
            queue_clear_codec_input(env, codec, &data, presentation_time, 0)
        })?;
        self.drain_available_outputs()
    }

    fn finish_input(&mut self) -> Result<Vec<Duration>, Error> {
        if self.input_finished {
            return Ok(Vec::new());
        }
        self.with_env(|env, codec| {
            queue_clear_codec_input(
                env,
                codec,
                &[],
                Duration::ZERO,
                MEDIA_CODEC_BUFFER_FLAG_END_OF_STREAM,
            )
        })?;
        self.input_finished = true;
        let mut outputs = Vec::new();
        loop {
            if let Some(output) = self.dequeue_and_release_output(INPUT_WAIT_FOREVER_MICROS)? {
                if output.end_of_stream {
                    return Ok(outputs);
                }
                outputs.push(output.presentation_time);
            }
        }
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.with_env(|env, codec| {
            env.call_method(codec, "flush", "()V", &[])
                .map_err(|error| jni_error(env, "flush tunneled MediaCodec", error))?;
            Ok(())
        })?;
        self.input_finished = false;
        Ok(())
    }

    fn drain_available_outputs(&self) -> Result<Vec<Duration>, Error> {
        let mut outputs = Vec::new();
        while let Some(output) = self.dequeue_and_release_output(0)? {
            if !output.end_of_stream {
                outputs.push(output.presentation_time);
            }
        }
        Ok(outputs)
    }

    fn dequeue_and_release_output(
        &self,
        timeout_micros: i64,
    ) -> Result<Option<TunneledVideoOutput>, Error> {
        self.with_env(|env, codec| {
            let info = env
                .new_object("android/media/MediaCodec$BufferInfo", "()V", &[])
                .map_err(|error| jni_error(env, "create tunneled BufferInfo", error))?;
            loop {
                let index = env
                    .call_method(
                        codec,
                        "dequeueOutputBuffer",
                        "(Landroid/media/MediaCodec$BufferInfo;J)I",
                        &[JValue::Object(&info), JValue::Long(timeout_micros)],
                    )
                    .and_then(jni::objects::JValueGen::i)
                    .map_err(|error| jni_error(env, "dequeue tunneled output", error))?;
                match index {
                    MEDIA_CODEC_INFO_TRY_AGAIN_LATER => return Ok(None),
                    MEDIA_CODEC_INFO_OUTPUT_FORMAT_CHANGED
                    | MEDIA_CODEC_INFO_OUTPUT_BUFFERS_CHANGED => {}
                    index if index >= 0 => {
                        let flags = env
                            .get_field(&info, "flags", "I")
                            .and_then(jni::objects::JValueGen::i)
                            .map_err(|error| jni_error(env, "read tunneled output flags", error))?;
                        let presentation_time_micros = env
                            .get_field(&info, "presentationTimeUs", "J")
                            .and_then(jni::objects::JValueGen::j)
                            .map_err(|error| jni_error(env, "read tunneled output PTS", error))?;
                        let presentation_time_micros =
                            u64::try_from(presentation_time_micros).map_err(|_| {
                                Error::Codec(format!(
                                    "MediaCodec returned negative tunneled output PTS {presentation_time_micros}"
                                ))
                            })?;
                        env.call_method(
                            codec,
                            "releaseOutputBuffer",
                            "(IZ)V",
                            &[JValue::Int(index), JValue::Bool(1)],
                        )
                        .map_err(|error| jni_error(env, "release tunneled output", error))?;
                        return Ok(Some(TunneledVideoOutput {
                            presentation_time: Duration::from_micros(presentation_time_micros),
                            end_of_stream: flags & MEDIA_CODEC_BUFFER_FLAG_END_OF_STREAM != 0,
                        }));
                    }
                    other => {
                        return Err(Error::Platform(format!(
                            "MediaCodec returned unknown tunneled output status {other}"
                        )));
                    }
                }
            }
        })
    }

    fn with_env<T>(
        &self,
        operation: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut env = self
            .surface
            .context
            .vm
            .attach_current_thread()
            .map_err(|error| {
                Error::Platform(format!("attach tunneled decoder thread to JVM: {error}"))
            })?;
        operation(&mut env, self.codec.as_obj())
    }
}

struct TunneledVideoOutput {
    presentation_time: Duration,
    end_of_stream: bool,
}

impl Drop for AndroidTunneledVideoDecoder {
    fn drop(&mut self) {
        let Ok(mut env) = self.surface.context.vm.attach_current_thread() else {
            tracing::error!("failed to attach JVM while releasing tunneled MediaCodec");
            return;
        };
        release_codec(&mut env, self.codec.as_obj(), true);
    }
}

fn validate_video_track(track: &TrackInfo) -> Result<(), Error> {
    if track.kind() != TrackKind::Video {
        return Err(Error::Codec(format!(
            "track {} is not video and cannot use Android tunneling",
            track.id().get()
        )));
    }
    if track.protection().is_some() {
        return Err(Error::Unsupported(format!(
            "protected video track {} requires a secure CDM tunnel",
            track.id().get()
        )));
    }
    if track.video_dimensions().is_none() {
        return Err(Error::Container(format!(
            "tunneled video track {} has no coded dimensions",
            track.id().get()
        )));
    }
    Ok(())
}

fn video_codec_description(
    track: &TrackInfo,
) -> Result<(&'static str, Option<NalStreamConverter>), Error> {
    match track.codec() {
        Codec::H264 => NalStreamConverter::new(false, Some(track.decoder_configuration()))
            .map(|converter| ("video/avc", Some(converter)))
            .map_err(|error| Error::Codec(error.to_string())),
        Codec::H265 => NalStreamConverter::new(true, Some(track.decoder_configuration()))
            .map(|converter| ("video/hevc", Some(converter)))
            .map_err(|error| Error::Codec(error.to_string())),
        Codec::Av1 => Ok(("video/av01", None)),
        Codec::Vp9 => Ok(("video/x-vnd.on2.vp9", None)),
        Codec::Vp8 => Ok(("video/x-vnd.on2.vp8", None)),
        Codec::Mpeg2Video => Ok(("video/mpeg2", None)),
        codec => Err(Error::Unsupported(format!(
            "Android tunneling does not map video codec {codec:?}"
        ))),
    }
}

fn create_tunneled_video_format<'local>(
    env: &mut JNIEnv<'local>,
    mime: &str,
    width: i32,
    height: i32,
    audio_session_id: i32,
    primary_csd: &[u8],
    secondary_csd: &[u8],
) -> Result<JObject<'local>, Error> {
    let mime = env
        .new_string(mime)
        .map_err(|error| jni_error(env, "create tunneled video MIME", error))?;
    let format = env
        .call_static_method(
            "android/media/MediaFormat",
            "createVideoFormat",
            "(Ljava/lang/String;II)Landroid/media/MediaFormat;",
            &[
                JValue::Object(&mime),
                JValue::Int(width),
                JValue::Int(height),
            ],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| jni_error(env, "create tunneled video format", error))?;
    set_media_format_integer(env, &format, "audio-session-id", audio_session_id)?;
    set_codec_specific_data(env, &format, "csd-0", primary_csd)?;
    set_codec_specific_data(env, &format, "csd-1", secondary_csd)?;
    let feature = env
        .new_string("tunneled-playback")
        .map_err(|error| jni_error(env, "create tunneled-playback feature", error))?;
    env.call_method(
        &format,
        "setFeatureEnabled",
        "(Ljava/lang/String;Z)V",
        &[JValue::Object(&feature), JValue::Bool(1)],
    )
    .map_err(|error| jni_error(env, "require tunneled-playback feature", error))?;
    Ok(format)
}

fn create_tunneled_media_codec<'local>(
    env: &mut JNIEnv<'local>,
    mime: &str,
    format: &JObject<'_>,
    surface: &JObject<'_>,
) -> Result<JObject<'local>, Error> {
    let codec_list = env
        .new_object(
            "android/media/MediaCodecList",
            "(I)V",
            &[JValue::Int(MEDIA_CODEC_LIST_ALL_CODECS)],
        )
        .map_err(|error| jni_error(env, "create tunneled MediaCodecList", error))?;
    let decoder_name = env
        .call_method(
            &codec_list,
            "findDecoderForFormat",
            "(Landroid/media/MediaFormat;)Ljava/lang/String;",
            &[JValue::Object(format)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| jni_error(env, "select tunneled decoder", error))?;
    if decoder_name.is_null() {
        return Err(Error::Unsupported(format!(
            "Android has no decoder advertising tunneled-playback for {mime}"
        )));
    }
    let codec = env
        .call_static_method(
            "android/media/MediaCodec",
            "createByCodecName",
            "(Ljava/lang/String;)Landroid/media/MediaCodec;",
            &[JValue::Object(&decoder_name)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| jni_error(env, "create tunneled MediaCodec", error))?;
    let null_crypto = JObject::null();
    if let Err(error) = env.call_method(
        &codec,
        "configure",
        "(Landroid/media/MediaFormat;Landroid/view/Surface;Landroid/media/MediaCrypto;I)V",
        &[
            JValue::Object(format),
            JValue::Object(surface),
            JValue::Object(&null_crypto),
            JValue::Int(0),
        ],
    ) {
        release_codec(env, &codec, false);
        return Err(jni_error(env, "configure tunneled MediaCodec", error));
    }
    if let Err(error) = env.call_method(&codec, "start", "()V", &[]) {
        release_codec(env, &codec, false);
        return Err(jni_error(env, "start tunneled MediaCodec", error));
    }
    Ok(codec)
}

fn queue_clear_codec_input(
    env: &mut JNIEnv<'_>,
    codec: &JObject<'_>,
    data: &[u8],
    presentation_time: Duration,
    flags: i32,
) -> Result<(), Error> {
    let index = env
        .call_method(
            codec,
            "dequeueInputBuffer",
            "(J)I",
            &[JValue::Long(INPUT_WAIT_FOREVER_MICROS)],
        )
        .and_then(jni::objects::JValueGen::i)
        .map_err(|error| jni_error(env, "dequeue tunneled input", error))?;
    if index < 0 {
        return Err(Error::Platform(format!(
            "MediaCodec returned {index} while waiting for tunneled input"
        )));
    }
    if !data.is_empty() {
        write_codec_input(env, codec, index, data)?;
    }
    let size = i32::try_from(data.len())
        .map_err(|_| Error::Codec(String::from("tunneled access unit exceeds Android jint")))?;
    let pts = i64::try_from(presentation_time.as_micros())
        .map_err(|_| Error::Codec(String::from("tunneled video PTS exceeds Android jlong")))?;
    env.call_method(
        codec,
        "queueInputBuffer",
        "(IIIJI)V",
        &[
            JValue::Int(index),
            JValue::Int(0),
            JValue::Int(size),
            JValue::Long(pts),
            JValue::Int(flags),
        ],
    )
    .map_err(|error| jni_error(env, "queue tunneled input", error))?;
    Ok(())
}

fn write_codec_input(
    env: &mut JNIEnv<'_>,
    codec: &JObject<'_>,
    index: i32,
    data: &[u8],
) -> Result<(), Error> {
    let buffer = env
        .call_method(
            codec,
            "getInputBuffer",
            "(I)Ljava/nio/ByteBuffer;",
            &[JValue::Int(index)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| jni_error(env, "get tunneled input buffer", error))?;
    if buffer.is_null() {
        return Err(Error::Platform(format!(
            "MediaCodec tunneled input buffer {index} is null"
        )));
    }
    env.call_method(&buffer, "clear", "()Ljava/nio/Buffer;", &[])
        .map_err(|error| jni_error(env, "clear tunneled input buffer", error))?;
    let capacity = env
        .call_method(&buffer, "remaining", "()I", &[])
        .and_then(jni::objects::JValueGen::i)
        .map_err(|error| jni_error(env, "read tunneled input capacity", error))?;
    let data_len = i32::try_from(data.len())
        .map_err(|_| Error::Codec(String::from("tunneled access unit exceeds Android jint")))?;
    if data_len > capacity {
        return Err(Error::Codec(format!(
            "tunneled access unit has {data_len} bytes but input capacity is {capacity}"
        )));
    }
    let data = env
        .byte_array_from_slice(data)
        .map_err(|error| jni_error(env, "create tunneled input bytes", error))?;
    env.call_method(
        &buffer,
        "put",
        "([B)Ljava/nio/ByteBuffer;",
        &[JValue::Object(&data)],
    )
    .map_err(|error| jni_error(env, "write tunneled input buffer", error))?;
    Ok(())
}

fn set_media_format_integer(
    env: &mut JNIEnv<'_>,
    format: &JObject<'_>,
    key: &str,
    value: i32,
) -> Result<(), Error> {
    let key = env
        .new_string(key)
        .map_err(|error| jni_error(env, "create tunneled MediaFormat key", error))?;
    env.call_method(
        format,
        "setInteger",
        "(Ljava/lang/String;I)V",
        &[JValue::Object(&key), JValue::Int(value)],
    )
    .map_err(|error| jni_error(env, "set tunneled MediaFormat integer", error))?;
    Ok(())
}

fn set_codec_specific_data(
    env: &mut JNIEnv<'_>,
    format: &JObject<'_>,
    key: &str,
    data: &[u8],
) -> Result<(), Error> {
    if data.is_empty() {
        return Ok(());
    }
    let key = env
        .new_string(key)
        .map_err(|error| jni_error(env, "create tunneled codec-data key", error))?;
    let data = env
        .byte_array_from_slice(data)
        .map_err(|error| jni_error(env, "create tunneled codec data", error))?;
    let buffer = env
        .call_static_method(
            "java/nio/ByteBuffer",
            "wrap",
            "([B)Ljava/nio/ByteBuffer;",
            &[JValue::Object(&data)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| jni_error(env, "wrap tunneled codec data", error))?;
    env.call_method(
        format,
        "setByteBuffer",
        "(Ljava/lang/String;Ljava/nio/ByteBuffer;)V",
        &[JValue::Object(&key), JValue::Object(&buffer)],
    )
    .map_err(|error| jni_error(env, "install tunneled codec data", error))?;
    Ok(())
}

fn release_codec(env: &mut JNIEnv<'_>, codec: &JObject<'_>, stop: bool) {
    let methods: &[&str] = if stop {
        &["stop", "release"]
    } else {
        &["release"]
    };
    for method in methods {
        if let Err(error) = env.call_method(codec, method, "()V", &[]) {
            tracing::error!(%error, %method, "failed to release Android tunneled MediaCodec");
            let _ = env.exception_clear();
        }
    }
}

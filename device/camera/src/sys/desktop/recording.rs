//! Desktop video recording pipeline.
//!
//! Brings capture-stream frames to `NV12` (NV12 frames pass through, YUYV is
//! re-subsampled, decoded MJPEG is converted from RGBA), encodes them through
//! `waterkit-codec` (hardware encoders first, software AV1 when no hardware
//! encoder exists), and muxes the packets with `waterkit-video-container`.
//! Raw recording writes the uncompressed `WKRV` RGBA frame stream the mobile
//! backends already produce.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use waterkit_codec::{CodecType, Encoder, EncoderProfile, annex_b_to_length_prefixed};
use waterkit_video_container::{MuxerCodecType, VideoWriter};
use yuv::{
    YuvBiPlanarImage, YuvBiPlanarImageMut, YuvChromaSubsampling, YuvConversionMode, YuvPackedImage,
    YuvPlanarImageMut, YuvRange, YuvStandardMatrix,
};

use super::{Captured, CapturedPixels, FrameSubscription, RawFrame, UVC_YCBCR_COLOR};
use crate::{CameraError, ColorRange, MatrixCoefficients, VideoColorInfo};

/// The `yuv` crate's names for a frame's YCbCr encoding.
fn yuv_encoding(color: VideoColorInfo) -> Result<(YuvRange, YuvStandardMatrix), CameraError> {
    let matrix = match color.matrix {
        MatrixCoefficients::Bt601 => YuvStandardMatrix::Bt601,
        MatrixCoefficients::Bt709 => YuvStandardMatrix::Bt709,
        MatrixCoefficients::Bt2020NonConstantLuminance => YuvStandardMatrix::Bt2020,
        MatrixCoefficients::Bt2020ConstantLuminance => {
            return Err(CameraError::RecordingError(format!(
                "matrix {:?} is unsupported by the yuv conversion for color description {color:?}",
                color.matrix
            )));
        }
    };
    let range = match color.range {
        ColorRange::Limited => YuvRange::Limited,
        ColorRange::Full => YuvRange::Full,
    };
    Ok((range, matrix))
}

fn yuv_error(error: &yuv::YuvError) -> CameraError {
    CameraError::RecordingError(format!("pixel layout conversion: {error}"))
}

/// Which encoder ended up serving the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EncoderChoice {
    /// Hardware H.264 (VA-API on Linux, Media Foundation on Windows).
    H264,
    /// Hardware H.265 (Media Foundation on Windows).
    H265,
    /// Software AV1 (rav1e).
    Av1,
}

impl EncoderChoice {
    const fn muxer_codec(self) -> MuxerCodecType {
        match self {
            Self::H264 => MuxerCodecType::H264,
            Self::H265 => MuxerCodecType::H265,
            Self::Av1 => MuxerCodecType::Av1,
        }
    }
}

/// Compressed-video recording session: `NV12` frames in, MP4/MOV out.
pub(super) struct RecordingPipeline {
    encoder: Encoder,
    choice: EncoderChoice,
    writer: VideoWriter,
    width: u32,
    height: u32,
    color: VideoColorInfo,
    nv12: Vec<u8>,
}

impl RecordingPipeline {
    /// Create the pipeline, trying the platform hardware encoders first and
    /// the software AV1 encoder after them.
    ///
    /// When no encoder can be constructed the error names every encoder that
    /// was tried and why it failed.
    pub(super) fn new(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        color: VideoColorInfo,
    ) -> Result<Self, CameraError> {
        Self::new_with_codecs(
            path,
            width,
            height,
            fps,
            color,
            &[
                (CodecType::H264, "h264"),
                (CodecType::H265, "h265"),
                (CodecType::Av1, "av1 (software)"),
            ],
        )
    }

    #[cfg(test)]
    fn new_av1(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        color: VideoColorInfo,
    ) -> Result<Self, CameraError> {
        Self::new_with_codecs(
            path,
            width,
            height,
            fps,
            color,
            &[(CodecType::Av1, "av1 (software)")],
        )
    }

    fn new_with_codecs(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        color: VideoColorInfo,
        codecs: &[(CodecType, &str)],
    ) -> Result<Self, CameraError> {
        let mut attempts = Vec::new();
        let mut encoder = None;
        let mut choice = EncoderChoice::Av1;

        for &(codec, name) in codecs {
            match Encoder::new(codec, width, height, EncoderProfile::Realtime, color) {
                Ok(enc) => {
                    choice = match codec {
                        CodecType::H264 => EncoderChoice::H264,
                        CodecType::H265 => EncoderChoice::H265,
                        CodecType::Av1 => EncoderChoice::Av1,
                    };
                    encoder = Some(enc);
                    break;
                }
                Err(err) => attempts.push(format!("{name}: {err}")),
            }
        }

        let Some(encoder) = encoder else {
            return Err(CameraError::RecordingError(format!(
                "no video encoder available on this machine ({})",
                attempts.join("; ")
            )));
        };

        let writer = VideoWriter::new(path, width, height, fps, choice.muxer_codec(), color)
            .map_err(|err| CameraError::RecordingError(format!("video writer: {err}")))?;

        Ok(Self {
            encoder,
            choice,
            writer,
            width,
            height,
            color,
            nv12: Vec::new(),
        })
    }

    /// Bring one capture frame to NV12, encode it, and append the sample.
    pub(super) fn push(&mut self, frame: &RawFrame) -> Result<(), CameraError> {
        if frame.color != self.color {
            return Err(CameraError::RecordingError(format!(
                "captured frame color {:?} differs from recording color {:?}",
                frame.color, self.color
            )));
        }
        match &frame.pixels {
            CapturedPixels::Nv12(nv12) => self.encode(nv12),
            CapturedPixels::Yuyv(yuyv) => {
                self.nv12 = yuyv_to_nv12(yuyv, self.width, self.height)?;
                self.encode_buffered()
            }
            CapturedPixels::Rgba(rgba) => {
                let (range, matrix) = yuv_encoding(self.color)?;
                let mut nv12 = YuvBiPlanarImageMut::alloc(
                    self.width,
                    self.height,
                    YuvChromaSubsampling::Yuv420,
                );
                yuv::rgba_to_yuv_nv12(
                    &mut nv12,
                    rgba,
                    self.width * 4,
                    range,
                    matrix,
                    YuvConversionMode::Balanced,
                )
                .map_err(|error| yuv_error(&error))?;
                self.nv12.clear();
                self.nv12.extend_from_slice(nv12.y_plane.borrow());
                self.nv12.extend_from_slice(nv12.uv_plane.borrow());
                self.encode_buffered()
            }
        }
    }

    fn encode_buffered(&mut self) -> Result<(), CameraError> {
        let nv12 = std::mem::take(&mut self.nv12);
        let result = self.encode(&nv12);
        self.nv12 = nv12;
        result
    }

    fn encode(&mut self, nv12: &[u8]) -> Result<(), CameraError> {
        let mut packet = Vec::new();
        for item in self.encoder.encode_nv12(nv12) {
            let bytes =
                item.map_err(|err| CameraError::RecordingError(format!("video encoder: {err}")))?;
            packet.extend_from_slice(&bytes);
        }
        if packet.is_empty() {
            return Ok(());
        }
        self.emit_packet(packet)
    }

    /// Annex-B (h26x) or OBU (av1) packet → container sample.
    fn emit_packet(&mut self, packet: Vec<u8>) -> Result<(), CameraError> {
        // Surface the codec configuration as soon as the encoder produces it;
        // VA-API only learns avcC once the first packet has been encoded.
        if let Some(config) = self.encoder.codec_config() {
            self.writer.set_codec_config(config);
        }

        let (sample, is_keyframe) = match self.choice {
            EncoderChoice::H264 => {
                let (avcc, keyframe) = annex_b_to_length_prefixed(&packet, false);
                (avcc.unwrap_or(packet), keyframe)
            }
            EncoderChoice::H265 => {
                let (avcc, keyframe) = annex_b_to_length_prefixed(&packet, true);
                (avcc.unwrap_or(packet), keyframe)
            }
            EncoderChoice::Av1 => {
                let keyframe = packet_contains_sequence_header(&packet);
                (packet, keyframe)
            }
        };

        self.writer
            .write_sample(&sample, is_keyframe)
            .map_err(|err| CameraError::RecordingError(format!("video writer: {err}")))
    }

    /// Drain the encoder's held frames and finalize the container.
    pub(super) fn finish(mut self) -> Result<(), CameraError> {
        let packets = self
            .encoder
            .flush()
            .map_err(|err| CameraError::RecordingError(format!("video encoder: {err}")))?;
        for packet in packets {
            self.emit_packet(packet)?;
        }
        self.writer
            .finish()
            .map_err(|err| CameraError::RecordingError(format!("video writer: {err}")))
    }
}

/// What a recording session runs on its worker thread.
enum RecordingSink {
    Compressed(Box<RecordingPipeline>),
    Raw(RawVideoWriter),
}

/// What woke the recording worker out of its `select`.
enum Wake {
    Frame(Result<Captured, async_channel::RecvError>),
    Stop,
}

impl RecordingSink {
    /// Records one captured frame, or ends the recording with the error that
    /// ended the capture.
    fn push(&mut self, captured: &Captured) -> Result<(), CameraError> {
        let frame = captured.as_ref().map_err(Clone::clone)?;
        match self {
            Self::Compressed(pipeline) => pipeline.push(frame),
            Self::Raw(writer) => writer.push_frame(
                &frame.pixels,
                frame.width,
                frame.height,
                u64::try_from(frame.timestamp.as_nanos()).unwrap_or(u64::MAX),
            ),
        }
    }

    fn finish(self) -> Result<(), CameraError> {
        match self {
            Self::Compressed(pipeline) => pipeline.finish(),
            Self::Raw(writer) => writer.finish(),
        }
    }
}

/// A live recording session owned by `CameraInner`.
pub(super) struct RecordingSession {
    stop_flag: Arc<AtomicBool>,
    stop_tx: async_channel::Sender<()>,
    join: JoinHandle<Result<(), CameraError>>,
    dropped: Arc<AtomicU64>,
    start: std::time::Instant,
}

/// What the worker thread should build. The codec `Encoder` is `!Send` on
/// Linux (the VA-API encoder inside is), so it is constructed on the worker
/// thread that owns it rather than moved across.
enum RecordingSpec {
    Compressed {
        path: PathBuf,
        width: u32,
        height: u32,
        fps: u32,
        color: VideoColorInfo,
    },
    Raw {
        path: PathBuf,
        width: u32,
        height: u32,
        fps: u32,
    },
}

impl RecordingSpec {
    fn build(self) -> Result<RecordingSink, CameraError> {
        match self {
            Self::Compressed {
                path,
                width,
                height,
                fps,
                color,
            } => Ok(RecordingSink::Compressed(Box::new(RecordingPipeline::new(
                &path, width, height, fps, color,
            )?))),
            Self::Raw {
                path,
                width,
                height,
                fps,
            } => Ok(RecordingSink::Raw(RawVideoWriter::new(
                &path, width, height, fps,
            )?)),
        }
    }
}

impl RecordingSession {
    /// Start a compressed recording on a dedicated worker thread fed by
    /// the subscription, keeping encoding off the capture thread.
    pub(super) fn compressed(
        path: &Path,
        subscription: FrameSubscription,
        width: u32,
        height: u32,
        fps: u32,
        color: VideoColorInfo,
    ) -> Result<Self, CameraError> {
        Self::spawn(
            subscription,
            RecordingSpec::Compressed {
                path: path.to_path_buf(),
                width,
                height,
                fps,
                color,
            },
        )
    }

    /// Start an uncompressed `WKRV` frame-stream recording.
    pub(super) fn raw(
        path: &Path,
        subscription: FrameSubscription,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self, CameraError> {
        Self::spawn(
            subscription,
            RecordingSpec::Raw {
                path: path.to_path_buf(),
                width,
                height,
                fps,
            },
        )
    }

    fn spawn(subscription: FrameSubscription, spec: RecordingSpec) -> Result<Self, CameraError> {
        let FrameSubscription {
            receiver: frame_rx,
            dropped,
        } = subscription;
        let stop_flag = Arc::new(AtomicBool::new(false));
        let (stop_tx, stop_rx) = async_channel::bounded::<()>(1);
        // The worker reports whether the sink (encoder + muxer, or the raw
        // writer) could be opened before this call returns, so `start_*`
        // fails fast on an unavailable encoder rather than at stop.
        let (init_tx, init_rx) = async_channel::bounded::<Result<(), CameraError>>(1);
        let worker_flag = Arc::clone(&stop_flag);
        let join = std::thread::spawn(move || {
            let mut sink = match spec.build() {
                Ok(sink) => {
                    let _ = init_tx.try_send(Ok(()));
                    sink
                }
                Err(err) => {
                    let _ = init_tx.try_send(Err(err));
                    return Ok(());
                }
            };
            let mut result: Result<(), CameraError> = Ok(());
            loop {
                if worker_flag.load(Ordering::SeqCst) {
                    break;
                }
                // `Recv` is !Unpin so the futures are pinned on the stack;
                // the `Either` (which borrows the loser) is consumed inside
                // the async block so nothing escapes its scope.
                let wake = futures::executor::block_on(async {
                    let frame = frame_rx.recv();
                    let stop = stop_rx.recv();
                    futures::pin_mut!(frame);
                    futures::pin_mut!(stop);
                    match futures::future::select(frame, stop).await {
                        futures::future::Either::Left((received, _)) => Wake::Frame(received),
                        futures::future::Either::Right(_) => Wake::Stop,
                    }
                });
                match wake {
                    Wake::Frame(Ok(frame)) => {
                        if let Err(err) = sink.push(&frame) {
                            result = Err(err);
                            break;
                        }
                    }
                    // Capture ended or stop was signalled: finish the file.
                    Wake::Frame(Err(_)) | Wake::Stop => break,
                }
            }
            // Frames queued before the stop/capture-end signal still belong
            // in the file; drain whatever is left without blocking.
            if result.is_ok() {
                while let Ok(frame) = frame_rx.try_recv() {
                    if let Err(err) = sink.push(&frame) {
                        result = Err(err);
                        break;
                    }
                }
            }
            result.and_then(|()| sink.finish())
        });
        match futures::executor::block_on(init_rx.recv()) {
            Ok(Ok(())) => Ok(Self {
                stop_flag,
                stop_tx,
                join,
                dropped,
                start: std::time::Instant::now(),
            }),
            Ok(Err(err)) => {
                let _ = join.join();
                Err(err)
            }
            Err(_) => Err(CameraError::RecordingError(
                "recording worker exited before initialization".into(),
            )),
        }
    }

    /// Stop the session and finalize the file.
    pub(super) fn stop(self) -> Result<(), CameraError> {
        let Self {
            stop_flag,
            stop_tx,
            join,
            dropped,
            ..
        } = self;
        stop_flag.store(true, Ordering::SeqCst);
        drop(stop_tx);
        let result = join
            .join()
            .map_err(|_| CameraError::RecordingError("recording worker panicked".into()))?;
        let dropped_frames = dropped.load(Ordering::Relaxed);
        if dropped_frames > 0 {
            tracing::warn!(
                "camera recording dropped {dropped_frames} frame(s): the encoder could not keep up with the capture rate"
            );
        }
        result
    }

    /// Elapsed recording time.
    pub(super) fn duration(&self) -> std::time::Duration {
        self.start.elapsed()
    }
}

/// Uncompressed frame-stream writer matching the mobile `WKRV` format: a
/// header of `WKRV` + version 1 + pixel-format byte + reserved u16 + width,
/// height and fps as u32, then per frame `timestamp_ns` u64, `payload_len`
/// u32 and the RGBA payload, all little-endian.
pub(super) struct RawVideoWriter {
    file: BufWriter<File>,
    /// Reused RGBA buffer for frames the camera delivered as YCbCr.
    rgba: Vec<u8>,
}

impl RawVideoWriter {
    /// Pixel-format byte used by the mobile backends: 1 = BGRA8, 2 = RGBA8.
    const PIXEL_FORMAT_RGBA8: u8 = 2;

    fn new(path: &Path, width: u32, height: u32, fps: u32) -> Result<Self, CameraError> {
        let mut file = BufWriter::new(
            File::create(path)
                .map_err(|err| CameraError::RecordingError(format!("raw video writer: {err}")))?,
        );
        file.write_all(b"WKRV")
            .and_then(|()| file.write_all(&[1, Self::PIXEL_FORMAT_RGBA8, 0, 0]))
            .and_then(|()| file.write_all(&width.to_le_bytes()))
            .and_then(|()| file.write_all(&height.to_le_bytes()))
            .and_then(|()| file.write_all(&fps.to_le_bytes()))
            .map_err(|err| CameraError::RecordingError(format!("raw video writer: {err}")))?;
        Ok(Self {
            file,
            rgba: Vec::new(),
        })
    }

    /// Append one capture frame as RGBA8.
    fn push_frame(
        &mut self,
        pixels: &CapturedPixels,
        width: u32,
        height: u32,
        timestamp_ns: u64,
    ) -> Result<(), CameraError> {
        let (range, matrix) = yuv_encoding(UVC_YCBCR_COLOR)?;
        let rgba_len = width as usize * height as usize * 4;
        match pixels {
            CapturedPixels::Rgba(rgba) => return self.write_rgba(rgba, timestamp_ns),
            CapturedPixels::Nv12(nv12) => {
                let (y_plane, uv_plane) = nv12.split_at(width as usize * height as usize);
                self.rgba.resize(rgba_len, 0);
                yuv::yuv_nv12_to_rgba(
                    &YuvBiPlanarImage {
                        y_plane,
                        y_stride: width,
                        uv_plane,
                        uv_stride: width,
                        width,
                        height,
                    },
                    &mut self.rgba,
                    width * 4,
                    range,
                    matrix,
                    YuvConversionMode::Balanced,
                )
                .map_err(|error| yuv_error(&error))?;
            }
            CapturedPixels::Yuyv(yuyv) => {
                self.rgba.resize(rgba_len, 0);
                yuv::yuyv422_to_rgba(
                    &YuvPackedImage {
                        yuy: yuyv,
                        yuy_stride: width * 2,
                        width,
                        height,
                    },
                    &mut self.rgba,
                    width * 4,
                    range,
                    matrix,
                )
                .map_err(|error| yuv_error(&error))?;
            }
        }
        let rgba = std::mem::take(&mut self.rgba);
        let result = self.write_rgba(&rgba, timestamp_ns);
        self.rgba = rgba;
        result
    }

    fn write_rgba(&mut self, rgba: &[u8], timestamp_ns: u64) -> Result<(), CameraError> {
        let len = u32::try_from(rgba.len()).map_err(|_| {
            CameraError::RecordingError("raw frame exceeds u32 payload length".into())
        })?;
        self.file
            .write_all(&timestamp_ns.to_le_bytes())
            .and_then(|()| self.file.write_all(&len.to_le_bytes()))
            .and_then(|()| self.file.write_all(rgba))
            .map_err(|err| CameraError::RecordingError(format!("raw video writer: {err}")))
    }

    fn finish(mut self) -> Result<(), CameraError> {
        self.file
            .flush()
            .map_err(|err| CameraError::RecordingError(format!("raw video writer: {err}")))
    }
}

/// Re-subsample packed YUYV 4:2:2 to NV12 4:2:0; the samples keep their
/// encoding.
fn yuyv_to_nv12(yuyv: &[u8], width: u32, height: u32) -> Result<Vec<u8>, CameraError> {
    let mut planar = YuvPlanarImageMut::alloc(width, height, YuvChromaSubsampling::Yuv420);
    yuv::yuyv422_to_yuv420(
        &mut planar,
        &YuvPackedImage {
            yuy: yuyv,
            yuy_stride: width * 2,
            width,
            height,
        },
    )
    .map_err(|error| yuv_error(&error))?;
    let mut nv12 = Vec::with_capacity(width as usize * height as usize * 3 / 2);
    nv12.extend_from_slice(planar.y_plane.borrow());
    for (u, v) in planar.u_plane.borrow().iter().zip(planar.v_plane.borrow()) {
        nv12.extend_from_slice(&[*u, *v]);
    }
    Ok(nv12)
}

fn read_leb128(data: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for i in 0..8 {
        let byte = *data.get(i)?;
        value |= usize::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// Iterate the top-level OBUs of an AV1 packet, yielding
/// `(obu_type, whole_obu_bytes)` pairs.
fn av1_obus(data: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let header = data[pos];
        if header & 0x80 != 0 {
            break;
        }
        let obu_type = (header >> 3) & 0x0f;
        let mut cursor = pos + 1;
        if header & 0x04 != 0 {
            cursor += 1; // obu_extension_flag
        }
        let payload_len = if header & 0x02 != 0 {
            let Some((len, bytes)) = read_leb128(&data[cursor..]) else {
                break;
            };
            cursor += bytes;
            len
        } else {
            data.len() - cursor
        };
        if cursor + payload_len > data.len() {
            break;
        }
        out.push((obu_type, &data[pos..cursor + payload_len]));
        pos = cursor + payload_len;
    }
    out
}

/// Whether an AV1 packet contains a sequence header OBU — true exactly for
/// keyframes in rav1e output.
fn packet_contains_sequence_header(data: &[u8]) -> bool {
    av1_obus(data).iter().any(|(ty, _)| *ty == 1)
}

#[cfg(test)]
mod tests {
    use super::super::RawFrame;
    use super::*;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;
    use waterkit_codec::{DecodePacket, Decoder};
    use waterkit_video_container::{VideoReader, probe_mp4_color_info};

    fn temp_path(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "waterkit-camera-{name}-{}-{nanos}.mp4",
            std::process::id()
        ))
    }

    fn synthetic_rgba(width: usize, height: usize, frame_index: usize) -> Vec<u8> {
        let mut rgba = vec![0u8; width * height * 4];
        for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let v = u8::try_from(i + frame_index).unwrap_or(u8::MAX);
            px[0] = v;
            px[1] = v.wrapping_mul(2);
            px[2] = 255 - v;
            px[3] = 255;
        }
        rgba
    }

    fn captured_frame(
        pixels: CapturedPixels,
        color: VideoColorInfo,
        width: u32,
        height: u32,
    ) -> RawFrame {
        RawFrame {
            pixels,
            color,
            width,
            height,
            timestamp: Duration::ZERO,
        }
    }

    fn decoded_av1_color(path: &Path, width: u32, height: u32) -> waterkit_video_core::CicpColor {
        let mut reader = VideoReader::open(path).expect("recorded file must reopen");
        let timescale = u128::from(reader.timescale());
        let samples: Vec<_> = reader
            .samples()
            .map(|sample| sample.expect("every recorded sample must read back"))
            .collect();
        let mut decoder =
            Decoder::new(CodecType::Av1, None, width, height).expect("software AV1 decoder");

        for (data, pts, _) in samples {
            let timestamp_ns = u64::try_from(u128::from(pts) * 1_000_000_000 / timescale).unwrap();
            let packet = DecodePacket::new(&data, Duration::from_nanos(timestamp_ns));
            for frame in decoder.decode(packet) {
                if let Some(color) = frame.expect("AV1 sample decodes").color() {
                    return color;
                }
            }
        }
        for frame in decoder.drain() {
            if let Some(color) = frame.expect("AV1 decoder drains").color() {
                return color;
            }
        }
        panic!("decoded AV1 frames must expose their CICP description");
    }

    fn assert_recording_color(path: &Path, color: VideoColorInfo, width: u32, height: u32) {
        assert_eq!(
            probe_mp4_color_info(path, None).expect("MP4 color metadata is readable"),
            color
        );
        assert_eq!(decoded_av1_color(path, width, height), color.cicp());
    }

    #[test]
    fn compressed_recording_roundtrips_through_the_container_reader() {
        let (width, height, fps, frames) = (64u32, 48u32, 30u32, 12usize);
        let path = temp_path("compressed");
        let mut pipeline =
            RecordingPipeline::new_av1(&path, width, height, fps, super::super::JFIF_RGB_COLOR)
                .expect("at least the software AV1 encoder must be constructible");

        for index in 0..frames {
            pipeline
                .push(&captured_frame(
                    CapturedPixels::Rgba(synthetic_rgba(width as usize, height as usize, index)),
                    super::super::JFIF_RGB_COLOR,
                    width,
                    height,
                ))
                .expect("encoding a synthetic frame");
        }
        pipeline.finish().expect("the container must finalize");

        let mut reader = VideoReader::open(&path).expect("recorded file must reopen");
        assert_eq!(reader.dimensions(), (width, height));
        assert_eq!(reader.sample_count() as usize, frames);
        let samples: Vec<_> = reader.samples().collect();
        assert_eq!(samples.len(), frames);
        for sample in samples {
            let (data, _, _) = sample.expect("every recorded sample must read back");
            assert_ne!(data, [] as [u8; 0]);
        }
        assert_recording_color(&path, super::super::JFIF_RGB_COLOR, width, height);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn raw_recording_writes_the_wkrv_frame_stream() {
        let (width, height, fps) = (32u32, 16u32, 15u32);
        let path = temp_path("raw");
        let mut writer = RawVideoWriter::new(&path, width, height, fps).expect("raw writer");

        let frame_bytes = width as usize * height as usize * 4;
        let frames: Vec<Vec<u8>> = (0..3)
            .map(|i| synthetic_rgba(width as usize, height as usize, i))
            .collect();
        for (i, frame) in frames.iter().enumerate() {
            writer
                .push_frame(
                    &CapturedPixels::Rgba(frame.clone()),
                    width,
                    height,
                    i as u64 * 33_333_333,
                )
                .expect("raw frame write");
        }
        writer.finish().expect("raw writer finalize");

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"WKRV");
        assert_eq!(bytes[4], 1); // version
        assert_eq!(bytes[5], RawVideoWriter::PIXEL_FORMAT_RGBA8);
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), width);
        assert_eq!(
            u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            height
        );
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), fps);

        let mut pos = 20;
        for (i, frame) in frames.iter().enumerate() {
            let ts = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            assert_eq!(ts, u64::try_from(i).unwrap() * 33_333_333);
            let len = u32::from_le_bytes(bytes[pos + 8..pos + 12].try_into().unwrap()) as usize;
            assert_eq!(len, frame_bytes);
            assert_eq!(&bytes[pos + 12..pos + 12 + len], &frame[..]);
            pos += 12 + len;
        }
        assert_eq!(pos, bytes.len());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn recording_rejects_frames_with_a_different_color_description() {
        let (width, height, fps) = (16u32, 16u32, 30u32);
        let path = temp_path("color-mismatch");
        let mut pipeline = RecordingPipeline::new_av1(&path, width, height, fps, UVC_YCBCR_COLOR)
            .expect("software AV1 encoder");
        let error = pipeline
            .push(&captured_frame(
                CapturedPixels::Rgba(vec![0; width as usize * height as usize * 4]),
                super::super::JFIF_RGB_COLOR,
                width,
                height,
            ))
            .expect_err("recording cannot change its color description");
        let CameraError::RecordingError(message) = error else {
            panic!("expected a recording error");
        };
        assert!(message.contains(&format!("{:?}", super::super::JFIF_RGB_COLOR)));
        assert!(message.contains(&format!("{UVC_YCBCR_COLOR:?}")));
        drop(pipeline);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn yuv_conversion_rejects_bt2020_constant_luminance() {
        let color = VideoColorInfo {
            matrix: MatrixCoefficients::Bt2020ConstantLuminance,
            ..UVC_YCBCR_COLOR
        };
        let error = yuv_encoding(color).expect_err("the yuv crate has no BT.2020 CL path");
        let CameraError::RecordingError(message) = error else {
            panic!("expected a recording error");
        };
        assert!(message.contains("Bt2020ConstantLuminance"));
        assert!(message.contains(&format!("{color:?}")));
    }

    #[test]
    fn rgba_recording_conversion_uses_jfif_601_full_range() {
        let (width, height, fps) = (16u32, 16u32, 30u32);
        let path = temp_path("rgba-jfif-conversion");
        let mut pipeline =
            RecordingPipeline::new_av1(&path, width, height, fps, super::super::JFIF_RGB_COLOR)
                .expect("software AV1 encoder");
        let rgba = [255, 0, 0, 255].repeat(width as usize * height as usize);
        pipeline
            .push(&captured_frame(
                CapturedPixels::Rgba(rgba),
                super::super::JFIF_RGB_COLOR,
                width,
                height,
            ))
            .expect("red RGBA frame converts to NV12");

        let pixels = width as usize * height as usize;
        assert!(pipeline.nv12[..pixels].iter().all(|&value| value == 76));
        let (uv_pairs, remainder) = pipeline.nv12[pixels..].as_chunks::<2>();
        assert!(remainder.is_empty());
        assert!(uv_pairs.iter().all(|uv| uv[0] == 85 && uv[1] == 255));
        pipeline.finish().expect("the container must finalize");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn recording_session_streams_frames_to_file() {
        let (width, height, fps, count) = (32u32, 16u32, 30u32, 6usize);
        let path = temp_path("session");
        let (tx, rx) = async_channel::bounded(4);
        let dropped = Arc::new(AtomicU64::new(0));
        let session = RecordingSession::compressed(
            &path,
            FrameSubscription {
                receiver: rx,
                dropped,
            },
            width,
            height,
            fps,
            super::super::JFIF_RGB_COLOR,
        )
        .expect("session opens");
        std::thread::spawn(move || {
            for i in 0..count {
                let mut frame = captured_frame(
                    CapturedPixels::Rgba(synthetic_rgba(width as usize, height as usize, i)),
                    super::super::JFIF_RGB_COLOR,
                    width,
                    height,
                );
                frame.timestamp = Duration::from_nanos(i as u64 * 33_333_333);
                let frame = Arc::new(frame);
                tx.send_blocking(Ok(frame)).expect("frame channel open");
            }
        })
        .join()
        .unwrap();
        session.stop().expect("stop finalizes the recording");

        let reader = VideoReader::open(&path).unwrap();
        assert_eq!(reader.sample_count() as usize, count);
        assert_eq!(
            probe_mp4_color_info(&path, None).expect("MP4 color metadata is readable"),
            super::super::JFIF_RGB_COLOR
        );
        std::fs::remove_file(&path).ok();
    }

    /// Frames the camera delivered as NV12 or YUYV reach the encoder and the
    /// raw writer in their own layouts.
    #[test]
    fn ycbcr_capture_frames_record_in_both_sinks() {
        let (width, height, fps) = (32u32, 16u32, 30u32);
        let pixels = width as usize * height as usize;
        let nv12 = CapturedPixels::Nv12(vec![128; pixels * 3 / 2]);
        let yuyv = CapturedPixels::Yuyv(vec![128; pixels * 2]);

        let path = temp_path("ycbcr");
        let mut pipeline = RecordingPipeline::new_av1(&path, width, height, fps, UVC_YCBCR_COLOR)
            .expect("at least the software AV1 encoder must be constructible");
        for _ in 0..3 {
            pipeline
                .push(&captured_frame(
                    CapturedPixels::Nv12(vec![128; pixels * 3 / 2]),
                    UVC_YCBCR_COLOR,
                    width,
                    height,
                ))
                .expect("encoding an NV12 frame");
            pipeline
                .push(&captured_frame(
                    CapturedPixels::Yuyv(vec![128; pixels * 2]),
                    UVC_YCBCR_COLOR,
                    width,
                    height,
                ))
                .expect("encoding a YUYV frame");
        }
        pipeline.finish().expect("the container must finalize");
        let reader = VideoReader::open(&path).expect("recorded file must reopen");
        assert_eq!(reader.sample_count(), 6);
        assert_recording_color(&path, UVC_YCBCR_COLOR, width, height);
        std::fs::remove_file(&path).ok();

        let raw_path = temp_path("ycbcr-raw");
        let mut writer = RawVideoWriter::new(&raw_path, width, height, fps).expect("raw writer");
        writer
            .push_frame(&nv12, width, height, 0)
            .expect("NV12 raw frame");
        writer
            .push_frame(&yuyv, width, height, 1)
            .expect("YUYV raw frame");
        writer.finish().expect("raw writer finalize");
        let bytes = std::fs::read(&raw_path).unwrap();
        assert_eq!(bytes.len(), 20 + 2 * (12 + pixels * 4));
        std::fs::remove_file(&raw_path).ok();
    }

    /// Synthetic capture muxed into an MP4 that ffprobe/ffmpeg can decode.
    /// The artifact stays in the temp dir as the recorded-file evidence; a
    /// handful of frames already observes encode -> mux -> read-back.
    #[test]
    fn short_recording_writes_a_decodable_mp4() {
        let (width, height, fps, frames) = (640u32, 480u32, 30u32, 6usize);
        let path = std::env::temp_dir().join("waterkit-camera-evidence.mp4");
        let mut pipeline =
            RecordingPipeline::new_av1(&path, width, height, fps, super::super::JFIF_RGB_COLOR)
                .expect("at least the software AV1 encoder must be constructible");
        for index in 0..frames {
            pipeline
                .push(&captured_frame(
                    CapturedPixels::Rgba(synthetic_rgba(width as usize, height as usize, index)),
                    super::super::JFIF_RGB_COLOR,
                    width,
                    height,
                ))
                .expect("encoding a synthetic frame");
        }
        pipeline.finish().expect("the container must finalize");

        let reader = VideoReader::open(&path).expect("recorded file must reopen");
        assert_eq!(reader.dimensions(), (width, height));
        assert_eq!(reader.sample_count() as usize, frames);
        assert_recording_color(&path, super::super::JFIF_RGB_COLOR, width, height);
    }

    /// Raw recording read back through the WKRV frame records; the
    /// artifact stays in the temp dir as evidence.
    #[test]
    fn short_raw_recording_reads_back_its_frames() {
        let (width, height, fps, frames) = (640u32, 480u32, 30u32, 6usize);
        let path = std::env::temp_dir().join("waterkit-camera-evidence.wkrv");
        let mut writer = RawVideoWriter::new(&path, width, height, fps).expect("raw writer");
        for index in 0..frames {
            writer
                .push_frame(
                    &CapturedPixels::Rgba(synthetic_rgba(width as usize, height as usize, index)),
                    width,
                    height,
                    index as u64 * 33_333_333,
                )
                .expect("raw frame write");
        }
        writer.finish().expect("raw writer finalize");

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"WKRV");
        let frame_bytes = width as usize * height as usize * 4;
        let mut pos = 20;
        let mut count = 0usize;
        while pos + 12 <= bytes.len() {
            let len = u32::from_le_bytes(bytes[pos + 8..pos + 12].try_into().unwrap()) as usize;
            assert_eq!(len, frame_bytes);
            pos += 12 + len;
            count += 1;
        }
        assert_eq!(count, frames);
        assert_eq!(pos, bytes.len());
    }

    #[test]
    fn av1_codec_config_is_the_rav1e_configuration_record() {
        let (width, height) = (64u32, 48u32);
        let mut encoder = Encoder::new(
            CodecType::Av1,
            width,
            height,
            EncoderProfile::Offline,
            crate::VideoColorInfo::default(),
        )
        .expect("software AV1 encoder is always available on desktop");
        // rav1e builds the AV1CodecConfigurationRecord at construction — no
        // packet has to be encoded first.
        let av1c = encoder.codec_config().expect("AV1 exposes av1C");
        assert_eq!(av1c[0], 0x81);
        assert_eq!(av1c[1] >> 5, 0, "8-bit 4:2:0 encodes as AV1 profile 0");
        // chroma_subsampling_x/y set for profile 0
        assert_eq!(av1c[2] & 0x0c, 0x0c);

        // The first drained packet still carries the sequence header OBU the
        // muxer flags as the keyframe.
        let mut nv12 = vec![0u8; (width * height) as usize];
        nv12.extend(vec![128u8; (width * height / 2) as usize]);
        // rav1e queues submitted frames internally; every `encode_nv12`
        // yields nothing until `flush` drains the encoder.
        for _ in 0..8 {
            for item in encoder.encode_nv12(&nv12) {
                item.unwrap();
            }
        }
        let packet = encoder
            .flush()
            .expect("encoder drains")
            .into_iter()
            .next()
            .expect("first packet holds the sequence header");
        assert!(packet_contains_sequence_header(&packet));
    }
}

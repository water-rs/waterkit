//! One owner per Android camera session, so teardown runs exactly once.
//!
//! The Kotlin camera helper keeps per-camera session state that a consumer
//! sequences as `open_camera` → `start_capture` → `wait_for_frame`* →
//! `stop_capture` → `close_camera`. The types here make that sequence
//! ownership: an [`OpenCamera`] closes the camera when it drops, a
//! [`Capture`] stops capture before the camera closes, and a [`FrameThread`]
//! owns the capture so the reader thread alone tears it down. Dropping the
//! handle joins that thread, so teardown has finished by the time the drop
//! returns.

use std::sync::Arc;
use std::time::Duration;

use crate::clock::StreamClock;
use crate::{CameraError, Resolution};

/// The Kotlin camera helper's session calls.
pub trait CameraHelper: Send + Sync + 'static {
    /// A frame as `wait_for_frame` produces it.
    type Frame: Send + 'static;
    /// An analysis frame as `wait_for_analysis_frame` produces it.
    type Analysis: Send + 'static;

    /// Opens `camera_id` at `resolution` and `frame_rate`, capturing a
    /// second CPU-readable stream at `analysis`'s resolution when set.
    fn open_camera(
        &self,
        camera_id: &str,
        resolution: Resolution,
        frame_rate: u32,
        analysis: Option<Resolution>,
    ) -> Result<(), CameraError>;

    /// Starts the opened camera's capture session.
    fn start_capture(&self) -> Result<(), CameraError>;

    /// The next captured frame, or `None` when none arrived within
    /// `timeout_ms`. `clock` turns the frame's capture-clock reading into
    /// its stream timestamp.
    fn wait_for_frame(
        &self,
        clock: &StreamClock<Duration>,
        timeout_ms: i32,
    ) -> Result<Option<Self::Frame>, CameraError>;

    /// The next analysis frame, or `None` when none arrived within
    /// `timeout_ms` — including when the camera was opened without an
    /// analysis output, whose queue never fills.
    fn wait_for_analysis_frame(
        &self,
        clock: &StreamClock<Duration>,
        timeout_ms: i32,
    ) -> Result<Option<Self::Analysis>, CameraError>;

    /// Stops the capture session.
    fn stop_capture(&self) -> Result<(), CameraError>;

    /// Closes the camera.
    fn close_camera(&self) -> Result<(), CameraError>;
}

/// The helper's open camera. Dropping it closes the camera, which therefore
/// closes exactly once.
pub struct OpenCamera<H: CameraHelper> {
    helper: H,
}

impl<H: CameraHelper> OpenCamera<H> {
    /// Opens `camera_id` on `helper`, with an analysis stream at
    /// `analysis`'s resolution when set.
    pub fn open(
        helper: H,
        camera_id: &str,
        resolution: Resolution,
        frame_rate: u32,
        analysis: Option<Resolution>,
    ) -> Result<Self, CameraError> {
        helper.open_camera(camera_id, resolution, frame_rate, analysis)?;
        Ok(Self { helper })
    }

    /// Starts capture; on failure the camera closes as `self` drops.
    pub fn start_capture(self) -> Result<Capture<H>, CameraError> {
        self.helper.start_capture()?;
        Ok(Capture { camera: self })
    }
}

impl<H: CameraHelper> Drop for OpenCamera<H> {
    fn drop(&mut self) {
        if let Err(error) = self.helper.close_camera() {
            tracing::error!("camera close failed: {error}");
        }
    }
}

/// A running capture session. Dropping it stops capture, then (its field
/// dropping) closes the camera.
pub struct Capture<H: CameraHelper> {
    camera: OpenCamera<H>,
}

impl<H: CameraHelper> Capture<H> {
    /// The helper this capture runs on.
    pub const fn helper(&self) -> &H {
        &self.camera.helper
    }
}

impl<H: CameraHelper> Drop for Capture<H> {
    fn drop(&mut self) {
        if let Err(error) = self.camera.helper.stop_capture() {
            tracing::error!("camera stop failed: {error}");
        }
    }
}

/// What a reader thread waits on. A [`Capture`] owns the camera; a shared
/// helper only reads one of its streams.
enum Waiter<H: CameraHelper> {
    /// The owning reader: its drop tears the camera down.
    Capture(Capture<H>),
    /// A secondary reader, which must not tear the camera down.
    Shared(H),
}

impl<H: CameraHelper> Waiter<H> {
    const fn helper(&self) -> &H {
        match self {
            Self::Capture(capture) => capture.helper(),
            Self::Shared(helper) => helper,
        }
    }
}

/// How a reader takes the next frame of its stream out of the helper.
type Wait<H, F> = fn(&H, &StreamClock<Duration>, i32) -> Result<Option<F>, CameraError>;

/// What a starved reader drives so the camera's buffers come back: a frame's
/// buffer returns to the camera only when wgpu destroys the textures aliasing
/// it, and wgpu does that during device maintenance. A consumer awaiting the
/// next frame submits nothing, so when a wait comes back empty the reader
/// runs one non-blocking maintenance pass itself.
pub trait Reclaim: Send + 'static {
    /// Runs one non-blocking maintenance pass. A failure is a reader
    /// failure: it ends the stream like a failed wait does.
    fn reclaim(&self) -> Result<(), CameraError>;
}

impl Reclaim for Arc<wgpu::Device> {
    fn reclaim(&self) -> Result<(), CameraError> {
        self.poll(wgpu::PollType::Poll)
            .map_err(|error| CameraError::GpuError(format!("device poll: {error}")))?;
        Ok(())
    }
}

/// The thread reading one of a capture's frame streams. When it owns the
/// [`Capture`], it alone tears the camera down: when capture fails, or when
/// this handle drops; dropping the handle waits for that teardown.
pub struct FrameThread<F> {
    frames: async_channel::Receiver<Result<F, CameraError>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl<F: Send + 'static> FrameThread<F> {
    /// Spawns the reader thread for `capture`'s preview frames, waiting at
    /// most `timeout_ms` for each frame.
    pub fn spawn<H: CameraHelper<Frame = F>, R: Reclaim>(
        capture: Capture<H>,
        reclaim: R,
        timeout_ms: i32,
    ) -> Self {
        Self::reader(
            Waiter::Capture(capture),
            reclaim,
            timeout_ms,
            H::wait_for_frame,
        )
    }

    /// Spawns a reader on `helper`'s analysis stream. Unlike [`Self::spawn`]
    /// the thread owns no capture — the preview `FrameThread` tears the
    /// camera down; this thread only waits and sends.
    pub fn spawn_analysis<H: CameraHelper<Analysis = F>, R: Reclaim>(
        helper: H,
        reclaim: R,
        timeout_ms: i32,
    ) -> Self {
        Self::reader(
            Waiter::Shared(helper),
            reclaim,
            timeout_ms,
            H::wait_for_analysis_frame,
        )
    }

    /// The read loop every reader shares: `wait` the next frame out of the
    /// helper and forward it; newest wins, and a failure is the last item —
    /// the channel closes only after the waiter drops, which for a
    /// [`Waiter::Capture`] is the camera's teardown. An empty wait means the
    /// camera is starved, so the reader drives `reclaim` once.
    fn reader<H: CameraHelper, R: Reclaim>(
        waiter: Waiter<H>,
        reclaim: R,
        timeout_ms: i32,
        wait: Wait<H, F>,
    ) -> Self {
        let (sender, frames) = async_channel::bounded(1);
        let thread = std::thread::spawn(move || {
            let clock = StreamClock::new();
            let helper = waiter.helper();
            while !sender.is_closed() {
                match wait(helper, &clock, timeout_ms) {
                    Ok(Some(frame)) => {
                        if sender.force_send(Ok(frame)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        if let Err(error) = reclaim.reclaim() {
                            // Closed means the owner is gone.
                            let _ = sender.force_send(Err(error));
                            break;
                        }
                    }
                    Err(error) => {
                        // Closed means the owner is gone.
                        let _ = sender.force_send(Err(error));
                        break;
                    }
                }
            }
            // Teardown before the channel closes.
            drop(waiter);
            drop(sender);
        });
        Self {
            frames,
            thread: Some(thread),
        }
    }

    /// The reader's frame stream.
    pub const fn frames(&self) -> &async_channel::Receiver<Result<F, CameraError>> {
        &self.frames
    }
}

impl<F> Drop for FrameThread<F> {
    fn drop(&mut self) {
        self.frames.close();
        if let Some(thread) = self.thread.take()
            && let Err(payload) = thread.join()
            && !std::thread::panicking()
        {
            std::panic::resume_unwind(payload);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CameraHelper, FrameThread, OpenCamera, Reclaim};
    use crate::clock::StreamClock;
    use crate::{CameraError, Resolution};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    /// A helper that counts its session calls; `wait_for_frame` yields a
    /// frame on every call unless `fail_wait` was set, which it consumes.
    struct MockHelper {
        opens: AtomicUsize,
        starts: AtomicUsize,
        stops: AtomicUsize,
        closes: AtomicUsize,
        /// True once a close was observed to run after a stop.
        close_after_stop: AtomicBool,
        /// When set, `wait_for_frame` fails once.
        fail_wait: AtomicBool,
        /// When set, `wait_for_frame` returns no frame, as a starved
        /// camera's does.
        starved: AtomicBool,
        /// When set, `wait_for_frame` waits on it once before answering.
        gate: Option<Barrier>,
        /// Each reclaim pushes a token here when set, so a test can wait
        /// for one.
        reclaim_observer: Option<async_channel::Sender<()>>,
    }

    impl MockHelper {
        const FRAME: Resolution = Resolution::HD;

        fn new() -> Self {
            Self {
                opens: AtomicUsize::new(0),
                starts: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
                closes: AtomicUsize::new(0),
                close_after_stop: AtomicBool::new(false),
                fail_wait: AtomicBool::new(false),
                starved: AtomicBool::new(false),
                gate: None,
                reclaim_observer: None,
            }
        }

        fn counts(&self) -> (usize, usize, usize, usize) {
            (
                self.opens.load(Ordering::SeqCst),
                self.starts.load(Ordering::SeqCst),
                self.stops.load(Ordering::SeqCst),
                self.closes.load(Ordering::SeqCst),
            )
        }
    }

    impl CameraHelper for Arc<MockHelper> {
        type Frame = Resolution;
        type Analysis = Resolution;

        fn open_camera(
            &self,
            _camera_id: &str,
            _resolution: Resolution,
            _frame_rate: u32,
            _analysis: Option<Resolution>,
        ) -> Result<(), CameraError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn start_capture(&self) -> Result<(), CameraError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn wait_for_frame(
            &self,
            _clock: &StreamClock<Duration>,
            _timeout_ms: i32,
        ) -> Result<Option<Self::Frame>, CameraError> {
            if let Some(gate) = &self.gate {
                gate.wait();
            }
            if self.fail_wait.swap(false, Ordering::SeqCst) {
                return Err(CameraError::CaptureFailed("mock capture failure".into()));
            }
            if self.starved.load(Ordering::SeqCst) {
                return Ok(None);
            }
            std::thread::yield_now();
            Ok(Some(MockHelper::FRAME))
        }

        fn wait_for_analysis_frame(
            &self,
            _clock: &StreamClock<Duration>,
            _timeout_ms: i32,
        ) -> Result<Option<Self::Analysis>, CameraError> {
            std::thread::yield_now();
            Ok(None)
        }

        fn stop_capture(&self) -> Result<(), CameraError> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn close_camera(&self) -> Result<(), CameraError> {
            if self.stops.load(Ordering::SeqCst) > 0 {
                self.close_after_stop.store(true, Ordering::SeqCst);
            }
            self.closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    impl Reclaim for Arc<MockHelper> {
        fn reclaim(&self) -> Result<(), CameraError> {
            if let Some(observer) = &self.reclaim_observer {
                let _ = observer.send_blocking(());
            }
            Ok(())
        }
    }

    /// A helper whose `start_capture` fails; it counts its closes.
    struct FailingStart {
        closes: AtomicUsize,
    }

    impl CameraHelper for Arc<FailingStart> {
        type Frame = Resolution;
        type Analysis = Resolution;

        fn open_camera(
            &self,
            _camera_id: &str,
            _resolution: Resolution,
            _frame_rate: u32,
            _analysis: Option<Resolution>,
        ) -> Result<(), CameraError> {
            Ok(())
        }

        fn start_capture(&self) -> Result<(), CameraError> {
            Err(CameraError::StartFailed("mock start failure".into()))
        }

        fn wait_for_frame(
            &self,
            _clock: &StreamClock<Duration>,
            _timeout_ms: i32,
        ) -> Result<Option<Self::Frame>, CameraError> {
            unreachable!("no capture started, so no frame is waited for")
        }

        fn wait_for_analysis_frame(
            &self,
            _clock: &StreamClock<Duration>,
            _timeout_ms: i32,
        ) -> Result<Option<Self::Analysis>, CameraError> {
            unreachable!("no capture started, so no analysis frame is waited for")
        }

        fn stop_capture(&self) -> Result<(), CameraError> {
            unreachable!("no capture started, so none is stopped")
        }

        fn close_camera(&self) -> Result<(), CameraError> {
            self.closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A shared reader on the analysis stream joins on drop but never stops
    /// or closes the camera — the owning reader alone does that.
    #[test]
    fn a_shared_analysis_reader_never_tears_the_camera_down() {
        let helper = Arc::new(MockHelper::new());
        let capture = OpenCamera::open(Arc::clone(&helper), "0", Resolution::HD, 30, None)
            .expect("open succeeds")
            .start_capture()
            .expect("capture starts");
        let frames = FrameThread::spawn(capture, Arc::clone(&helper), 1);
        let analysis = FrameThread::spawn_analysis(Arc::clone(&helper), Arc::clone(&helper), 1);
        drop(analysis);
        let (_, _, stops, closes) = helper.counts();
        assert_eq!(
            (stops, closes),
            (0, 0),
            "the shared reader owns no teardown"
        );
        drop(frames);
        let (_, _, stops, closes) = helper.counts();
        assert_eq!((stops, closes), (1, 1), "the owning reader tears down once");
    }

    /// A wait that returns no frame means the camera is starved, so the
    /// reader drives the reclaim itself.
    #[test]
    fn an_empty_wait_drives_the_reclaim() {
        let (observer, reclaimed) = async_channel::unbounded();
        let helper = Arc::new(MockHelper {
            starved: AtomicBool::new(true),
            reclaim_observer: Some(observer),
            ..MockHelper::new()
        });
        let capture = OpenCamera::open(Arc::clone(&helper), "0", Resolution::HD, 30, None)
            .expect("open succeeds")
            .start_capture()
            .expect("capture starts");
        let frames = FrameThread::spawn(capture, Arc::clone(&helper), 1);
        reclaimed
            .recv_blocking()
            .expect("an empty wait triggers the reclaim");
        drop(frames);
        let (_, _, stops, closes) = helper.counts();
        assert_eq!((stops, closes), (1, 1), "the owning reader tears down once");
    }

    #[test]
    fn a_failed_start_closes_the_camera_once_without_stopping() {
        let helper = Arc::new(FailingStart {
            closes: AtomicUsize::new(0),
        });
        let opened = OpenCamera::open(Arc::clone(&helper), "0", Resolution::HD, 30, None)
            .expect("open succeeds");
        let result = opened.start_capture();
        assert!(matches!(result, Err(CameraError::StartFailed(_))));
        assert_eq!(helper.closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_the_handle_tears_down_once_before_drop_returns() {
        let helper = Arc::new(MockHelper::new());
        let capture = OpenCamera::open(Arc::clone(&helper), "0", Resolution::HD, 30, None)
            .expect("open succeeds")
            .start_capture()
            .expect("capture starts");
        let frame_thread = FrameThread::spawn(capture, Arc::clone(&helper), 1);
        drop(frame_thread);
        let (opens, starts, stops, closes) = helper.counts();
        assert_eq!(
            (opens, starts, stops, closes),
            (1, 1, 1, 1),
            "open→start→stop→close each ran exactly once"
        );
        assert!(helper.close_after_stop.load(Ordering::SeqCst));
    }

    #[test]
    fn a_capture_failure_ends_the_stream_after_teardown() {
        let helper = Arc::new(MockHelper::new());
        helper.fail_wait.store(true, Ordering::SeqCst);
        let capture = OpenCamera::open(Arc::clone(&helper), "0", Resolution::HD, 30, None)
            .expect("open succeeds")
            .start_capture()
            .expect("capture starts");
        let frame_thread = FrameThread::spawn(capture, Arc::clone(&helper), 1);
        let first = frame_thread.frames().recv_blocking();
        assert!(matches!(first, Ok(Err(CameraError::CaptureFailed(_)))));
        // The channel stays open while the camera closes, so the next
        // receive sees both.
        assert!(frame_thread.frames().recv_blocking().is_err());
        let (_, _, stops, closes) = helper.counts();
        assert_eq!((stops, closes), (1, 1));
        assert!(helper.close_after_stop.load(Ordering::SeqCst));
        drop(frame_thread);
        let (_, _, stops, closes) = helper.counts();
        assert_eq!((stops, closes), (1, 1), "teardown ran exactly once");
    }

    #[test]
    fn teardown_runs_once_when_the_owner_drops_as_the_thread_fails() {
        for _ in 0..500 {
            let helper = Arc::new(MockHelper {
                gate: Some(Barrier::new(2)),
                ..MockHelper::new()
            });
            helper.fail_wait.store(true, Ordering::SeqCst);
            let capture = OpenCamera::open(Arc::clone(&helper), "0", Resolution::HD, 30, None)
                .expect("open succeeds")
                .start_capture()
                .expect("capture starts");
            let frame_thread = FrameThread::spawn(capture, Arc::clone(&helper), 1);
            // Release the reader's wait_for_frame just as the owner drops.
            helper.gate.as_ref().unwrap().wait();
            drop(frame_thread);
            let (_, _, stops, closes) = helper.counts();
            assert_eq!((stops, closes), (1, 1));
            assert!(helper.close_after_stop.load(Ordering::SeqCst));
        }
    }
}

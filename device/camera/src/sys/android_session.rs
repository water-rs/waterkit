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

use std::time::Duration;

use crate::frame::StreamClock;
use crate::{CameraError, Resolution};

/// The Kotlin camera helper's session calls.
pub trait CameraHelper: Send + Sync + 'static {
    /// A frame as `wait_for_frame` produces it.
    type Frame: Send + 'static;

    /// Opens `camera_id` at `resolution` and `frame_rate`.
    fn open_camera(
        &self,
        camera_id: &str,
        resolution: Resolution,
        frame_rate: u32,
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
    /// Opens `camera_id` on `helper`.
    pub fn open(
        helper: H,
        camera_id: &str,
        resolution: Resolution,
        frame_rate: u32,
    ) -> Result<Self, CameraError> {
        helper.open_camera(camera_id, resolution, frame_rate)?;
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

/// The thread reading a capture's frames. It owns the [`Capture`], so it
/// alone tears the camera down: when capture fails, or when this handle
/// drops; dropping the handle waits for that teardown.
pub struct FrameThread<F> {
    frames: async_channel::Receiver<Result<F, CameraError>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl<F: Send + 'static> FrameThread<F> {
    /// Spawns the reader thread for `capture`, waiting at most `timeout_ms`
    /// for each frame.
    pub fn spawn<H: CameraHelper<Frame = F>>(capture: Capture<H>, timeout_ms: i32) -> Self {
        // Newest wins, and a failure is the last item: the channel closes
        // only after the camera has closed.
        let (sender, frames) = async_channel::bounded(1);
        let thread = std::thread::spawn(move || {
            let clock = StreamClock::new();
            while !sender.is_closed() {
                match capture.helper().wait_for_frame(&clock, timeout_ms) {
                    Ok(Some(frame)) => {
                        if sender.force_send(Ok(frame)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        // Closed means the owner is gone.
                        let _ = sender.force_send(Err(error));
                        break;
                    }
                }
            }
            // Teardown before the channel closes.
            drop(capture);
            drop(sender);
        });
        Self {
            frames,
            thread: Some(thread),
        }
    }

    /// The capture's frame stream.
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
    use super::{CameraHelper, FrameThread, OpenCamera};
    use crate::frame::StreamClock;
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
        /// When set, `wait_for_frame` waits on it once before answering.
        gate: Option<Barrier>,
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
                gate: None,
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

        fn open_camera(
            &self,
            _camera_id: &str,
            _resolution: Resolution,
            _frame_rate: u32,
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
            std::thread::yield_now();
            Ok(Some(MockHelper::FRAME))
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

    /// A helper whose `start_capture` fails; it counts its closes.
    struct FailingStart {
        closes: AtomicUsize,
    }

    impl CameraHelper for Arc<FailingStart> {
        type Frame = Resolution;

        fn open_camera(
            &self,
            _camera_id: &str,
            _resolution: Resolution,
            _frame_rate: u32,
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

        fn stop_capture(&self) -> Result<(), CameraError> {
            unreachable!("no capture started, so none is stopped")
        }

        fn close_camera(&self) -> Result<(), CameraError> {
            self.closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn a_failed_start_closes_the_camera_once_without_stopping() {
        let helper = Arc::new(FailingStart {
            closes: AtomicUsize::new(0),
        });
        let opened =
            OpenCamera::open(Arc::clone(&helper), "0", Resolution::HD, 30).expect("open succeeds");
        let result = opened.start_capture();
        assert!(matches!(result, Err(CameraError::StartFailed(_))));
        assert_eq!(helper.closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_the_handle_tears_down_once_before_drop_returns() {
        let helper = Arc::new(MockHelper::new());
        let capture = OpenCamera::open(Arc::clone(&helper), "0", Resolution::HD, 30)
            .expect("open succeeds")
            .start_capture()
            .expect("capture starts");
        let frame_thread = FrameThread::spawn(capture, 1);
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
        let capture = OpenCamera::open(Arc::clone(&helper), "0", Resolution::HD, 30)
            .expect("open succeeds")
            .start_capture()
            .expect("capture starts");
        let frame_thread = FrameThread::spawn(capture, 1);
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
            let capture = OpenCamera::open(Arc::clone(&helper), "0", Resolution::HD, 30)
                .expect("open succeeds")
                .start_capture()
                .expect("capture starts");
            let frame_thread = FrameThread::spawn(capture, 1);
            // Release the reader's wait_for_frame just as the owner drops.
            helper.gate.as_ref().unwrap().wait();
            drop(frame_thread);
            let (_, _, stops, closes) = helper.counts();
            assert_eq!((stops, closes), (1, 1));
            assert!(helper.close_after_stop.load(Ordering::SeqCst));
        }
    }
}

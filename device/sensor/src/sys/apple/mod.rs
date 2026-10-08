//! Apple platform (iOS/macOS) sensor implementation using objc2.

use crate::{ScalarData, SensorData, SensorError};
use waterkit_core::Timestamp;

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    use block2::RcBlock;
    use objc2_core_motion::{
        CMAccelerometerData, CMAltimeter, CMAltitudeData, CMGyroData, CMMagnetometerData,
        CMMotionManager,
    };
    use objc2_foundation::{NSError, NSNumber, NSOperationQueue};

    use core::pin::Pin;
    use core::task::{Context, Poll};
    use futures::stream;

    use super::{ScalarData, SensorData, SensorError, Timestamp};

    /// Update rate for the motion sensors, matching the previous 0.01s interval.
    const MOTION_UPDATE_INTERVAL: f64 = 0.01;

    /// Maps a `CoreMotion` callback's error to a `SensorError`.
    ///
    /// SAFETY: `error` must be a valid `NSError` pointer or NULL.
    unsafe fn callback_error(error: *mut NSError) -> SensorError {
        // SAFETY: Upheld by the caller.
        unsafe { error.as_ref() }.map_or_else(
            || SensorError::Platform("sensor update failed".to_owned()),
            |error| SensorError::Platform(error.localizedDescription().to_string()),
        )
    }

    /// Starts updates on a private `NSOperationQueue`, awaits the first sample
    /// (or the first handler error), then stops updates again.
    ///
    /// Every Objective-C handle lives inside a synchronous scope that yields
    /// only the result receiver, so nothing `!Send` crosses the `.await`.
    /// `CoreMotion` retains its own copies of the handler block (including the
    /// manager inside it) and the queue until updates stop.
    macro_rules! read_motion_sample {
        ($is_avail:ident, $set_interval:ident, $start:ident, $stop:ident, $data:ty, $map:expr) => {{
            let receiver = {
                let manager = unsafe { CMMotionManager::new() };
                if !unsafe { manager.$is_avail() } {
                    return Err(SensorError::NotAvailable);
                }
                unsafe { manager.$set_interval(MOTION_UPDATE_INTERVAL) };

                // Bounded at one so only the first handler invocation is seen.
                let (sender, receiver) = async_channel::bounded(1);
                let queue = NSOperationQueue::new();
                let queue_in_block = queue.clone();
                let stop_manager = manager.clone();
                let fired = AtomicBool::new(false);
                let map = $map;

                let handler = RcBlock::new(move |data: *mut $data, error: *mut NSError| {
                    let _keep_queue_alive = &queue_in_block;
                    if fired.swap(true, Ordering::SeqCst) {
                        return;
                    }
                    // SAFETY: Core Motion passes a valid data pointer or NULL.
                    let result = unsafe { data.as_ref() }
                        .map(|data| {
                            let (x, y, z) = map(data);
                            SensorData::new(x, y, z, Timestamp::now())
                        })
                        // SAFETY: `error` is a valid NSError pointer or NULL.
                        .ok_or_else(|| unsafe { callback_error(error) });
                    drop(sender.try_send(result));
                    unsafe { stop_manager.$stop() };
                });

                unsafe { manager.$start(&queue, RcBlock::as_ptr(&handler).cast()) };
                receiver
            };

            receiver.recv().await.unwrap_or_else(|_| {
                Err(SensorError::Platform(
                    "sensor update channel closed".to_owned(),
                ))
            })
        }};
    }

    pub fn accelerometer_available() -> bool {
        let manager = unsafe { CMMotionManager::new() };
        unsafe { manager.isAccelerometerAvailable() }
    }

    pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
        read_motion_sample!(
            isAccelerometerAvailable,
            setAccelerometerUpdateInterval,
            startAccelerometerUpdatesToQueue_withHandler,
            stopAccelerometerUpdates,
            CMAccelerometerData,
            |data: &CMAccelerometerData| {
                let acceleration = unsafe { data.acceleration() };
                (acceleration.x, acceleration.y, acceleration.z)
            }
        )
    }

    pub fn gyroscope_available() -> bool {
        let manager = unsafe { CMMotionManager::new() };
        unsafe { manager.isGyroAvailable() }
    }

    pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
        read_motion_sample!(
            isGyroAvailable,
            setGyroUpdateInterval,
            startGyroUpdatesToQueue_withHandler,
            stopGyroUpdates,
            CMGyroData,
            |data: &CMGyroData| {
                let rate = unsafe { data.rotationRate() };
                (rate.x, rate.y, rate.z)
            }
        )
    }

    pub fn magnetometer_available() -> bool {
        let manager = unsafe { CMMotionManager::new() };
        unsafe { manager.isMagnetometerAvailable() }
    }

    pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
        read_motion_sample!(
            isMagnetometerAvailable,
            setMagnetometerUpdateInterval,
            startMagnetometerUpdatesToQueue_withHandler,
            stopMagnetometerUpdates,
            CMMagnetometerData,
            |data: &CMMagnetometerData| {
                let field = unsafe { data.magneticField() };
                (field.x, field.y, field.z)
            }
        )
    }

    pub fn barometer_available() -> bool {
        unsafe { CMAltimeter::isRelativeAltitudeAvailable() }
    }

    pub async fn barometer_read() -> Result<ScalarData, SensorError> {
        if !unsafe { CMAltimeter::isRelativeAltitudeAvailable() } {
            return Err(SensorError::NotAvailable);
        }

        // Same ownership shape as `read_motion_sample!`: the altimeter, the
        // queue and the handler stay inside a synchronous scope that yields
        // only the receiver.
        let receiver = {
            let altimeter = unsafe { CMAltimeter::new() };
            let (sender, receiver) = async_channel::bounded(1);
            let queue = NSOperationQueue::new();
            let queue_in_block = queue.clone();
            let altimeter_in_block = altimeter.clone();
            let fired = AtomicBool::new(false);

            let handler = RcBlock::new(move |data: *mut CMAltitudeData, error: *mut NSError| {
                let _keep_queue_alive = &queue_in_block;
                if fired.swap(true, Ordering::SeqCst) {
                    return;
                }
                // SAFETY: Core Motion passes a valid data pointer or NULL.
                let result = unsafe { data.as_ref() }
                    .map(|data| {
                        // SAFETY: `data` is valid; `pressure` is a retained NSNumber.
                        let pressure: objc2::rc::Retained<NSNumber> = unsafe { data.pressure() };
                        // CoreMotion reports kPa; the API exposes hPa.
                        let value = pressure.doubleValue() * 10.0;
                        ScalarData::new(value, Timestamp::now())
                    })
                    // SAFETY: `error` is a valid NSError pointer or NULL.
                    .ok_or_else(|| unsafe { callback_error(error) });
                drop(sender.try_send(result));
                unsafe { altimeter_in_block.stopRelativeAltitudeUpdates() };
            });

            unsafe {
                altimeter.startRelativeAltitudeUpdatesToQueue_withHandler(
                    &queue,
                    RcBlock::as_ptr(&handler).cast(),
                );
            }
            receiver
        };

        receiver.recv().await.unwrap_or_else(|_| {
            Err(SensorError::Platform(
                "sensor update channel closed".to_owned(),
            ))
        })
    }

    // Ambient light is not exposed via public API on iOS.
    #[expect(
        clippy::missing_const_for_fn,
        reason = "keeps the sys-impl signature uniform across sensors"
    )]
    pub fn ambient_light_available() -> bool {
        false
    }

    #[expect(
        clippy::unused_async,
        reason = "keeps the sys-impl signature uniform across sensors"
    )]
    pub async fn ambient_light_read() -> Result<ScalarData, SensorError> {
        Err(SensorError::NotAvailable)
    }

    /// One running `CoreMotion` watch: every `!Send` Objective-C object lives on
    /// a dedicated worker thread for the stream's lifetime. The update handler
    /// forwards each sample or error into an unbounded channel; dropping the
    /// stream signals the worker, which stops updates and releases everything.
    pub struct WatchStream<T> {
        receiver: Pin<Box<async_channel::Receiver<Result<T, SensorError>>>>,
        stop: std::sync::mpsc::Sender<()>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl<T> Drop for WatchStream<T> {
        fn drop(&mut self) {
            let _ = self.stop.send(());
            if let Some(worker) = self.worker.take() {
                drop(worker.join());
            }
        }
    }

    impl<T> futures_core::Stream for WatchStream<T> {
        type Item = Result<T, SensorError>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.receiver.as_mut().poll_next(cx)
        }
    }

    /// Spawns the worker thread that runs `run`: it creates the manager, queue
    /// and handler, starts updates, waits for the stop signal, then stops
    /// updates. Only `Send` values cross the spawn boundary.
    fn spawn_watch<T: Send + 'static>(
        run: impl FnOnce(async_channel::Sender<Result<T, SensorError>>, std::sync::mpsc::Receiver<()>)
        + Send
        + 'static,
    ) -> WatchStream<T> {
        let (sender, receiver) = async_channel::unbounded();
        let receiver = Box::pin(receiver);
        let (stop, stop_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || run(sender, stop_rx));
        WatchStream {
            receiver,
            stop,
            worker: Some(worker),
        }
    }

    /// Starts one `CMMotionManager` at the requested interval with
    /// `start*UpdatesToQueue:withHandler:` and streams every handler result.
    macro_rules! watch_motion {
        ($interval:expr, $set_interval:ident, $start:ident, $stop:ident, $data:ty, $map:expr) => {{
            spawn_watch(move |sender, stop_rx| {
                let manager = unsafe { CMMotionManager::new() };
                unsafe { manager.$set_interval($interval) };
                let queue = NSOperationQueue::new();
                let queue_in_block = queue.clone();
                let map = $map;
                let handler = RcBlock::new(move |data: *mut $data, error: *mut NSError| {
                    let _keep_queue_alive = &queue_in_block;
                    // SAFETY: Core Motion passes a valid data pointer or NULL.
                    let result = unsafe { data.as_ref() }
                        .map(|data| {
                            let (x, y, z) = map(data);
                            SensorData::new(x, y, z, Timestamp::now())
                        })
                        // SAFETY: `error` is a valid NSError pointer or NULL.
                        .ok_or_else(|| unsafe { callback_error(error) });
                    // Unbounded: never blocks; a closed receiver drops the send.
                    drop(sender.try_send(result));
                });
                unsafe { manager.$start(&queue, RcBlock::as_ptr(&handler).cast()) };
                let _ = stop_rx.recv();
                unsafe { manager.$stop() };
            })
        }};
    }

    /// Requested update interval in seconds (`watch` takes milliseconds).
    fn interval_seconds(interval_ms: u32) -> f64 {
        f64::from(interval_ms) / 1000.0
    }

    pub fn accelerometer_watch(
        interval_ms: u32,
    ) -> impl futures_core::Stream<Item = Result<SensorData, SensorError>> + Send + 'static {
        watch_motion!(
            interval_seconds(interval_ms),
            setAccelerometerUpdateInterval,
            startAccelerometerUpdatesToQueue_withHandler,
            stopAccelerometerUpdates,
            CMAccelerometerData,
            |data: &CMAccelerometerData| {
                let acceleration = unsafe { data.acceleration() };
                (acceleration.x, acceleration.y, acceleration.z)
            }
        )
    }

    pub fn gyroscope_watch(
        interval_ms: u32,
    ) -> impl futures_core::Stream<Item = Result<SensorData, SensorError>> + Send + 'static {
        watch_motion!(
            interval_seconds(interval_ms),
            setGyroUpdateInterval,
            startGyroUpdatesToQueue_withHandler,
            stopGyroUpdates,
            CMGyroData,
            |data: &CMGyroData| {
                let rate = unsafe { data.rotationRate() };
                (rate.x, rate.y, rate.z)
            }
        )
    }

    pub fn magnetometer_watch(
        interval_ms: u32,
    ) -> impl futures_core::Stream<Item = Result<SensorData, SensorError>> + Send + 'static {
        watch_motion!(
            interval_seconds(interval_ms),
            setMagnetometerUpdateInterval,
            startMagnetometerUpdatesToQueue_withHandler,
            stopMagnetometerUpdates,
            CMMagnetometerData,
            |data: &CMMagnetometerData| {
                let field = unsafe { data.magneticField() };
                (field.x, field.y, field.z)
            }
        )
    }

    pub fn barometer_watch(
        _interval_ms: u32,
    ) -> impl futures_core::Stream<Item = Result<ScalarData, SensorError>> + Send + 'static {
        // `CMAltimeter` has no update-interval knob; CoreMotion picks the rate.
        spawn_watch(move |sender, stop_rx| {
            let altimeter = unsafe { CMAltimeter::new() };
            let queue = NSOperationQueue::new();
            let queue_in_block = queue.clone();
            let handler = RcBlock::new(move |data: *mut CMAltitudeData, error: *mut NSError| {
                let _keep_queue_alive = &queue_in_block;
                // SAFETY: Core Motion passes a valid data pointer or NULL.
                let result = unsafe { data.as_ref() }
                    .map(|data| {
                        // SAFETY: `data` is valid; `pressure` is a retained NSNumber.
                        let pressure: objc2::rc::Retained<NSNumber> = unsafe { data.pressure() };
                        // CoreMotion reports kPa; the API exposes hPa.
                        ScalarData::new(pressure.doubleValue() * 10.0, Timestamp::now())
                    })
                    // SAFETY: `error` is a valid NSError pointer or NULL.
                    .ok_or_else(|| unsafe { callback_error(error) });
                drop(sender.try_send(result));
            });
            unsafe {
                altimeter.startRelativeAltitudeUpdatesToQueue_withHandler(
                    &queue,
                    RcBlock::as_ptr(&handler).cast(),
                );
            }
            let _ = stop_rx.recv();
            unsafe { altimeter.stopRelativeAltitudeUpdates() };
        })
    }

    // Ambient light is not exposed via public API on iOS: no watch stream.
    pub fn ambient_light_watch(
        _interval_ms: u32,
    ) -> stream::Empty<Result<ScalarData, SensorError>> {
        stream::empty()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2_core_foundation::{CFDictionary, CFRetained};
    use objc2_io_kit::{
        IOConnectCallScalarMethod, IOObjectRelease, IOServiceClose, IOServiceGetMatchingService,
        IOServiceMatching, IOServiceOpen, io_connect_t, io_service_t, kIOMainPortDefault,
    };

    use futures::stream;

    use super::{ScalarData, SensorData, SensorError, Timestamp};

    // Accelerometer, gyroscope, magnetometer and barometer are not exposed via
    // public API on macOS.
    #[expect(
        clippy::missing_const_for_fn,
        reason = "keeps the sys-impl signature uniform across sensors"
    )]
    pub fn accelerometer_available() -> bool {
        false
    }

    #[expect(
        clippy::unused_async,
        reason = "keeps the sys-impl signature uniform across sensors"
    )]
    pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
        Err(SensorError::NotAvailable)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "keeps the sys-impl signature uniform across sensors"
    )]
    pub fn gyroscope_available() -> bool {
        false
    }

    #[expect(
        clippy::unused_async,
        reason = "keeps the sys-impl signature uniform across sensors"
    )]
    pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
        Err(SensorError::NotAvailable)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "keeps the sys-impl signature uniform across sensors"
    )]
    pub fn magnetometer_available() -> bool {
        false
    }

    #[expect(
        clippy::unused_async,
        reason = "keeps the sys-impl signature uniform across sensors"
    )]
    pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
        Err(SensorError::NotAvailable)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "keeps the sys-impl signature uniform across sensors"
    )]
    pub fn barometer_available() -> bool {
        false
    }

    #[expect(
        clippy::unused_async,
        reason = "keeps the sys-impl signature uniform across sensors"
    )]
    pub async fn barometer_read() -> Result<ScalarData, SensorError> {
        Err(SensorError::NotAvailable)
    }

    /// Returns the `AppleLMUController` service handle, which the caller must
    /// release with `IOObjectRelease`, or `None` when the Mac has no ambient
    /// light sensor.
    fn lmu_service() -> Option<io_service_t> {
        // SAFETY: `IOServiceMatching` takes a valid C string and returns a +1
        // retained dictionary; the `from_raw` cast keeps ownership while
        // upcasting to the immutable CFDictionary the callee consumes.
        // `IOServiceGetMatchingService` returns a +1 service handle (0 = none).
        let service = unsafe {
            let matching = IOServiceMatching(c"AppleLMUController".as_ptr()).map(|dict| {
                CFRetained::from_raw(CFRetained::into_raw(dict).cast::<CFDictionary>())
            });
            IOServiceGetMatchingService(kIOMainPortDefault, matching)
        };
        (service != 0).then_some(service)
    }

    pub fn ambient_light_available() -> bool {
        lmu_service().is_some_and(|service| {
            IOObjectRelease(service);
            true
        })
    }

    #[expect(
        clippy::unused_async,
        reason = "keeps the sys-impl signature uniform across sensors"
    )]
    pub async fn ambient_light_read() -> Result<ScalarData, SensorError> {
        let Some(service) = lmu_service() else {
            return Err(SensorError::NotAvailable);
        };

        let mut conn: io_connect_t = 0;
        // SAFETY: `service` is a live io_service_t, `mach_task_self()` yields
        // the current task port, and `conn` is a valid out-pointer.
        let open =
            unsafe { IOServiceOpen(service, mach2::traps::mach_task_self(), 0, &raw mut conn) };
        IOObjectRelease(service);
        if open != mach2::kern_return::KERN_SUCCESS {
            return Err(SensorError::PermissionDenied);
        }

        let mut outputs = [0u64; 2];
        let mut count: u32 = 2;
        // SAFETY: `conn` was opened above, the null input pointer is paired
        // with a zero count, and `outputs`/`count` are valid out-pointers.
        let call = unsafe {
            IOConnectCallScalarMethod(
                conn,
                0,
                std::ptr::null(),
                0,
                outputs.as_mut_ptr(),
                &raw mut count,
            )
        };
        IOServiceClose(conn);
        if call != mach2::kern_return::KERN_SUCCESS {
            return Err(SensorError::NotAvailable);
        }

        // Average the left/right sensors.
        #[expect(
            clippy::cast_precision_loss,
            reason = "sensor counts fit in an f64 mantissa on every Mac with an LMU"
        )]
        let average = (outputs[0] + outputs[1]) as f64 / 2.0;
        Ok(ScalarData::new(average, Timestamp::now()))
    }

    // Accelerometer, gyroscope, magnetometer and barometer are not exposed via
    // public API on macOS: no watch streams.
    pub fn accelerometer_watch(
        _interval_ms: u32,
    ) -> stream::Empty<Result<SensorData, SensorError>> {
        stream::empty()
    }

    pub fn gyroscope_watch(_interval_ms: u32) -> stream::Empty<Result<SensorData, SensorError>> {
        stream::empty()
    }

    pub fn magnetometer_watch(_interval_ms: u32) -> stream::Empty<Result<SensorData, SensorError>> {
        stream::empty()
    }

    pub fn barometer_watch(_interval_ms: u32) -> stream::Empty<Result<ScalarData, SensorError>> {
        stream::empty()
    }

    /// The LMU has no push primitive, so ambient light keeps polling — the
    /// item type matches every other platform's watch contract.
    pub fn ambient_light_watch(
        interval_ms: u32,
    ) -> impl futures_core::Stream<Item = Result<ScalarData, SensorError>> + Send + 'static {
        let interval = std::time::Duration::from_millis(u64::from(interval_ms));
        stream::unfold((), move |()| async move {
            futures_timer::Delay::new(interval).await;
            Some((ambient_light_read().await, ()))
        })
    }
}

// Accelerometer
pub fn accelerometer_available() -> bool {
    imp::accelerometer_available()
}

pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
    imp::accelerometer_read().await
}

pub fn accelerometer_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = Result<SensorData, SensorError>> + Send, SensorError> {
    if !accelerometer_available() {
        return Err(SensorError::NotAvailable);
    }
    Ok(imp::accelerometer_watch(interval_ms))
}

// Gyroscope
pub fn gyroscope_available() -> bool {
    imp::gyroscope_available()
}

pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
    imp::gyroscope_read().await
}

pub fn gyroscope_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = Result<SensorData, SensorError>> + Send, SensorError> {
    if !gyroscope_available() {
        return Err(SensorError::NotAvailable);
    }
    Ok(imp::gyroscope_watch(interval_ms))
}

// Magnetometer
pub fn magnetometer_available() -> bool {
    imp::magnetometer_available()
}

pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
    imp::magnetometer_read().await
}

pub fn magnetometer_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = Result<SensorData, SensorError>> + Send, SensorError> {
    if !magnetometer_available() {
        return Err(SensorError::NotAvailable);
    }
    Ok(imp::magnetometer_watch(interval_ms))
}

// Barometer
pub fn barometer_available() -> bool {
    imp::barometer_available()
}

pub async fn barometer_read() -> Result<ScalarData, SensorError> {
    imp::barometer_read().await
}

pub fn barometer_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = Result<ScalarData, SensorError>> + Send, SensorError> {
    if !barometer_available() {
        return Err(SensorError::NotAvailable);
    }
    Ok(imp::barometer_watch(interval_ms))
}

// Ambient Light
pub fn ambient_light_available() -> bool {
    imp::ambient_light_available()
}

pub async fn ambient_light_read() -> Result<ScalarData, SensorError> {
    imp::ambient_light_read().await
}

pub fn ambient_light_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = Result<ScalarData, SensorError>> + Send, SensorError> {
    if !ambient_light_available() {
        return Err(SensorError::NotAvailable);
    }
    Ok(imp::ambient_light_watch(interval_ms))
}

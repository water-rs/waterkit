//! Bounded stream scanning with temporal tracking.
//!
//! [`BarcodeScanner`] owns a dedicated worker thread. [`BarcodeScanner::submit`]
//! hands it owned frames through a channel of capacity **1** — a stale
//! frame is dropped in favour of the newest one, so in-flight work is
//! always bounded and frames never accumulate in a queue.
//!
//! Per decoded symbol the worker maintains a track keyed by
//! `(symbology, payload)`. The same code seen again updates its track;
//! a code absent for longer than `track_ttl` (measured in *frame
//! timestamps*, not wall clock) is expired with [`ScanEvent::Lost`].
//!
//! Events are delivered on a bounded channel; a slow consumer slows the
//! worker (backpressure) rather than dropping events.

use std::collections::HashMap;
use std::thread::JoinHandle;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use futures::Stream;
use waterkit_core::Timestamp;

use crate::barcode::Symbology;
use crate::error::VisionError;
use crate::frame::FrameBuf;
use crate::{Barcode, BarcodeEngine};

/// Capacity of the frame channel: the worker plus one pending slot.
const IN_FLIGHT: usize = 1;
/// Capacity of the event channel.
const EVENT_CAPACITY: usize = 256;

/// Opaque identifier of one tracked code instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrackId(pub u64);

/// Tuning for [`BarcodeScanner`].
#[derive(Debug, Clone, Copy)]
pub struct ScanConfig {
    /// Minimum spacing between decode passes. `Duration::ZERO` decodes
    /// every submitted frame.
    pub min_interval: Duration,
    /// How long a code may go unseen (in frame-timestamp space) before
    /// its track is expired with [`ScanEvent::Lost`].
    pub track_ttl: Duration,
    /// Distance in pixels the centroid of a track's quadrilateral must
    /// move before an [`ScanEvent::Updated`] is emitted.
    pub move_threshold: f64,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            min_interval: Duration::ZERO,
            track_ttl: Duration::from_millis(1500),
            move_threshold: 8.0,
        }
    }
}

impl ScanConfig {
    /// Builder-style override of `min_interval`.
    #[must_use]
    pub const fn with_min_interval(mut self, interval: Duration) -> Self {
        self.min_interval = interval;
        self
    }

    /// Builder-style override of `track_ttl`.
    #[must_use]
    pub const fn with_track_ttl(mut self, ttl: Duration) -> Self {
        self.track_ttl = ttl;
        self
    }

    /// Builder-style override of `move_threshold`.
    #[must_use]
    pub const fn with_move_threshold(mut self, px: f64) -> Self {
        self.move_threshold = px;
        self
    }
}

/// One tracked barcode with its observation history.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct TrackedBarcode {
    /// Stable identifier while the track is alive.
    pub id: TrackId,
    /// Latest decode of this symbol.
    pub barcode: Barcode,
    /// Frame timestamp of first detection.
    pub first_seen: Timestamp,
    /// Frame timestamp of latest detection.
    pub last_seen: Timestamp,
    /// Number of frames in which this code was decoded.
    pub sightings: u32,
}

/// Emitted by the scanner for each lifecycle change of a tracked code.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ScanEvent {
    /// A code appeared (first sighting of a `(symbology, payload)` pair
    /// not currently tracked).
    Detected(TrackedBarcode),
    /// A still-tracked code moved more than `move_threshold` pixels.
    Updated(TrackedBarcode),
    /// A tracked code was not seen for `track_ttl`.
    Lost(TrackedBarcode),
}

impl ScanEvent {
    /// The tracked barcode this event concerns.
    #[must_use]
    pub fn tracked(&self) -> &TrackedBarcode {
        match self {
            Self::Detected(t) | Self::Updated(t) | Self::Lost(t) => t,
        }
    }
}

/// A bounded, paced, deduplicating barcode stream handle.
///
/// `Send + Sync`; the event stream is `Send` and `'static`. Dropping the
/// handle stops the worker (equivalent to [`BarcodeScanner::stop`] without
/// awaiting).
#[derive(Debug)]
pub struct BarcodeScanner {
    tx: Sender<FrameBuf>,
    events: Receiver<ScanEvent>,
    worker: Option<JoinHandle<()>>,
}

impl BarcodeScanner {
    /// Start a scanner with default configuration.
    #[must_use]
    pub fn new(engine: BarcodeEngine) -> Self {
        Self::with_config(engine, ScanConfig::default())
    }

    /// Start a scanner with explicit configuration.
    #[must_use]
    pub fn with_config(engine: BarcodeEngine, config: ScanConfig) -> Self {
        let (tx, rx) = async_channel::bounded::<FrameBuf>(IN_FLIGHT);
        let (ev_tx, ev_rx) = async_channel::bounded::<ScanEvent>(EVENT_CAPACITY);
        let worker = std::thread::Builder::new()
            .name("waterkit-vision-scanner".into())
            .spawn(move || worker_loop(rx, ev_tx, engine, config))
            .expect("failed to spawn vision scanner thread");
        Self {
            tx,
            events: ev_rx,
            worker: Some(worker),
        }
    }

    /// Offer a frame to the scanner.
    ///
    /// If a previous frame is still pending it is discarded — the worker
    /// always decodes the latest submitted frame.
    ///
    /// # Errors
    /// [`VisionError::ScannerClosed`] after [`BarcodeScanner::stop`].
    pub fn submit(&self, frame: FrameBuf) -> Result<(), VisionError> {
        if self.tx.is_closed() {
            return Err(VisionError::ScannerClosed);
        }
        // Drop a stale pending frame so the newest one is processed.
        if self.tx.is_full() {
            let _ = self.tx.try_recv();
        }
        self.tx
            .try_send(frame)
            .map_err(|_| VisionError::ScannerClosed)
    }

    /// Stream of scan events. Additional calls return new receivers over
    /// the same broadcast channel (cloned receivers see all events).
    #[must_use]
    pub fn events(&self) -> impl Stream<Item = ScanEvent> + Send + 'static {
        self.events.clone()
    }

    /// Stop the scanner: drains the event channel and joins the worker.
    ///
    /// Any tracks still alive are expired with [`ScanEvent::Lost`] first,
    /// then the event channel closes, so this future resolves once the
    /// worker thread has exited.
    pub async fn stop(mut self) {
        self.tx.close();
        // Drains all remaining events; completes when the worker exits
        // (its event sender is dropped), so the join below is instant.
        while self.events.recv().await.is_ok() {}
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for BarcodeScanner {
    fn drop(&mut self) {
        self.tx.close();
        // The worker exits after the channel closes; do not join on Drop.
        let _ = self.worker.take();
    }
}

/// One live track.
struct Track {
    id: TrackId,
    barcode: Barcode,
    first_seen: Timestamp,
    last_seen: Timestamp,
    sightings: u32,
}

impl Track {
    fn view(&self) -> TrackedBarcode {
        TrackedBarcode {
            id: self.id,
            barcode: self.barcode.clone(),
            first_seen: self.first_seen,
            last_seen: self.last_seen,
            sightings: self.sightings,
        }
    }
}

fn worker_loop(
    rx: Receiver<FrameBuf>,
    events: Sender<ScanEvent>,
    engine: BarcodeEngine,
    config: ScanConfig,
) {
    let mut tracks: HashMap<(Symbology, Vec<u8>), Track> = HashMap::new();
    let mut next_id = 1u64;
    let mut last_decode = std::time::Instant::now() - config.min_interval;

    while let Ok(frame) = rx.recv_blocking() {
        // Pacing: sleep off the remainder of the interval.
        let elapsed = last_decode.elapsed();
        if elapsed < config.min_interval {
            std::thread::sleep(config.min_interval - elapsed);
        }
        let now_ts = frame.timestamp();
        let cpu = frame.as_cpu_frame();
        let report = match engine.decode_report(&cpu) {
            Ok(r) => r,
            Err(_) => continue,
        };
        last_decode = std::time::Instant::now();

        let mut seen_keys = Vec::with_capacity(report.barcodes.len());
        for barcode in report.barcodes {
            let key = (barcode.symbology, barcode.raw.clone());
            seen_keys.push(key.clone());
            match tracks.get_mut(&key) {
                Some(track) => {
                    let moved = track.barcode.quad.center().distance(barcode.quad.center())
                        > config.move_threshold;
                    track.barcode = barcode;
                    track.last_seen = now_ts;
                    track.sightings += 1;
                    if moved {
                        let _ = events.send_blocking(ScanEvent::Updated(track.view()));
                    }
                }
                None => {
                    let track = Track {
                        id: TrackId(next_id),
                        barcode,
                        first_seen: now_ts,
                        last_seen: now_ts,
                        sightings: 1,
                    };
                    next_id += 1;
                    let _ = events.send_blocking(ScanEvent::Detected(track.view()));
                    tracks.insert(key, track);
                }
            }
        }

        // Expire tracks whose timestamp staleness exceeds the ttl.
        let expired: Vec<(Symbology, Vec<u8>)> = tracks
            .iter()
            .filter(|(key, track)| {
                !seen_keys.contains(key) && stale(now_ts, track.last_seen, config.track_ttl)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            if let Some(track) = tracks.remove(&key) {
                let _ = events.send_blocking(ScanEvent::Lost(track.view()));
            }
        }
    }

    // Channel closed: expire every live track before exiting.
    for (_, track) in tracks {
        let _ = events.send_blocking(ScanEvent::Lost(track.view()));
    }
}

/// `last` precedes `now` by more than `ttl` (frame-timestamp space).
fn stale(now: Timestamp, last: Timestamp, ttl: Duration) -> bool {
    let diff_ns = now.as_nanosecond() - last.as_nanosecond();
    diff_ns > ttl.as_nanos() as i128
}

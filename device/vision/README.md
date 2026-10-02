# waterkit-vision

On-device vision primitives for Waterkit. Currently shipping the first
slice: an in-crate linear-barcode engine with a bounded stream API.

## Implemented

- **EAN-13 / UPC-A** decoding (UPC-A is the EAN-13 leading-zero subset):
  adaptive binarization, guard/digit decode with per-element tolerance,
  quiet-zone and mod-10 checksum validation, multi-code results ordered
  deterministically by source geometry. No false-positive fallbacks —
  rejected candidates surface structured `RejectReason` diagnostics via
  `BarcodeEngine::decode_report`.
- **`CpuFrame`**: borrowed typed CPU plane view — stride, format
  (Luma8, RGB/BGR, RGBA/BGRA, NV12, I420), dimensions, timestamp,
  orientation. Owned counterpart `FrameBuf`.
- **`BarcodeScanner`**: stream handle with bounded in-flight work (latest
  frame wins, channel capacity 1), explicit pacing (`min_interval`),
  cancellation (`stop()`), and temporal dedup/tracking of individual
  codes (`Detected` / `Updated` / `Lost`, keyed by symbology + payload,
  expiry measured in frame timestamps).

## Not implemented (explicit)

QR Code, Data Matrix, Aztec, PDF417, EAN-8, UPC-E, Code 128, Code 39,
ITF and all other symbologies are not decoded: they are absent from
`Formats`, so they cannot be requested. GPU frame input, a
`waterkit-camera` adapter and OCR are not part of this build.

## Threading & async

`BarcodeEngine` is an immutable value type (`Send + Sync`). `BarcodeScanner`
owns one worker thread; submission and events go through bounded
`async-channel` queues. The public API never blocks an async executor and
exposes no `Arc<Mutex>` state.

```rust,ignore
use waterkit_vision::{BarcodeEngine, BarcodeScanner, CpuFrame, ScanEvent};
use futures::StreamExt;

let scanner = BarcodeScanner::new(BarcodeEngine::new());
scanner.submit(frame_buf)?; // FrameBuf: owned CPU frame
let mut events = scanner.events();
while let Some(event) = events.next().await {
    match event {
        ScanEvent::Detected(t) => println!("found {}", t.barcode.text),
        ScanEvent::Updated(t) => println!("moved {}", t.barcode.text),
        ScanEvent::Lost(t) => println!("lost {}", t.barcode.text),
    }
}
scanner.stop().await;
```

# waterkit-background

Cross-platform background task scheduling for WaterKit.

## Supported Backends

- iOS: `BGAppRefreshTaskRequest`, `BGProcessingTaskRequest`, and iOS 26 `BGContinuedProcessingTaskRequest`.
- Android: `JobScheduler` scheduling support for app refresh, processing, and continued processing (queue strategy; no GPU resource support).
- Other platforms: explicit `NotSupported` behavior.

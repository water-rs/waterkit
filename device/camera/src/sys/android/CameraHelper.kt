package waterkit.camera

import android.content.Context
import android.graphics.ImageFormat
import android.graphics.Rect
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.DngCreator
import android.hardware.camera2.TotalCaptureResult
import android.hardware.camera2.params.DynamicRangeProfiles
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.hardware.HardwareBuffer
import android.hardware.display.DisplayManager
import android.media.Image
import android.media.ImageReader
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.media.MediaRecorder
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.Log
import android.util.Range
import android.util.Size
import android.view.Display
import android.view.Surface
import kotlin.math.roundToInt
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.Executor
import java.util.concurrent.LinkedBlockingDeque
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import waterkit.build.NativeCallback

/**
 * Camera helper for waterkit-camera crate.
 * Uses Camera2 API for camera enumeration, still capture and video recording.
 */
class CameraHelper(private val appContext: Context) {
    private companion object {
        private const val TAG = "WaterkitCamera"
        private const val DATA_SPACE_STANDARD_SHIFT = 16
        private const val DATA_SPACE_STANDARD_MASK = 0x3F
        private const val DATA_SPACE_RANGE_SHIFT = 27
        private const val DATA_SPACE_RANGE_MASK = 0x7

        /**
         * Preview images the consumer may hold at once: the newest waiting
         * in `frameQueue`, the one in the Rust reader channel, and the frames
         * whose `HardwareBuffer` the GPU still reads. The preview listener
         * acquires only while fewer are out, so a consumer that falls behind
         * makes the camera drop frames instead of running the reader dry.
         */
        private const val PREVIEW_MAX_IN_FLIGHT = 4

        /**
         * `acquireLatestImage` acquires the next image before closing the
         * stale ones it drains, so while it runs the reader hands out one
         * more than were out at entry: the pool needs a spare slot above
         * `PREVIEW_MAX_IN_FLIGHT`.
         */
        private const val PREVIEW_IMAGES = PREVIEW_MAX_IN_FLIGHT + 1

        /**
         * Analysis images the consumer may hold at once: the newest waiting
         * in `analysisQueue`, the one parked in the Rust reader's channel,
         * and the frames an application still holds while it analyzes them.
         * The analysis listener acquires only while fewer are out, so a
         * consumer that falls behind makes the camera drop frames instead of
         * running the reader dry.
         */
        private const val ANALYSIS_MAX_IN_FLIGHT = 4

        /**
         * `acquireLatestImage` acquires the next image before closing the
         * stale ones it drains, so while it runs the reader hands out one
         * more than were out at entry: the pool needs a spare slot above
         * `ANALYSIS_MAX_IN_FLIGHT`.
         */
        private const val ANALYSIS_IMAGES = ANALYSIS_MAX_IN_FLIGHT + 1

        private const val DYNAMIC_RANGE_SDR = 0
        private const val DYNAMIC_RANGE_HDR10 = 1
        private const val DYNAMIC_RANGE_HLG10 = 2
        private const val DYNAMIC_RANGE_DOLBY_VISION = 3
        private const val PLATFORM_DYNAMIC_RANGE_STANDARD = 1L

        private const val FLASH_OFF = 0
        private const val FLASH_ON = 1
        private const val FLASH_AUTO = 2
        private const val FLASH_TORCH = 3

        private const val STABILIZATION_OFF = 0
        private const val STABILIZATION_STANDARD = 1
        private const val STABILIZATION_CINEMATIC = 2

        private const val FOCUS_CONTINUOUS_AUTO = 0
        private const val FOCUS_AUTO = 1
        private const val FOCUS_MANUAL = 2
        private const val FOCUS_LOCKED = 3
    }

    private var cameraManager: CameraManager? = null
    private var currentCameraId: String? = null
    private var currentCharacteristics: CameraCharacteristics? = null

    private var cameraDevice: CameraDevice? = null
    private var captureSession: CameraCaptureSession? = null
    private var previewRequestBuilder: CaptureRequest.Builder? = null

    private var previewImageReader: ImageReader? = null
    /**
     * CPU-readable YUV_420_888 frames for image analysis, present only when
     * the camera was opened with an analysis output.
     */
    private var analysisImageReader: ImageReader? = null
    /** CPU-readable YUV frames for RAW video recording, present only while it runs. */
    private var rawVideoImageReader: ImageReader? = null
    private var stillImageReader: ImageReader? = null
    private var rawImageReader: ImageReader? = null

    private var mediaRecorder: MediaRecorder? = null
    private var recorderSurface: Surface? = null
    private var recordingStartElapsedRealtimeMs: Long = 0
    private var isRecording: Boolean = false
    private var rawVideoOutput: FileOutputStream? = null
    private var rawVideoDataSpace: Int? = null
    private var rawVideoRecordingStartElapsedRealtimeMs: Long = 0
    private var isRawVideoRecording: Boolean = false

    private var backgroundThread: HandlerThread? = null
    private var backgroundHandler: Handler? = null

    /**
     * One preview frame: the `Image` acquired from the GPU-sampled `PRIVATE`
     * reader, its `HardwareBuffer`, and the display rotation, in degrees,
     * when the frame arrived, plus its data space and active dynamic-range
     * profile. Together with the sensor orientation and lens facing the
     * rotation gives the frame's orientation.
     * Below API 33 the bridge reports `DATASPACE_UNKNOWN` (0), so each
     * unspecified data-space component uses the captured profile's default.
     * The profile is captured with the image so Rust does not look it up
     * after arrival.
     *
     * The receiver owns the frame and must call [close] once the GPU has
     * finished with `hardwareBuffer`: it returns the image to the reader,
     * freeing one of the `PREVIEW_MAX_IN_FLIGHT` slots, and drops this
     * handle's reference on the buffer. The receiver also closes
     * `hardwareBuffer` itself, once it has taken a reference of its own;
     * [close] then only releases the image. The pixels are never read on
     * the CPU.
     */
    class CapturedFrame(
        val image: Image,
        val hardwareBuffer: HardwareBuffer,
        val displayRotation: Int,
        val dataSpace: Int,
        val dynamicRangeProfile: Int,
        /** The image's sensor timestamp, the start of exposure. */
        val captureTimeNs: Long,
        /**
         * Runs after the image is closed, on whichever thread called
         * [close]; the reader bookkeeping lives behind it.
         */
        private val onClosed: () -> Unit,
    ) {
        private val closed = AtomicBoolean(false)

        /** Returns the image and the buffer reference to the reader, once. */
        fun close() {
            if (closed.compareAndSet(false, true)) {
                hardwareBuffer.close()
                image.close()
                onClosed()
            }
        }
    }

    private fun imageDataSpace(image: Image): Int =
        if (Build.VERSION.SDK_INT >= 33) image.dataSpace else 0

    private val frameQueue: LinkedBlockingDeque<CapturedFrame> = LinkedBlockingDeque(1)

    /**
     * One analysis image acquired from `analysisImageReader`: the
     * `YUV_420_888` `Image`, the display rotation in degrees when it
     * arrived, its data space, and its sensor timestamp.
     *
     * The receiver owns the frame and must call [close] once it has read
     * the image: it returns the image to the reader, freeing one of the
     * `ANALYSIS_MAX_IN_FLIGHT` slots. The queue closes one a consumer
     * never takes.
     */
    class AnalysisFrame(
        val image: Image,
        val displayRotation: Int,
        val dataSpace: Int,
        /** The image's sensor timestamp, the start of exposure. */
        val captureTimeNs: Long,
        /**
         * Runs after the image is closed, on whichever thread called
         * [close]; the reader bookkeeping lives behind it.
         */
        private val onClosed: () -> Unit,
    ) {
        private val closed = AtomicBoolean(false)

        /** Returns the image and its in-flight slot to the reader, once. */
        fun close() {
            if (closed.compareAndSet(false, true)) {
                image.close()
                onClosed()
            }
        }
    }

    /**
     * The newest pending analysis frame; a second frame evicts — and closes
     * — the one nobody took.
     */
    private val analysisQueue: LinkedBlockingDeque<AnalysisFrame> = LinkedBlockingDeque(1)

    /**
     * Preview images acquired from `previewImageReader` and not yet closed:
     * the one waiting in `frameQueue` plus every frame leased to the
     * consumer. Read and written only on the camera background thread,
     * except that `closeCamera` resets it once that thread has stopped.
     */
    private var previewImagesInFlight = 0

    /**
     * Frames the producer wrote while `previewImagesInFlight` was at
     * `PREVIEW_MAX_IN_FLIGHT`; they stay with the producer and are dropped.
     * Counted and logged on the camera background thread.
     */
    private var previewFramesDropped = 0

    /**
     * True while [drainPreviewReader] runs; the release a stale frame's
     * close posts back must not re-enter it.
     */
    private var previewDrainActive = false

    /**
     * Analysis images acquired from `analysisImageReader` and not yet
     * closed: the one waiting in `analysisQueue` plus every frame leased to
     * the consumer. Read and written only on the camera background thread,
     * except that `closeCamera` resets it once that thread has stopped.
     */
    private var analysisImagesInFlight = 0

    /**
     * Frames the producer wrote while `analysisImagesInFlight` was at
     * `ANALYSIS_MAX_IN_FLIGHT`; they stay with the producer and are dropped.
     * Counted and logged on the camera background thread.
     */
    private var analysisFramesDropped = 0

    /**
     * True while [drainAnalysisReader] runs; the release a stale frame's
     * close posts back must not re-enter it.
     */
    private var analysisDrainActive = false
    private val displayManager: DisplayManager =
        appContext.getSystemService(Context.DISPLAY_SERVICE) as DisplayManager
    /**
     * Delivers one operation's peer result exactly once: the device, session
     * and capture callbacks race each other and `closeCamera`, and only the
     * first answer may reach the `NativeCallback`.
     */
    private class PendingResult(private val callback: NativeCallback?) {
        private val answered = AtomicBoolean(false)

        fun complete(result: Any?) {
            if (answered.compareAndSet(false, true)) {
                callback?.complete(result)
            }
        }

        fun fail(message: String) {
            if (answered.compareAndSet(false, true)) {
                callback?.fail(message)
            }
        }
    }

    /**
     * One RAW still request: its newest `RAW_SENSOR` image and its capture
     * result build the DNG together, so both halves race to answer it.
     */
    private class RawPhotoResult(
        callback: NativeCallback,
        private val characteristics: CameraCharacteristics,
    ) {
        private val result = PendingResult(callback)
        private var image: Image? = null
        private var captureResult: TotalCaptureResult? = null

        @Synchronized
        fun onImage(image: Image) {
            this.image?.close()
            this.image = image
            maybeFinish()
        }

        @Synchronized
        fun onResult(result: TotalCaptureResult) {
            captureResult = result
            maybeFinish()
        }

        fun fail(message: String) {
            synchronized(this) {
                image?.close()
                image = null
            }
            result.fail(message)
        }

        private fun maybeFinish() {
            val image = image ?: return
            val result = captureResult ?: return
            try {
                val output = ByteArrayOutputStream()
                DngCreator(characteristics, result).use { creator ->
                    creator.writeImage(output, image)
                }
                image.close()
                this.image = null
                this.result.complete(output.toByteArray())
            } catch (error: Exception) {
                image.close()
                this.image = null
                fail("DNG write failed: ${error.message ?: error.javaClass.name}")
            }
        }
    }

    /**
     * In-flight peer operations, so `closeCamera` can answer them: the
     * single lock guards every slot — writes are rare and short.
     */
    private val requestLock = Any()
    private var pendingOpen: PendingResult? = null
    private var pendingSession: PendingResult? = null
    private var pendingPhoto: PendingResult? = null
    private var pendingRawPhoto: RawPhotoResult? = null

    private val rawVideoLock = Any()

    private var frameWidth: Int = 1280
    private var frameHeight: Int = 720
    private var frameRate: Int = 30

    @Volatile private var selectedDynamicRangeProfile: Int = DYNAMIC_RANGE_SDR
    private var selectedPlatformDynamicRangeProfile: Long = PLATFORM_DYNAMIC_RANGE_STANDARD
    private var selectedFlashMode: Int = FLASH_OFF
    private var selectedStabilizationMode: Int = STABILIZATION_OFF
    private var selectedZoomFactor: Float = 1.0f
    private var selectedExposureCompensationSteps: Int? = null
    private var selectedFocusMode: Int = FOCUS_CONTINUOUS_AUTO
    private var selectedManualFocusDistance: Float? = null

    private var cachedResolutions: IntArray = intArrayOf(1280, 720)
    private var cachedFrameRates: IntArray = intArrayOf(30)
    private var cachedZoomMin: Float = 1.0f
    private var cachedZoomMax: Float = 1.0f
    private var cachedSupportsExposureCompensation: Boolean = false
    private var cachedSupportsManualFocus: Boolean = false
    private var cachedSupportsManualWhiteBalance: Boolean = false
    private var cachedPlatformDynamicRanges: Map<Int, Long> = mapOf(
        DYNAMIC_RANGE_SDR to PLATFORM_DYNAMIC_RANGE_STANDARD,
    )
    private var cachedSupportsStandardStabilization: Boolean = false
    private var cachedSupportsCinematicStabilization: Boolean = false
    private var cachedHasFlash: Boolean = false
    private var cachedHasTorch: Boolean = false
    private var cachedSupportsRawPhoto: Boolean = false
    private var cachedSupportsRawVideo: Boolean = true
    private var cachedSupportsConcurrentMultiCamera: Boolean = false
    private var cachedMaxConcurrentCameras: Int = 1

    private data class CapabilitySnapshot(
        val resolutions: List<Size>,
        val frameRates: IntArray,
        val zoomMin: Float,
        val zoomMax: Float,
        val supportsExposureCompensation: Boolean,
        val supportsManualFocus: Boolean,
        val supportsManualWhiteBalance: Boolean,
        val dynamicRanges: IntArray,
        val platformDynamicRanges: Map<Int, Long>,
        val supportsStandardStabilization: Boolean,
        val supportsCinematicStabilization: Boolean,
        val hasFlash: Boolean,
        val hasTorch: Boolean,
        val supportsRawPhoto: Boolean,
        val supportsRawVideo: Boolean,
        val supportsConcurrentMultiCamera: Boolean,
        val maxConcurrentCameras: Int,
    )

    /**
     * List available cameras.
     * Returns array entries as [cameraId, displayName, isFrontFacing].
     */
    fun listCameras(): Array<Array<String>> {
        val manager = appContext.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        val cameras = mutableListOf<Array<String>>()

        for (cameraId in manager.cameraIdList) {
            val characteristics = manager.getCameraCharacteristics(cameraId)
            val facing = characteristics.get(CameraCharacteristics.LENS_FACING)
            val isFront = facing == CameraCharacteristics.LENS_FACING_FRONT
            val name = when (facing) {
                CameraCharacteristics.LENS_FACING_FRONT -> "Front Camera"
                CameraCharacteristics.LENS_FACING_BACK -> "Back Camera"
                CameraCharacteristics.LENS_FACING_EXTERNAL -> "External Camera"
                else -> "Camera"
            }
            cameras.add(arrayOf(cameraId, name, isFront.toString()))
        }

        return cameras.toTypedArray()
    }

    /**
     * Open a camera by ID with requested configuration; [callback] answers
     * once `CameraDevice.StateCallback` reports the open, a disconnect, or
     * an error.
     */
    fun openCamera(
        cameraId: String,
        requestedWidth: Int,
        requestedHeight: Int,
        requestedFrameRate: Int,
        analysisWidth: Int,
        analysisHeight: Int,
        callback: NativeCallback,
    ) {
        val pending = PendingResult(callback)
        closeCamera()
        synchronized(requestLock) {
            pendingOpen = pending
        }

        startBackgroundThread()
        val handler = backgroundHandler ?: run {
            Log.e(TAG, "background handler is not initialized")
            pending.fail("background handler is not initialized")
            return
        }

        try {
            val manager = appContext.getSystemService(Context.CAMERA_SERVICE) as CameraManager
            val characteristics = manager.getCameraCharacteristics(cameraId)
            val snapshot = queryCapabilitySnapshot(manager, cameraId, characteristics)

            cameraManager = manager
            currentCameraId = cameraId
            currentCharacteristics = characteristics

            cacheSnapshot(snapshot)

            val selectedSize = chooseNearestSize(
                requestedWidth.coerceAtLeast(1),
                requestedHeight.coerceAtLeast(1),
                snapshot.resolutions,
            )
            frameWidth = selectedSize.width
            frameHeight = selectedSize.height
            frameRate = chooseNearestFrameRate(requestedFrameRate.coerceIn(1, 240), snapshot.frameRates)

            selectedDynamicRangeProfile = DYNAMIC_RANGE_SDR
            selectedPlatformDynamicRangeProfile = PLATFORM_DYNAMIC_RANGE_STANDARD
            selectedFlashMode = FLASH_OFF
            selectedStabilizationMode = STABILIZATION_OFF
            selectedZoomFactor = 1.0f
            selectedExposureCompensationSteps = null
            selectedFocusMode = FOCUS_CONTINUOUS_AUTO
            selectedManualFocusDistance = null
            isRawVideoRecording = false
            rawVideoRecordingStartElapsedRealtimeMs = 0L

            // PRIVATE buffers for GPU sampling reach the GPU as they are; the
            // driver describes their YCbCr layout and encoding. The pool is
            // one slot wider than the number of images the consumer may hold
            // at once, so `acquireLatestImage` always has a slot to take.
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
                throw IllegalStateException(
                    "GPU-sampled camera frames need Android 10 (API 29); this device runs API ${Build.VERSION.SDK_INT}",
                )
            }
            previewImageReader = ImageReader.newInstance(
                frameWidth,
                frameHeight,
                ImageFormat.PRIVATE,
                PREVIEW_IMAGES,
                HardwareBuffer.USAGE_GPU_SAMPLED_IMAGE,
            )
            stillImageReader = ImageReader.newInstance(frameWidth, frameHeight, ImageFormat.JPEG, 2)
            // The analysis reader runs beside the preview at its own size;
            // the PRIVATE preview plus YUV_420_888 combination at preview
            // size is guaranteed on every hardware level.
            analysisImageReader = if (analysisWidth > 0 && analysisHeight > 0) {
                ImageReader.newInstance(
                    analysisWidth,
                    analysisHeight,
                    ImageFormat.YUV_420_888,
                    ANALYSIS_IMAGES,
                )
            } else {
                null
            }
            // RAW_SENSOR streams only come in the sizes the sensor reads
            // out, normally just its full array; a reader at the preview size
            // makes the whole capture session fail to configure.
            rawImageReader = if (snapshot.supportsRawPhoto) {
                val rawSize = characteristics
                    .get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
                    ?.getOutputSizes(ImageFormat.RAW_SENSOR)
                    ?.maxByOrNull { size -> size.width.toLong() * size.height }
                    ?: throw IllegalStateException("camera $cameraId reports RAW without a RAW_SENSOR size")
                ImageReader.newInstance(rawSize.width, rawSize.height, ImageFormat.RAW_SENSOR, 2)
            } else {
                null
            }

            previewImageReader?.setOnImageAvailableListener({ reader ->
                drainPreviewReader(reader, handler)
            }, handler)

            analysisImageReader?.setOnImageAvailableListener({ reader ->
                drainAnalysisReader(reader, handler)
            }, handler)

            rawImageReader?.setOnImageAvailableListener({ reader ->
                val image = reader.acquireLatestImage() ?: return@setOnImageAvailableListener
                val pending = synchronized(requestLock) { pendingRawPhoto }
                if (pending == null) {
                    image.close()
                } else {
                    pending.onImage(image)
                }
            }, handler)

            stillImageReader?.setOnImageAvailableListener({ reader ->
                val image = reader.acquireLatestImage() ?: return@setOnImageAvailableListener
                val pending = synchronized(requestLock) {
                    val current = pendingPhoto
                    if (current == null) {
                        image.close()
                        return@setOnImageAvailableListener
                    }
                    pendingPhoto = null
                    current
                }
                try {
                    if (image.format != ImageFormat.JPEG) {
                        pending.fail("unexpected still image format ${image.format}")
                        return@setOnImageAvailableListener
                    }
                    val plane = image.planes.firstOrNull() ?: run {
                        pending.fail("still image carries no plane")
                        return@setOnImageAvailableListener
                    }
                    val buffer = plane.buffer
                    val bytes = ByteArray(buffer.remaining())
                    buffer.get(bytes)
                    pending.complete(bytes)
                } catch (error: Exception) {
                    pending.fail("failed to process still image: ${error.message ?: error.javaClass.name}")
                } finally {
                    image.close()
                }
            }, handler)

            manager.openCamera(cameraId, object : CameraDevice.StateCallback() {
                override fun onOpened(camera: CameraDevice) {
                    cameraDevice = camera
                    synchronized(requestLock) {
                        if (pendingOpen === pending) {
                            pendingOpen = null
                        }
                    }
                    pending.complete(null)
                }

                override fun onDisconnected(camera: CameraDevice) {
                    Log.e(TAG, "Camera disconnected: $cameraId")
                    camera.close()
                    if (cameraDevice === camera) {
                        cameraDevice = null
                    }
                    synchronized(requestLock) {
                        if (pendingOpen === pending) {
                            pendingOpen = null
                        }
                    }
                    pending.fail("camera $cameraId disconnected")
                }

                override fun onError(camera: CameraDevice, error: Int) {
                    Log.e(TAG, "Camera open error for $cameraId: $error")
                    camera.close()
                    if (cameraDevice === camera) {
                        cameraDevice = null
                    }
                    synchronized(requestLock) {
                        if (pendingOpen === pending) {
                            pendingOpen = null
                        }
                    }
                    pending.fail("camera $cameraId open failed with error $error")
                }
            }, handler)
        } catch (error: SecurityException) {
            Log.e(TAG, "Missing camera permission", error)
            closeCamera()
            pending.fail("missing camera permission")
        } catch (error: Exception) {
            Log.e(TAG, "Failed to open camera", error)
            closeCamera()
            pending.fail("failed to open camera: ${error.message ?: error.javaClass.name}")
        }
    }

    /**
     * Start frame capture; [callback] answers once the capture session is
     * configured and the repeating request is running.
     */
    fun startCapture(callback: NativeCallback) {
        createCaptureSession(includeRecorderSurface = false, callback) {
            callback.complete(null)
        }
    }

    /**
     * Stop frame capture.
     */
    fun stopCapture() {
        captureSession?.close()
        captureSession = null
        previewRequestBuilder = null
    }

    /**
     * Capture a high-quality still image using Camera2 still-capture
     * pipeline; [callback] answers with the JPEG bytes.
     */
    fun capturePhoto(callback: NativeCallback) {
        val pending = PendingResult(callback)
        val session = captureSession ?: run {
            Log.e(TAG, "capturePhoto called before capture session start")
            pending.fail("capturePhoto called before capture session start")
            return
        }
        val device = cameraDevice ?: run {
            Log.e(TAG, "capturePhoto called before camera open")
            pending.fail("capturePhoto called before camera open")
            return
        }
        val stillReader = stillImageReader ?: run {
            Log.e(TAG, "capturePhoto called before still image reader init")
            pending.fail("capturePhoto called before still image reader init")
            return
        }
        val handler = backgroundHandler ?: run {
            Log.e(TAG, "capturePhoto called without background handler")
            pending.fail("capturePhoto called without background handler")
            return
        }

        synchronized(requestLock) {
            pendingPhoto?.fail("superseded by a newer photo capture")
            pendingPhoto = pending
        }

        try {
            val stillBuilder = device.createCaptureRequest(CameraDevice.TEMPLATE_STILL_CAPTURE)
            stillBuilder.addTarget(stillReader.surface)
            applyRequestControls(stillBuilder, forStillCapture = true)

            session.capture(
                stillBuilder.build(),
                object : CameraCaptureSession.CaptureCallback() {
                    override fun onCaptureFailed(
                        session: CameraCaptureSession,
                        request: CaptureRequest,
                        failure: android.hardware.camera2.CaptureFailure,
                    ) {
                        Log.e(TAG, "Still capture failed: $failure")
                        photoFailed(pending, "still capture failed: $failure")
                    }
                },
                handler,
            )
        } catch (error: Exception) {
            Log.e(TAG, "Failed to capture still image", error)
            photoFailed(pending, "failed to capture still image: ${error.message ?: error.javaClass.name}")
        }
    }

    private fun photoFailed(pending: PendingResult, message: String) {
        synchronized(requestLock) {
            if (pendingPhoto === pending) {
                pendingPhoto = null
            }
        }
        pending.fail(message)
    }

    /**
     * Capture RAW photo using RAW_SENSOR + DNG container; [callback]
     * answers with the DNG bytes once the newest RAW image and the capture
     * result have both arrived.
     */
    fun captureRawPhoto(callback: NativeCallback) {
        val session = captureSession ?: run {
            Log.e(TAG, "captureRawPhoto called before capture session start")
            callback.fail("captureRawPhoto called before capture session start")
            return
        }
        val device = cameraDevice ?: run {
            Log.e(TAG, "captureRawPhoto called before camera open")
            callback.fail("captureRawPhoto called before camera open")
            return
        }
        val reader = rawImageReader ?: run {
            Log.e(TAG, "captureRawPhoto called on camera without RAW_SENSOR support")
            callback.fail("captureRawPhoto called on camera without RAW_SENSOR support")
            return
        }
        val characteristics = currentCharacteristics ?: run {
            Log.e(TAG, "captureRawPhoto called without camera characteristics")
            callback.fail("captureRawPhoto called without camera characteristics")
            return
        }
        val handler = backgroundHandler ?: run {
            Log.e(TAG, "captureRawPhoto called without background handler")
            callback.fail("captureRawPhoto called without background handler")
            return
        }

        val pending = RawPhotoResult(callback, characteristics)
        synchronized(requestLock) {
            pendingRawPhoto?.fail("superseded by a newer RAW photo capture")
            pendingRawPhoto = pending
        }

        try {
            val request = device.createCaptureRequest(CameraDevice.TEMPLATE_STILL_CAPTURE)
            request.addTarget(reader.surface)
            applyRequestControls(request, forStillCapture = true)

            session.capture(
                request.build(),
                object : CameraCaptureSession.CaptureCallback() {
                    override fun onCaptureCompleted(
                        session: CameraCaptureSession,
                        request: CaptureRequest,
                        result: TotalCaptureResult,
                    ) {
                        pending.onResult(result)
                    }

                    override fun onCaptureFailed(
                        session: CameraCaptureSession,
                        request: CaptureRequest,
                        failure: android.hardware.camera2.CaptureFailure,
                    ) {
                        Log.e(TAG, "RAW still capture failed: $failure")
                        rawPhotoFailed(pending, "RAW still capture failed: $failure")
                    }
                },
                handler,
            )
        } catch (error: Exception) {
            Log.e(TAG, "Failed to capture RAW photo", error)
            rawPhotoFailed(pending, "failed to capture RAW photo: ${error.message ?: error.javaClass.name}")
        }
    }

    private fun rawPhotoFailed(pending: RawPhotoResult, message: String) {
        synchronized(requestLock) {
            if (pendingRawPhoto === pending) {
                pendingRawPhoto = null
            }
        }
        pending.fail(message)
    }

    /**
     * Start video recording via MediaRecorder; [callback] answers once the
     * recorder is running, or once a failure leaves the preview session
     * restored.
     */
    fun startRecording(path: String, callback: NativeCallback) {
        if (isRecording) {
            Log.e(TAG, "startRecording called while already recording")
            callback.fail("startRecording called while already recording")
            return
        }
        if (isRawVideoRecording) {
            Log.e(TAG, "startRecording called while RAW recording is active")
            callback.fail("startRecording called while RAW recording is active")
            return
        }

        if (!prepareRecorder(path)) {
            callback.fail("failed to prepare the recorder for $path")
            return
        }

        createCaptureSession(includeRecorderSurface = true, callback) {
            try {
                mediaRecorder?.start()
                recordingStartElapsedRealtimeMs = SystemClock.elapsedRealtime()
                isRecording = true
                callback.complete(null)
            } catch (error: Exception) {
                Log.e(TAG, "Failed to start MediaRecorder", error)
                releaseRecorder()
                stopCapture()
                // Restore the preview session before reporting the failure.
                createCaptureSession(includeRecorderSurface = false, callback) {
                    callback.fail(
                        "failed to start MediaRecorder: ${error.message ?: error.javaClass.name}",
                    )
                }
            }
        }
    }

    /**
     * Stop video recording; [callback] answers once the recorder has
     * stopped and the preview session is restored.
     */
    fun stopRecording(callback: NativeCallback) {
        if (!isRecording) {
            callback.complete(null)
            return
        }

        val recorder = mediaRecorder ?: run {
            Log.e(TAG, "stopRecording called with missing MediaRecorder")
            isRecording = false
            recordingStartElapsedRealtimeMs = 0
            callback.fail("stopRecording called with missing MediaRecorder")
            return
        }

        var stopError: String? = null
        try {
            recorder.stop()
        } catch (error: RuntimeException) {
            Log.e(TAG, "Failed to stop MediaRecorder cleanly", error)
            stopError = error.message ?: "MediaRecorder.stop failed"
        } catch (error: Exception) {
            Log.e(TAG, "Failed to stop MediaRecorder", error)
            stopError = error.message ?: "MediaRecorder.stop failed"
        }

        isRecording = false
        recordingStartElapsedRealtimeMs = 0
        releaseRecorder()

        stopCapture()
        createCaptureSession(includeRecorderSurface = false, callback) {
            when (val error = stopError) {
                null -> callback.complete(null)
                else -> callback.fail(error)
            }
        }
    }

    /**
     * Synchronously stops an active recording for teardown, where no
     * session restore is wanted because the camera is closing anyway.
     */
    private fun stopRecordingNow() {
        if (!isRecording) {
            return
        }
        try {
            mediaRecorder?.stop()
        } catch (error: Exception) {
            Log.e(TAG, "Failed to stop MediaRecorder during teardown", error)
        }
        isRecording = false
        recordingStartElapsedRealtimeMs = 0
        releaseRecorder()
    }
    fun getRecordingDurationMs(): Long {
        if (!isRecording || recordingStartElapsedRealtimeMs == 0L) {
            return 0L
        }
        return (SystemClock.elapsedRealtime() - recordingStartElapsedRealtimeMs).coerceAtLeast(0L)
    }

    /**
     * Start RAW video frame stream recording; [callback] answers once the
     * session carries the RAW output.
     */
    fun startRawRecording(path: String, callback: NativeCallback) {
        val outputPath = path.trim()
        if (outputPath.isEmpty()) {
            Log.e(TAG, "RAW recording path must not be empty")
            callback.fail("RAW recording path must not be empty")
            return
        }
        if (isRawVideoRecording) {
            Log.e(TAG, "startRawRecording called while already recording RAW")
            callback.fail("startRawRecording called while already recording RAW")
            return
        }
        if (isRecording) {
            Log.e(TAG, "startRawRecording called while standard recording is active")
            callback.fail("startRawRecording called while standard recording is active")
            return
        }

        try {
            val file = File(outputPath)
            if (file.exists() && !file.delete()) {
                callback.fail("failed to remove existing RAW output file: $outputPath")
                return
            }
            file.parentFile?.mkdirs()
            val stream = FileOutputStream(file)
            synchronized(rawVideoLock) {
                rawVideoOutput = stream
                rawVideoDataSpace = null
                rawVideoRecordingStartElapsedRealtimeMs = SystemClock.elapsedRealtime()
                isRawVideoRecording = true
            }
            // The preview frames are GPU-only, so RAW video reads its own
            // CPU-readable YUV stream, attached only while recording.
            val reader = ImageReader.newInstance(frameWidth, frameHeight, ImageFormat.YUV_420_888, 2)
            reader.setOnImageAvailableListener({ source ->
                val image = source.acquireLatestImage() ?: return@setOnImageAvailableListener
                try {
                    maybeWriteRawVideoFrame(image, SystemClock.elapsedRealtimeNanos())
                } finally {
                    image.close()
                }
            }, backgroundHandler)
            rawVideoImageReader = reader
        } catch (error: Exception) {
            Log.e(TAG, "Failed to start RAW recording", error)
            stopRawVideoRecordingNow()
            callback.fail("failed to start RAW recording: ${error.message ?: error.javaClass.name}")
            return
        }

        createCaptureSession(includeRecorderSurface = false, callback) {
            callback.complete(null)
        }
    }

    /**
     * Stop RAW video frame stream recording; [callback] answers once the
     * reader is off the session and the output file is closed.
     */
    fun stopRawRecording(callback: NativeCallback) {
        val reader = rawVideoImageReader
        rawVideoImageReader = null

        if (reader != null && cameraDevice != null) {
            // Reconfigure the session without the reader before closing it.
            createCaptureSession(includeRecorderSurface = false, callback) {
                reader.close()
                closeRawVideoOutput(callback)
            }
        } else {
            reader?.close()
            closeRawVideoOutput(callback)
        }
    }

    /**
     * Closes the RAW output stream and answers [callback] when one is set
     * (an internal teardown passes none).
     */
    private fun closeRawVideoOutput(callback: NativeCallback?) {
        val output = synchronized(rawVideoLock) {
            val stream = rawVideoOutput
            rawVideoOutput = null
            rawVideoDataSpace = null
            isRawVideoRecording = false
            rawVideoRecordingStartElapsedRealtimeMs = 0L
            stream
        }
        try {
            output?.flush()
            output?.close()
            callback?.complete(null)
        } catch (error: Exception) {
            Log.e(TAG, "Failed to stop RAW recording stream", error)
            callback?.fail("failed to stop RAW recording stream: ${error.message ?: error.javaClass.name}")
        }
    }

    /**
     * Synchronous teardown of RAW video recording for closeCamera, where
     * the session is being torn down anyway.
     */
    private fun stopRawVideoRecordingNow() {
        rawVideoImageReader?.close()
        rawVideoImageReader = null
        val output = synchronized(rawVideoLock) {
            val stream = rawVideoOutput
            rawVideoOutput = null
            rawVideoDataSpace = null
            isRawVideoRecording = false
            rawVideoRecordingStartElapsedRealtimeMs = 0L
            stream
        }
        try {
            output?.flush()
            output?.close()
        } catch (error: Exception) {
            Log.e(TAG, "Failed to stop RAW recording stream", error)
        }
    }
    fun getRawRecordingDurationMs(): Long {
        if (!isRawVideoRecording || rawVideoRecordingStartElapsedRealtimeMs == 0L) {
            return 0L
        }
        return (SystemClock.elapsedRealtime() - rawVideoRecordingStartElapsedRealtimeMs).coerceAtLeast(0L)
    }

    /**
     * Wait for the next available frame and consume it.
     * Returns null on timeout or if no frame is available.
     *
     * The returned frame keeps one of the `PREVIEW_MAX_IN_FLIGHT` slots
     * until its [CapturedFrame.close] runs.
     */
    fun waitForNextFrame(timeoutMs: Int): CapturedFrame? {
        return try {
            if (timeoutMs <= 0) {
                frameQueue.pollFirst()
            } else {
                frameQueue.pollFirst(timeoutMs.toLong(), TimeUnit.MILLISECONDS)
            }
        } catch (error: InterruptedException) {
            Thread.currentThread().interrupt()
            null
        }
    }

    /**
     * Wait for the next analysis frame and consume it.
     * Returns null on timeout or when the camera was opened without an
     * analysis output, whose queue never fills.
     *
     * The receiver owns the returned frame: [AnalysisFrame.close] returns
     * its image to the reader and frees one of the `ANALYSIS_MAX_IN_FLIGHT`
     * slots, re-arming acquisition.
     */
    fun waitForNextAnalysisFrame(timeoutMs: Int): AnalysisFrame? {
        return try {
            if (timeoutMs <= 0) {
                analysisQueue.pollFirst()
            } else {
                analysisQueue.pollFirst(timeoutMs.toLong(), TimeUnit.MILLISECONDS)
            }
        } catch (error: InterruptedException) {
            Thread.currentThread().interrupt()
            null
        }
    }

    /**
     * Takes the newest pending preview image while a slot is free, evicting
     * the queued frame nobody took — newest wins. Runs only on the camera
     * background thread; the counter it reads is mutated there alone.
     *
     * When `PREVIEW_MAX_IN_FLIGHT` images are already out this acquires
     * nothing, so `acquireLatestImage` always has a spare `PREVIEW_IMAGES`
     * slot: the pending frame stays with the producer and is dropped once a
     * leased image's close re-arms the drain through [releasePreviewImage].
     */
    private fun drainPreviewReader(reader: ImageReader, handler: Handler) {
        if (reader !== previewImageReader || previewDrainActive) {
            return
        }
        previewDrainActive = true
        try {
            while (true) {
                if (previewImagesInFlight >= PREVIEW_MAX_IN_FLIGHT) {
                    previewFramesDropped += 1
                    if (previewFramesDropped == 1) {
                        Log.i(
                            TAG,
                            "preview consumer holds $PREVIEW_MAX_IN_FLIGHT frames; " +
                                "dropping until one returns",
                        )
                    }
                    return
                }
                val image = reader.acquireLatestImage() ?: return
                previewImagesInFlight += 1
                if (previewFramesDropped != 0) {
                    Log.i(TAG, "preview resumed after dropping $previewFramesDropped frames")
                    previewFramesDropped = 0
                }
                val buffer = image.hardwareBuffer
                if (buffer == null) {
                    image.close()
                    previewImagesInFlight -= 1
                    throw IllegalStateException(
                        "a GPU-sampled camera image carries no HardwareBuffer",
                    )
                }
                frameWidth = image.width
                frameHeight = image.height
                // Newest wins: a frame nobody took yet goes back to the reader.
                frameQueue.pollLast()?.let { stale -> stale.close() }
                frameQueue.offerLast(
                    CapturedFrame(
                        image,
                        buffer,
                        displayRotationDegrees(),
                        imageDataSpace(image),
                        selectedDynamicRangeProfile,
                        image.timestamp,
                    ) {
                        releasePreviewImage(reader, handler)
                    },
                )
            }
        } finally {
            previewDrainActive = false
        }
    }

    /**
     * A leased preview image was closed on some thread: free one in-flight
     * slot, then re-check the reader for the newest image that waited for
     * it. Runs on the camera background thread — [close] on another thread
     * posts here; once the session's looper is gone the slot count has been
     * reset anyway, so a close during teardown is dropped.
     */
    private fun releasePreviewImage(reader: ImageReader, handler: Handler) {
        val released = Runnable {
            if (reader === previewImageReader) {
                previewImagesInFlight -= 1
                drainPreviewReader(reader, handler)
            }
        }
        if (handler.looper.isCurrentThread) {
            released.run()
        } else if (handler.looper.thread.isAlive) {
            handler.post(released)
        }
    }

    /**
     * Takes the newest pending analysis image while a slot is free,
     * evicting the queued frame nobody took — newest wins. Runs only on
     * the camera background thread; the counter it reads is mutated there
     * alone.
     *
     * When `ANALYSIS_MAX_IN_FLIGHT` images are already out this acquires
     * nothing, so `acquireLatestImage` always has a spare `ANALYSIS_IMAGES`
     * slot: the pending frame stays with the producer and is dropped once a
     * leased image's close re-arms the drain through [releaseAnalysisImage].
     */
    private fun drainAnalysisReader(reader: ImageReader, handler: Handler) {
        if (reader !== analysisImageReader || analysisDrainActive) {
            return
        }
        analysisDrainActive = true
        try {
            while (true) {
                if (analysisImagesInFlight >= ANALYSIS_MAX_IN_FLIGHT) {
                    analysisFramesDropped += 1
                    if (analysisFramesDropped == 1) {
                        Log.i(
                            TAG,
                            "analysis consumer holds $ANALYSIS_MAX_IN_FLIGHT frames; " +
                                "dropping until one returns",
                        )
                    }
                    return
                }
                val image = reader.acquireLatestImage() ?: return
                analysisImagesInFlight += 1
                if (analysisFramesDropped != 0) {
                    Log.i(TAG, "analysis resumed after dropping $analysisFramesDropped frames")
                    analysisFramesDropped = 0
                }
                // Newest wins: a frame nobody took yet goes back to the reader.
                analysisQueue.pollLast()?.let { stale -> stale.close() }
                analysisQueue.offerLast(
                    AnalysisFrame(
                        image,
                        displayRotationDegrees(),
                        imageDataSpace(image),
                        image.timestamp,
                    ) {
                        releaseAnalysisImage(reader, handler)
                    },
                )
            }
        } finally {
            analysisDrainActive = false
        }
    }

    /**
     * A leased analysis image was closed on some thread: free one in-flight
     * slot, then re-check the reader for the newest image that waited for
     * it. Runs on the camera background thread — [AnalysisFrame.close] on
     * another thread posts here; once the session's looper is gone the slot
     * count has been reset anyway, so a close during teardown is dropped.
     */
    private fun releaseAnalysisImage(reader: ImageReader, handler: Handler) {
        val released = Runnable {
            if (reader === analysisImageReader) {
                analysisImagesInFlight -= 1
                drainAnalysisReader(reader, handler)
            }
        }
        if (handler.looper.isCurrentThread) {
            released.run()
        } else if (handler.looper.thread.isAlive) {
            handler.post(released)
        }
    }

    /**
     * Clockwise angle, in degrees, through which the sensor's output must be
     * rotated to be upright in the device's natural orientation.
     */
    fun getSensorOrientation(cameraId: String): Int {
        val manager = appContext.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        return manager.getCameraCharacteristics(cameraId)
            .get(CameraCharacteristics.SENSOR_ORIENTATION)
            ?: throw IllegalStateException("camera $cameraId reports no SENSOR_ORIENTATION")
    }

    /**
     * Whether the lens faces away from the display. Front and external lenses
     * do not, so the display rotation turns their image the other way.
     */
    fun lensFacesBack(cameraId: String): Boolean {
        val manager = appContext.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        return manager.getCameraCharacteristics(cameraId)
            .get(CameraCharacteristics.LENS_FACING) == CameraCharacteristics.LENS_FACING_BACK
    }

    private fun displayRotationDegrees(): Int {
        val display = displayManager.getDisplay(Display.DEFAULT_DISPLAY)
            ?: throw IllegalStateException("the default display is gone")
        return when (val rotation = display.rotation) {
            Surface.ROTATION_0 -> 0
            Surface.ROTATION_90 -> 90
            Surface.ROTATION_180 -> 180
            Surface.ROTATION_270 -> 270
            else -> throw IllegalStateException("unknown display rotation $rotation")
        }
    }

    /**
     * Get current frame size.
     */
    fun getFrameSize(): IntArray {
        return intArrayOf(frameWidth, frameHeight)
    }

    /**
     * Close camera resources.
     */
    fun closeCamera() {
        stopRecordingNow()
        stopCapture()

        synchronized(requestLock) {
            pendingOpen?.fail("camera closed")
            pendingOpen = null
            pendingSession?.fail("camera closed")
            pendingSession = null
            pendingPhoto?.fail("camera closed")
            pendingPhoto = null
            pendingRawPhoto?.fail("camera closed")
            pendingRawPhoto = null
        }

        cameraDevice?.close()
        cameraDevice = null

        // The readers' listeners run on the background thread and read their
        // images' buffers there; closing a reader frees those buffers, so the
        // thread must have finished with them first.
        stopBackgroundThread()

        previewImageReader?.close()
        previewImageReader = null
        analysisImageReader?.close()
        analysisImageReader = null
        stillImageReader?.close()
        stillImageReader = null
        rawImageReader?.close()
        rawImageReader = null

        releaseRecorder()
        stopRawVideoRecordingNow()

        // The background thread is dead by now, so the frames' closes could
        // not post their bookkeeping anyway; the count resets below.
        while (true) {
            val stale = frameQueue.pollFirst() ?: break
            stale.hardwareBuffer.close()
            stale.image.close()
        }
        while (true) {
            val stale = analysisQueue.pollFirst() ?: break
            stale.close()
        }
        previewImagesInFlight = 0
        previewFramesDropped = 0
        previewDrainActive = false
        analysisImagesInFlight = 0
        analysisFramesDropped = 0
        analysisDrainActive = false

        currentCameraId = null
        currentCharacteristics = null
        cameraManager = null
        recordingStartElapsedRealtimeMs = 0L
        isRecording = false
    }

    // -------------------------------------------------------------------------
    // Capability Queries (Camera ID + Context based)
    // -------------------------------------------------------------------------
    fun getSupportedResolutions(cameraId: String): IntArray {
        val snapshot = queryCapabilitySnapshot(cameraId)
        if (snapshot.resolutions.isEmpty()) {
            return intArrayOf()
        }
        val output = IntArray(snapshot.resolutions.size * 2)
        snapshot.resolutions.forEachIndexed { index, size ->
            output[index * 2] = size.width
            output[index * 2 + 1] = size.height
        }
        return output
    }
    fun getSupportedFrameRates(cameraId: String): IntArray {
        return queryCapabilitySnapshot(cameraId).frameRates
    }
    fun getZoomRange(cameraId: String): FloatArray {
        val snapshot = queryCapabilitySnapshot(cameraId)
        return floatArrayOf(snapshot.zoomMin, snapshot.zoomMax)
    }
    fun supportsExposureCompensation(cameraId: String): Boolean {
        return queryCapabilitySnapshot(cameraId).supportsExposureCompensation
    }
    fun supportsManualFocus(cameraId: String): Boolean {
        return queryCapabilitySnapshot(cameraId).supportsManualFocus
    }
    fun supportsManualWhiteBalance(cameraId: String): Boolean {
        return queryCapabilitySnapshot(cameraId).supportsManualWhiteBalance
    }
    fun getSupportedDynamicRanges(cameraId: String): IntArray {
        return queryCapabilitySnapshot(cameraId).dynamicRanges
    }
    fun supportsStandardStabilization(cameraId: String): Boolean {
        return queryCapabilitySnapshot(cameraId).supportsStandardStabilization
    }
    fun supportsCinematicStabilization(cameraId: String): Boolean {
        return queryCapabilitySnapshot(cameraId).supportsCinematicStabilization
    }
    fun hasFlash(cameraId: String): Boolean {
        return queryCapabilitySnapshot(cameraId).hasFlash
    }
    fun hasTorch(cameraId: String): Boolean {
        return queryCapabilitySnapshot(cameraId).hasTorch
    }
    fun supportsRawPhoto(cameraId: String): Boolean {
        return queryCapabilitySnapshot(cameraId).supportsRawPhoto
    }
    fun supportsRawVideo(cameraId: String): Boolean {
        return queryCapabilitySnapshot(cameraId).supportsRawVideo
    }
    fun supportsConcurrentMultiCamera(cameraId: String): Boolean {
        return queryCapabilitySnapshot(cameraId).supportsConcurrentMultiCamera
    }
    fun maxConcurrentCameras(cameraId: String): Int {
        return queryCapabilitySnapshot(cameraId).maxConcurrentCameras
    }

    // -------------------------------------------------------------------------
    // Runtime Control APIs (active camera)
    // -------------------------------------------------------------------------
    fun setZoom(factor: Float): Boolean {
        if (factor.isNaN() || factor <= 0f) {
            Log.e(TAG, "Invalid zoom factor: $factor")
            return false
        }
        val clamped = factor.coerceIn(cachedZoomMin, cachedZoomMax)
        selectedZoomFactor = clamped
        return updateRepeatingRequest()
    }
    fun setFlashMode(mode: Int): Boolean {
        if (mode !in FLASH_OFF..FLASH_TORCH) {
            Log.e(TAG, "Invalid flash mode: $mode")
            return false
        }
        if ((mode == FLASH_ON || mode == FLASH_AUTO) && !cachedHasFlash) {
            Log.e(TAG, "Flash mode requested but flash is unavailable")
            return false
        }
        if (mode == FLASH_TORCH && !cachedHasTorch) {
            Log.e(TAG, "Torch mode requested but torch is unavailable")
            return false
        }
        selectedFlashMode = mode
        return updateRepeatingRequest()
    }
    fun setStabilizationMode(mode: Int): Boolean {
        if (mode !in STABILIZATION_OFF..STABILIZATION_CINEMATIC) {
            Log.e(TAG, "Invalid stabilization mode: $mode")
            return false
        }
        if (mode == STABILIZATION_STANDARD && !cachedSupportsStandardStabilization) {
            Log.e(TAG, "Standard stabilization is not supported")
            return false
        }
        if (mode == STABILIZATION_CINEMATIC && !cachedSupportsCinematicStabilization) {
            Log.e(TAG, "Cinematic stabilization is not supported")
            return false
        }
        selectedStabilizationMode = mode
        return updateRepeatingRequest()
    }
    fun setDynamicRange(profile: Int): Boolean {
        if (profile !in DYNAMIC_RANGE_SDR..DYNAMIC_RANGE_DOLBY_VISION) {
            Log.e(TAG, "Invalid dynamic range profile: $profile")
            return false
        }
        if (isRecording) {
            Log.e(TAG, "Dynamic range cannot change while recording")
            return false
        }
        val platformProfile = cachedPlatformDynamicRanges[profile] ?: run {
            Log.e(TAG, "Dynamic range profile is not supported by this camera: $profile")
            return false
        }
        selectedDynamicRangeProfile = profile
        selectedPlatformDynamicRangeProfile = platformProfile
        return true
    }
    fun setExposureCompensation(ev: Float): Boolean {
        val characteristics = currentCharacteristics ?: run {
            Log.e(TAG, "setExposureCompensation called before camera open")
            return false
        }
        val range = characteristics.get(CameraCharacteristics.CONTROL_AE_COMPENSATION_RANGE) ?: run {
            Log.e(TAG, "Exposure compensation range is unavailable")
            return false
        }
        val stepRational = characteristics.get(CameraCharacteristics.CONTROL_AE_COMPENSATION_STEP)
            ?: run {
                Log.e(TAG, "Exposure compensation step is unavailable")
                return false
            }
        val step = stepRational.toFloat()
        if (step == 0f) {
            Log.e(TAG, "Exposure compensation step is zero")
            return false
        }
        val targetSteps = (ev / step).roundToInt().coerceIn(range.lower, range.upper)
        selectedExposureCompensationSteps = targetSteps
        return updateRepeatingRequest()
    }
    fun setFocusMode(mode: Int): Boolean {
        if (mode !in FOCUS_CONTINUOUS_AUTO..FOCUS_LOCKED) {
            Log.e(TAG, "Invalid focus mode: $mode")
            return false
        }
        if ((mode == FOCUS_MANUAL || mode == FOCUS_LOCKED) && !cachedSupportsManualFocus) {
            Log.e(TAG, "Manual/locked focus requested but unsupported")
            return false
        }
        selectedFocusMode = mode
        if (mode != FOCUS_MANUAL) {
            selectedManualFocusDistance = null
        }
        return updateRepeatingRequest()
    }
    fun setFocusDistanceNormalized(distance: Float): Boolean {
        if (distance.isNaN() || distance < 0f || distance > 1f) {
            Log.e(TAG, "Invalid normalized focus distance: $distance")
            return false
        }
        val characteristics = currentCharacteristics ?: run {
            Log.e(TAG, "setFocusDistanceNormalized called before camera open")
            return false
        }
        val minFocusDistance = characteristics.get(
            CameraCharacteristics.LENS_INFO_MINIMUM_FOCUS_DISTANCE,
        ) ?: 0f
        if (minFocusDistance <= 0f) {
            Log.e(TAG, "Manual focus distance is unsupported")
            return false
        }

        // Rust contract: 0.0 = near, 1.0 = infinity.
        val lensDistance = (1f - distance) * minFocusDistance
        selectedFocusMode = FOCUS_MANUAL
        selectedManualFocusDistance = lensDistance.coerceIn(0f, minFocusDistance)
        return updateRepeatingRequest()
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    private fun startBackgroundThread() {
        if (backgroundThread != null && backgroundHandler != null) {
            return
        }

        val thread = HandlerThread("WaterkitCameraBackground")
        thread.start()
        backgroundThread = thread
        backgroundHandler = Handler(thread.looper)
    }

    private fun stopBackgroundThread() {
        val thread = backgroundThread ?: return
        thread.quitSafely()
        try {
            thread.join()
        } catch (error: InterruptedException) {
            Thread.currentThread().interrupt()
            Log.e(TAG, "Interrupted while stopping camera background thread", error)
        }

        backgroundThread = null
        backgroundHandler = null
    }

    private fun queryCapabilitySnapshot(cameraId: String): CapabilitySnapshot {
        val manager = appContext.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        val characteristics = manager.getCameraCharacteristics(cameraId)
        return queryCapabilitySnapshot(manager, cameraId, characteristics)
    }

    private fun queryCapabilitySnapshot(
        manager: CameraManager,
        cameraId: String,
        characteristics: CameraCharacteristics,
    ): CapabilitySnapshot {
        val streamMap = characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
        val yuvSizes = streamMap?.getOutputSizes(ImageFormat.YUV_420_888)?.toList() ?: emptyList()
        val sortedSizes = yuvSizes
            .distinctBy { it.width to it.height }
            .sortedByDescending { it.width.toLong() * it.height.toLong() }

        val fpsRanges =
            characteristics.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)
                ?: emptyArray()
        val frameRates = fpsRanges
            .map { it.upper.coerceAtLeast(1) }
            .distinct()
            .sorted()
            .ifEmpty { listOf(30) }
            .toIntArray()

        val supportsExposureCompensation = characteristics
            .get(CameraCharacteristics.CONTROL_AE_COMPENSATION_RANGE)
            ?.let { range -> range.lower != 0 || range.upper != 0 } ?: false

        val supportsManualFocus = (characteristics.get(
            CameraCharacteristics.LENS_INFO_MINIMUM_FOCUS_DISTANCE,
        ) ?: 0f) > 0f

        val supportsManualWhiteBalance = characteristics
            .get(CameraCharacteristics.CONTROL_AWB_AVAILABLE_MODES)
            ?.contains(CaptureRequest.CONTROL_AWB_MODE_OFF) ?: false

        val capabilities = characteristics.get(
            CameraCharacteristics.REQUEST_AVAILABLE_CAPABILITIES,
        ) ?: intArrayOf()
        val supportsRawPhoto = capabilities.contains(
            CameraCharacteristics.REQUEST_AVAILABLE_CAPABILITIES_RAW,
        )

        val zoomRange = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            characteristics.get(CameraCharacteristics.CONTROL_ZOOM_RATIO_RANGE)?.let { it.lower to it.upper }
        } else {
            null
        }
        val maxDigitalZoom = characteristics
            .get(CameraCharacteristics.SCALER_AVAILABLE_MAX_DIGITAL_ZOOM)
            ?.coerceAtLeast(1.0f) ?: 1.0f
        val zoomMin = zoomRange?.first ?: 1.0f
        val zoomMax = zoomRange?.second ?: maxDigitalZoom

        val platformDynamicRanges = queryDynamicRangeProfiles(characteristics)
        val dynamicRanges = platformDynamicRanges.keys.sorted().toIntArray()

        val stabilizationModes = characteristics.get(
            CameraCharacteristics.CONTROL_AVAILABLE_VIDEO_STABILIZATION_MODES,
        ) ?: intArrayOf()
        val supportsStandardStabilization =
            stabilizationModes.contains(CaptureRequest.CONTROL_VIDEO_STABILIZATION_MODE_ON)

        val hasFlash = characteristics.get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true
        val hasTorch = hasFlash

        val (supportsConcurrent, maxConcurrent) =
            concurrentCameraSupport(manager, cameraId)

        return CapabilitySnapshot(
            resolutions = sortedSizes,
            frameRates = frameRates,
            zoomMin = zoomMin,
            zoomMax = zoomMax,
            supportsExposureCompensation = supportsExposureCompensation,
            supportsManualFocus = supportsManualFocus,
            supportsManualWhiteBalance = supportsManualWhiteBalance,
            dynamicRanges = dynamicRanges,
            platformDynamicRanges = platformDynamicRanges,
            supportsStandardStabilization = supportsStandardStabilization,
            supportsCinematicStabilization = false,
            hasFlash = hasFlash,
            hasTorch = hasTorch,
            supportsRawPhoto = supportsRawPhoto,
            supportsRawVideo = true,
            supportsConcurrentMultiCamera = supportsConcurrent,
            maxConcurrentCameras = maxConcurrent,
        )
    }

    private fun concurrentCameraSupport(manager: CameraManager, cameraId: String): Pair<Boolean, Int> {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            return false to 1
        }
        val concurrentSets = manager.concurrentCameraIds
        if (concurrentSets.isEmpty()) {
            return false to 1
        }
        val containing = concurrentSets.filter { set -> set.contains(cameraId) }
        if (containing.isEmpty()) {
            return false to 1
        }
        val max = containing.maxOf { it.size }.coerceAtLeast(1)
        return (max > 1) to max
    }

    private fun queryDynamicRangeProfiles(
        characteristics: CameraCharacteristics,
    ): Map<Int, Long> {
        val result = linkedMapOf(DYNAMIC_RANGE_SDR to PLATFORM_DYNAMIC_RANGE_STANDARD)
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            return result
        }

        val profiles = characteristics.get(
            CameraCharacteristics.REQUEST_AVAILABLE_DYNAMIC_RANGE_PROFILES,
        ) ?: return result
        val supported = profiles.supportedProfiles
        val supportsMixedStandard = { profile: Long ->
            val constraints = profiles.getProfileCaptureRequestConstraints(profile)
            constraints.isEmpty() || constraints.contains(DynamicRangeProfiles.STANDARD)
        }

        if (
            supported.contains(DynamicRangeProfiles.HLG10) &&
            supportsMixedStandard(DynamicRangeProfiles.HLG10) &&
            encoderProfile(
                MediaFormat.MIMETYPE_VIDEO_HEVC,
                setOf(MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10),
            ) != null
        ) {
            result[DYNAMIC_RANGE_HLG10] = DynamicRangeProfiles.HLG10
        }
        if (
            supported.contains(DynamicRangeProfiles.HDR10) &&
            supportsMixedStandard(DynamicRangeProfiles.HDR10) &&
            encoderProfile(
                MediaFormat.MIMETYPE_VIDEO_HEVC,
                setOf(MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10),
            ) != null
        ) {
            result[DYNAMIC_RANGE_HDR10] = DynamicRangeProfiles.HDR10
        }

        val dolbyProfiles = listOf(
            DynamicRangeProfiles.DOLBY_VISION_10B_HDR_OEM,
            DynamicRangeProfiles.DOLBY_VISION_10B_HDR_REF,
            DynamicRangeProfiles.DOLBY_VISION_10B_HDR_OEM_PO,
            DynamicRangeProfiles.DOLBY_VISION_10B_HDR_REF_PO,
            DynamicRangeProfiles.DOLBY_VISION_8B_HDR_OEM,
            DynamicRangeProfiles.DOLBY_VISION_8B_HDR_REF,
            DynamicRangeProfiles.DOLBY_VISION_8B_HDR_OEM_PO,
            DynamicRangeProfiles.DOLBY_VISION_8B_HDR_REF_PO,
        )
        val dolbyProfile = dolbyProfiles.firstOrNull { profile ->
            supported.contains(profile) && supportsMixedStandard(profile)
        }
        if (
            dolbyProfile != null &&
            encoderProfile(MediaFormat.MIMETYPE_VIDEO_DOLBY_VISION, null) != null
        ) {
            result[DYNAMIC_RANGE_DOLBY_VISION] = dolbyProfile
        }
        return result
    }

    private fun cacheSnapshot(snapshot: CapabilitySnapshot) {
        cachedResolutions = if (snapshot.resolutions.isEmpty()) {
            intArrayOf(frameWidth, frameHeight)
        } else {
            IntArray(snapshot.resolutions.size * 2).also { out ->
                snapshot.resolutions.forEachIndexed { index, size ->
                    out[index * 2] = size.width
                    out[index * 2 + 1] = size.height
                }
            }
        }
        cachedFrameRates = snapshot.frameRates
        cachedZoomMin = snapshot.zoomMin
        cachedZoomMax = snapshot.zoomMax
        cachedSupportsExposureCompensation = snapshot.supportsExposureCompensation
        cachedSupportsManualFocus = snapshot.supportsManualFocus
        cachedSupportsManualWhiteBalance = snapshot.supportsManualWhiteBalance
        cachedPlatformDynamicRanges = snapshot.platformDynamicRanges
        cachedSupportsStandardStabilization = snapshot.supportsStandardStabilization
        cachedSupportsCinematicStabilization = snapshot.supportsCinematicStabilization
        cachedHasFlash = snapshot.hasFlash
        cachedHasTorch = snapshot.hasTorch
        cachedSupportsRawPhoto = snapshot.supportsRawPhoto
        cachedSupportsRawVideo = snapshot.supportsRawVideo
        cachedSupportsConcurrentMultiCamera = snapshot.supportsConcurrentMultiCamera
        cachedMaxConcurrentCameras = snapshot.maxConcurrentCameras
    }

    private fun chooseNearestSize(requestedWidth: Int, requestedHeight: Int, sizes: List<Size>): Size {
        if (sizes.isEmpty()) {
            return Size(requestedWidth, requestedHeight)
        }
        return sizes.minBy { size ->
            kotlin.math.abs(size.width - requestedWidth) + kotlin.math.abs(size.height - requestedHeight)
        }
    }

    private fun chooseNearestFrameRate(requestedFps: Int, supported: IntArray): Int {
        if (supported.isEmpty()) {
            return requestedFps.coerceAtLeast(1)
        }
        return supported.minBy { fps -> kotlin.math.abs(fps - requestedFps) }
    }

    /**
     * Rebuilds the capture session; [callback] fails on a configure error,
     * and on success [onReady] runs before it, answering the operation's
     * caller. There is no timeout — a session that never answers surfaces
     * through the caller's own deadline.
     */
    private fun createCaptureSession(
        includeRecorderSurface: Boolean,
        callback: NativeCallback?,
        onReady: () -> Unit,
    ) {
        val pending = PendingResult(callback)
        synchronized(requestLock) {
            pendingSession?.fail("superseded by a newer session request")
            pendingSession = pending
        }

        val device = cameraDevice ?: run {
            Log.e(TAG, "createCaptureSession called before camera open")
            sessionFailed(pending, "createCaptureSession called before camera open")
            return
        }
        val previewReader = previewImageReader ?: run {
            Log.e(TAG, "createCaptureSession called before preview reader init")
            sessionFailed(pending, "createCaptureSession called before preview reader init")
            return
        }
        val stillReader = stillImageReader ?: run {
            Log.e(TAG, "createCaptureSession called before still reader init")
            sessionFailed(pending, "createCaptureSession called before still reader init")
            return
        }
        val handler = backgroundHandler ?: run {
            Log.e(TAG, "createCaptureSession called without background handler")
            sessionFailed(pending, "createCaptureSession called without background handler")
            return
        }

        val rawVideoSurface = rawVideoImageReader?.surface
        val analysisSurface = analysisImageReader?.surface
        val surfaces = mutableListOf<Surface>(
            previewReader.surface,
            stillReader.surface,
        )
        rawImageReader?.surface?.let { surfaces.add(it) }
        rawVideoSurface?.let { surfaces.add(it) }
        analysisSurface?.let { surfaces.add(it) }

        if (includeRecorderSurface) {
            val surface = recorderSurface ?: run {
                Log.e(TAG, "Recorder surface is missing while starting recording session")
                sessionFailed(pending, "recorder surface is missing while starting recording session")
                return
            }
            surfaces.add(surface)
        }

        stopCapture()

        val sessionCallback = object : CameraCaptureSession.StateCallback() {
            override fun onConfigured(session: CameraCaptureSession) {
                captureSession = session
                try {
                    val template = if (includeRecorderSurface) {
                        CameraDevice.TEMPLATE_RECORD
                    } else {
                        CameraDevice.TEMPLATE_PREVIEW
                    }
                    val builder = device.createCaptureRequest(template)
                    builder.addTarget(previewReader.surface)
                    rawVideoSurface?.let { builder.addTarget(it) }
                    analysisSurface?.let { builder.addTarget(it) }
                    if (includeRecorderSurface) {
                        val recordingSurface = recorderSurface
                            ?: error("Recorder surface lost during session configuration")
                        builder.addTarget(recordingSurface)
                    }
                    applyRequestControls(builder, forStillCapture = false)
                    session.setRepeatingRequest(builder.build(), null, handler)
                    previewRequestBuilder = builder
                } catch (error: Exception) {
                    Log.e(TAG, "Failed to configure repeating request", error)
                    previewRequestBuilder = null
                    sessionFailed(
                        pending,
                        "failed to configure repeating request: ${error.message ?: error.javaClass.name}",
                    )
                    return
                }
                synchronized(requestLock) {
                    if (pendingSession === pending) {
                        pendingSession = null
                    }
                }
                try {
                    onReady()
                } catch (error: Exception) {
                    pending.fail(
                        "session-ready step failed: ${error.message ?: error.javaClass.name}",
                    )
                }
            }

            override fun onConfigureFailed(session: CameraCaptureSession) {
                Log.e(TAG, "Camera capture session configuration failed")
                sessionFailed(pending, "camera capture session configuration failed")
            }
        }

        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                val recordingSurface = recorderSurface
                val outputs = surfaces.map { surface ->
                    OutputConfiguration(surface).apply {
                        val profile = if (includeRecorderSurface && surface === recordingSurface) {
                            selectedPlatformDynamicRangeProfile
                        } else {
                            DynamicRangeProfiles.STANDARD
                        }
                        setDynamicRangeProfile(profile)
                    }
                }
                val executor = Executor { command -> handler.post(command) }
                device.createCaptureSession(
                    SessionConfiguration(
                        SessionConfiguration.SESSION_REGULAR,
                        outputs,
                        executor,
                        sessionCallback,
                    ),
                )
            } else {
                @Suppress("DEPRECATION") // the only session API on API < 33
                device.createCaptureSession(surfaces, sessionCallback, handler)
            }
        } catch (error: Exception) {
            Log.e(TAG, "Failed to create capture session", error)
            sessionFailed(pending, "failed to create capture session: ${error.message ?: error.javaClass.name}")
        }
    }

    private fun sessionFailed(pending: PendingResult, message: String) {
        synchronized(requestLock) {
            if (pendingSession === pending) {
                pendingSession = null
            }
        }
        pending.fail(message)
    }

    private fun updateRepeatingRequest(): Boolean {
        val session = captureSession ?: return false
        val builder = previewRequestBuilder ?: return false
        val handler = backgroundHandler ?: return false

        return try {
            applyRequestControls(builder, forStillCapture = false)
            session.setRepeatingRequest(builder.build(), null, handler)
            true
        } catch (error: Exception) {
            Log.e(TAG, "Failed to update repeating request controls", error)
            false
        }
    }

    private fun applyRequestControls(
        builder: CaptureRequest.Builder,
        forStillCapture: Boolean,
    ) {
        trySet(builder, CaptureRequest.CONTROL_MODE, CaptureRequest.CONTROL_MODE_AUTO)

        val characteristics = currentCharacteristics
            ?: error("Camera characteristics are not initialized")
        val fpsRanges = characteristics.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)
            ?: emptyArray()
        val selectedRange = chooseFpsRange(frameRate, fpsRanges)
        if (selectedRange != null) {
            trySet(builder, CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, selectedRange)
        }

        selectedExposureCompensationSteps?.let { steps ->
            trySet(builder, CaptureRequest.CONTROL_AE_EXPOSURE_COMPENSATION, steps)
        }

        when (selectedFlashMode) {
            FLASH_OFF -> {
                trySet(builder, CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_ON)
                trySet(builder, CaptureRequest.FLASH_MODE, CaptureRequest.FLASH_MODE_OFF)
            }
            FLASH_ON -> {
                val mode = if (forStillCapture) {
                    CaptureRequest.CONTROL_AE_MODE_ON_ALWAYS_FLASH
                } else {
                    CaptureRequest.CONTROL_AE_MODE_ON
                }
                trySet(builder, CaptureRequest.CONTROL_AE_MODE, mode)
                trySet(builder, CaptureRequest.FLASH_MODE, CaptureRequest.FLASH_MODE_SINGLE)
            }
            FLASH_AUTO -> {
                trySet(builder, CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_ON_AUTO_FLASH)
                trySet(builder, CaptureRequest.FLASH_MODE, CaptureRequest.FLASH_MODE_OFF)
            }
            FLASH_TORCH -> {
                trySet(builder, CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_ON)
                trySet(builder, CaptureRequest.FLASH_MODE, CaptureRequest.FLASH_MODE_TORCH)
            }
        }

        when (selectedFocusMode) {
            FOCUS_CONTINUOUS_AUTO -> {
                trySet(builder, CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO)
            }
            FOCUS_AUTO -> {
                trySet(builder, CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_AUTO)
            }
            FOCUS_MANUAL -> {
                trySet(builder, CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_OFF)
                val distance = selectedManualFocusDistance ?: 0f
                trySet(builder, CaptureRequest.LENS_FOCUS_DISTANCE, distance)
            }
            FOCUS_LOCKED -> {
                trySet(builder, CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_OFF)
            }
        }

        when (selectedStabilizationMode) {
            STABILIZATION_OFF -> {
                trySet(
                    builder,
                    CaptureRequest.CONTROL_VIDEO_STABILIZATION_MODE,
                    CaptureRequest.CONTROL_VIDEO_STABILIZATION_MODE_OFF,
                )
            }
            STABILIZATION_STANDARD,
            STABILIZATION_CINEMATIC -> {
                trySet(
                    builder,
                    CaptureRequest.CONTROL_VIDEO_STABILIZATION_MODE,
                    CaptureRequest.CONTROL_VIDEO_STABILIZATION_MODE_ON,
                )
            }
        }

        trySet(builder, CaptureRequest.CONTROL_MODE, CaptureRequest.CONTROL_MODE_AUTO)
        trySet(builder, CaptureRequest.CONTROL_SCENE_MODE, CaptureRequest.CONTROL_SCENE_MODE_DISABLED)

        applyZoom(builder, selectedZoomFactor, characteristics)
    }

    private fun chooseFpsRange(requestedFps: Int, ranges: Array<Range<Int>>): Range<Int>? {
        if (ranges.isEmpty()) {
            return null
        }
        val exact = ranges.firstOrNull { range -> requestedFps in range }
        if (exact != null) {
            return exact
        }
        return ranges.minBy { range -> kotlin.math.abs(range.upper - requestedFps) }
    }

    private fun applyZoom(
        builder: CaptureRequest.Builder,
        zoomFactor: Float,
        characteristics: CameraCharacteristics,
    ) {
        val clampedZoom = zoomFactor.coerceIn(cachedZoomMin, cachedZoomMax)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            try {
                builder.set(CaptureRequest.CONTROL_ZOOM_RATIO, clampedZoom)
                return
            } catch (error: IllegalArgumentException) {
                Log.w(TAG, "CONTROL_ZOOM_RATIO unsupported, falling back to crop region")
            }
        }

        val sensorRect = characteristics.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE)
            ?: return
        val centerX = sensorRect.centerX()
        val centerY = sensorRect.centerY()
        val halfWidth = (sensorRect.width() / (2f * clampedZoom)).toInt().coerceAtLeast(1)
        val halfHeight = (sensorRect.height() / (2f * clampedZoom)).toInt().coerceAtLeast(1)
        val cropRect = Rect(
            (centerX - halfWidth).coerceAtLeast(0),
            (centerY - halfHeight).coerceAtLeast(0),
            (centerX + halfWidth).coerceAtMost(sensorRect.right),
            (centerY + halfHeight).coerceAtMost(sensorRect.bottom),
        )
        trySet(builder, CaptureRequest.SCALER_CROP_REGION, cropRect)
    }

    private fun prepareRecorder(path: String): Boolean {
        val outputPath = path.trim()
        if (outputPath.isEmpty()) {
            Log.e(TAG, "Recording path must not be empty")
            return false
        }

        releaseRecorder()

        return try {
            val recorder =
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                    MediaRecorder(appContext)
                } else {
                    @Suppress("DEPRECATION") // `MediaRecorder(context)` needs API 31
                    MediaRecorder()
                }
            recorder.setVideoSource(MediaRecorder.VideoSource.SURFACE)
            recorder.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4)

            val encoder = when (selectedDynamicRangeProfile) {
                DYNAMIC_RANGE_DOLBY_VISION -> MediaRecorder.VideoEncoder.DOLBY_VISION
                DYNAMIC_RANGE_HDR10,
                DYNAMIC_RANGE_HLG10 -> MediaRecorder.VideoEncoder.HEVC
                else -> MediaRecorder.VideoEncoder.H264
            }
            recorder.setVideoEncoder(encoder)
            if (selectedDynamicRangeProfile != DYNAMIC_RANGE_SDR) {
                val profileLevel = recordingEncoderProfile()
                    ?: error("Selected dynamic range has no matching video encoder profile")
                recorder.setVideoEncodingProfileLevel(profileLevel.profile, profileLevel.level)
            }
            recorder.setVideoEncodingBitRate(computeVideoBitrate(frameWidth, frameHeight, frameRate))
            recorder.setVideoFrameRate(frameRate)
            recorder.setVideoSize(frameWidth, frameHeight)
            recorder.setOutputFile(outputPath)
            recorder.prepare()

            mediaRecorder = recorder
            recorderSurface = recorder.surface
            true
        } catch (error: Exception) {
            Log.e(TAG, "Failed to prepare MediaRecorder", error)
            releaseRecorder()
            false
        }
    }

    private fun computeVideoBitrate(width: Int, height: Int, fps: Int): Int {
        val pixelRate = width.toLong() * height.toLong() * fps.toLong()
        val bitrate = pixelRate * 10L
        return bitrate.coerceIn(2_000_000L, 60_000_000L).toInt()
    }



    /**
     * Appends one YUV_420_888 image as NV12: the luma rows, then the
     * interleaved Cb/Cr rows, without padding. The planes are copied as
     * stored; no colour conversion happens.
     */
    private fun maybeWriteRawVideoFrame(image: Image, timestampNs: Long) {
        if (!synchronized(rawVideoLock) { isRawVideoRecording && rawVideoOutput != null }) {
            return
        }

        val dataSpace = imageDataSpace(image)
        val colorCodes = rawVideoColorCodes(dataSpace)
        if (colorCodes == null) {
            failRawVideoRecording(
                "Unsupported RAW video data space=$dataSpace " +
                    "(0x${Integer.toHexString(dataSpace)}), " +
                    "standard=${rawVideoDataSpaceStandard(dataSpace)}, " +
                    "range=${rawVideoDataSpaceRange(dataSpace)}",
            )
            return
        }

        try {
            val width = image.width
            val height = image.height
            val chromaWidth = (width + 1) / 2
            val chromaHeight = (height + 1) / 2
            val nv12 = ByteArray(width * height + chromaWidth * chromaHeight * 2)
            val luma = image.planes[0]
            for (row in 0 until height) {
                val source = luma.buffer.duplicate()
                source.position(row * luma.rowStride)
                source.get(nv12, row * width, width)
            }
            val cb = image.planes[1]
            val cr = image.planes[2]
            var offset = width * height
            for (row in 0 until chromaHeight) {
                for (column in 0 until chromaWidth) {
                    nv12[offset] = cb.buffer.get(row * cb.rowStride + column * cb.pixelStride)
                    nv12[offset + 1] = cr.buffer.get(row * cr.rowStride + column * cr.pixelStride)
                    offset += 2
                }
            }

            val changedDataSpace = synchronized(rawVideoLock) {
                val output = rawVideoOutput ?: return
                val firstDataSpace = rawVideoDataSpace
                if (firstDataSpace != null && firstDataSpace != dataSpace) {
                    firstDataSpace
                } else {
                    if (firstDataSpace == null) {
                        writeRawVideoHeader(
                            output,
                            width,
                            height,
                            frameRate,
                            colorCodes.first,
                            colorCodes.second,
                        )
                        rawVideoDataSpace = dataSpace
                    }
                    writeU64LE(output, timestampNs)
                    writeU32LE(output, nv12.size)
                    output.write(nv12)
                    null
                }
            }
            if (changedDataSpace != null) {
                failRawVideoRecording(
                    "RAW video data space changed from $changedDataSpace " +
                        "(0x${Integer.toHexString(changedDataSpace)}) to $dataSpace " +
                        "(0x${Integer.toHexString(dataSpace)})",
                )
            }
        } catch (error: Exception) {
            failRawVideoRecording("Failed writing RAW video frame in data space $dataSpace", error)
        }
    }

    private fun rawVideoColorCodes(dataSpace: Int): Pair<Int, Int>? {
        val matrix = when (rawVideoDataSpaceStandard(dataSpace)) {
            0 -> 6
            1 -> 1
            2, 3, 4, 5 -> 6
            6 -> 9
            else -> return null
        }
        val range = when (rawVideoDataSpaceRange(dataSpace)) {
            0, 1 -> 1
            2 -> 0
            else -> return null
        }
        return matrix to range
    }

    private fun rawVideoDataSpaceStandard(dataSpace: Int): Int =
        (dataSpace ushr DATA_SPACE_STANDARD_SHIFT) and DATA_SPACE_STANDARD_MASK

    private fun rawVideoDataSpaceRange(dataSpace: Int): Int =
        (dataSpace ushr DATA_SPACE_RANGE_SHIFT) and DATA_SPACE_RANGE_MASK

    private fun failRawVideoRecording(message: String, error: Exception? = null) {
        if (error == null) {
            Log.e(TAG, message)
        } else {
            Log.e(TAG, message, error)
        }
        // Torn down internally, so no caller is waiting: detach the reader's
        // surface from the session, then close the reader and the output.
        val reader = rawVideoImageReader
        rawVideoImageReader = null
        if (reader != null && cameraDevice != null) {
            createCaptureSession(includeRecorderSurface = false, callback = null) {
                reader.close()
            }
        } else {
            reader?.close()
        }
        closeRawVideoOutput(callback = null)
    }

    private fun writeRawVideoHeader(
        output: FileOutputStream,
        width: Int,
        height: Int,
        fps: Int,
        matrix: Int,
        range: Int,
    ) {
        // Header layout:
        // magic(4)='WKRV', version=2, NV12 pixel format, H.273 matrix, range,
        // then width(u32), height(u32), fps(u32).
        output.write(byteArrayOf('W'.code.toByte(), 'K'.code.toByte(), 'R'.code.toByte(), 'V'.code.toByte()))
        output.write(byteArrayOf(2, 3, matrix.toByte(), range.toByte()))
        writeU32LE(output, width)
        writeU32LE(output, height)
        writeU32LE(output, fps)
    }

    private fun writeU32LE(output: FileOutputStream, value: Int) {
        output.write(byteArrayOf(
            (value and 0xFF).toByte(),
            ((value ushr 8) and 0xFF).toByte(),
            ((value ushr 16) and 0xFF).toByte(),
            ((value ushr 24) and 0xFF).toByte(),
        ))
    }

    private fun writeU64LE(output: FileOutputStream, value: Long) {
        output.write(byteArrayOf(
            (value and 0xFF).toByte(),
            ((value ushr 8) and 0xFF).toByte(),
            ((value ushr 16) and 0xFF).toByte(),
            ((value ushr 24) and 0xFF).toByte(),
            ((value ushr 32) and 0xFF).toByte(),
            ((value ushr 40) and 0xFF).toByte(),
            ((value ushr 48) and 0xFF).toByte(),
            ((value ushr 56) and 0xFF).toByte(),
        ))
    }

    private fun releaseRecorder() {
        mediaRecorder?.reset()
        mediaRecorder?.release()
        mediaRecorder = null
        recorderSurface = null
    }

    private fun encoderProfile(
        mimeType: String,
        acceptedProfiles: Set<Int>?,
    ): MediaCodecInfo.CodecProfileLevel? {
        return try {
            val codecs = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
            codecs.asSequence()
                .filter { codec -> codec.isEncoder }
                .flatMap { codec ->
                    codec.supportedTypes.asSequence()
                        .filter { type -> type.equals(mimeType, ignoreCase = true) }
                        .flatMap { type -> codec.getCapabilitiesForType(type).profileLevels.asSequence() }
                }
                .filter { profileLevel ->
                    acceptedProfiles == null || acceptedProfiles.contains(profileLevel.profile)
                }
                .maxByOrNull { profileLevel -> profileLevel.level }
        } catch (error: Exception) {
            Log.e(TAG, "Failed to query encoder profile for $mimeType", error)
            null
        }
    }

    private fun recordingEncoderProfile(): MediaCodecInfo.CodecProfileLevel? {
        return when (selectedDynamicRangeProfile) {
            DYNAMIC_RANGE_HLG10 -> encoderProfile(
                MediaFormat.MIMETYPE_VIDEO_HEVC,
                setOf(MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10),
            )
            DYNAMIC_RANGE_HDR10 -> encoderProfile(
                MediaFormat.MIMETYPE_VIDEO_HEVC,
                setOf(MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10),
            )
            DYNAMIC_RANGE_DOLBY_VISION -> encoderProfile(
                MediaFormat.MIMETYPE_VIDEO_DOLBY_VISION,
                null,
            )
            else -> null
        }
    }

    private fun <T> trySet(
        builder: CaptureRequest.Builder,
        key: CaptureRequest.Key<T>,
        value: T,
    ) {
        try {
            builder.set(key, value)
        } catch (_: IllegalArgumentException) {
            // Key unsupported on this device/request template. Skip gracefully.
        }
    }
}
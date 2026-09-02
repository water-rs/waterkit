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
import android.view.Surface
import kotlin.math.roundToInt
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.LinkedBlockingDeque
import java.util.concurrent.TimeUnit

/**
 * Camera helper for waterkit-camera crate.
 * Uses Camera2 API for camera enumeration, still capture and video recording.
 */
class CameraHelper(private val appContext: Context) {
    private companion object {
        private const val TAG = "WaterkitCamera"

        private const val OPEN_TIMEOUT_SECONDS = 5L
        private const val SESSION_TIMEOUT_SECONDS = 5L
        private const val PHOTO_TIMEOUT_SECONDS = 5L

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
    private var stillImageReader: ImageReader? = null
    private var rawImageReader: ImageReader? = null

    private var mediaRecorder: MediaRecorder? = null
    private var recorderSurface: Surface? = null
    private var recordingStartElapsedRealtimeMs: Long = 0
    private var isRecording: Boolean = false
    private var rawVideoOutput: FileOutputStream? = null
    private var rawVideoRecordingStartElapsedRealtimeMs: Long = 0
    private var isRawVideoRecording: Boolean = false

    private var backgroundThread: HandlerThread? = null
    private var backgroundHandler: Handler? = null

    private val frameQueue: LinkedBlockingDeque<ByteArray> = LinkedBlockingDeque(1)
    private var latestPhotoData: ByteArray? = null
    private var latestRawPhotoData: ByteArray? = null
    private var latestRawImage: Image? = null
    private var pendingPhotoLatch: CountDownLatch? = null
    private var pendingRawImageLatch: CountDownLatch? = null

    private val photoLock = Any()
    private val rawPhotoLock = Any()
    private val rawVideoLock = Any()

    private var frameWidth: Int = 1280
    private var frameHeight: Int = 720
    private var frameRate: Int = 30

    private var selectedDynamicRangeProfile: Int = DYNAMIC_RANGE_SDR
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
     * Open a camera by ID with requested configuration.
     */
    fun openCamera(
        cameraId: String,
        requestedWidth: Int,
        requestedHeight: Int,
        requestedFrameRate: Int,
    ): Boolean {
        closeCamera()

        startBackgroundThread()
        val handler = backgroundHandler ?: run {
            Log.e(TAG, "background handler is not initialized")
            return false
        }

        return try {
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

            previewImageReader = ImageReader.newInstance(frameWidth, frameHeight, ImageFormat.YUV_420_888, 3)
            stillImageReader = ImageReader.newInstance(frameWidth, frameHeight, ImageFormat.JPEG, 2)
            rawImageReader = if (snapshot.supportsRawPhoto) {
                ImageReader.newInstance(frameWidth, frameHeight, ImageFormat.RAW_SENSOR, 2)
            } else {
                null
            }

            previewImageReader?.setOnImageAvailableListener({ reader ->
                val image = reader.acquireLatestImage() ?: return@setOnImageAvailableListener
                try {
                    frameWidth = image.width
                    frameHeight = image.height
                    val rgba = yuv420ToRgba(image)
                    val timestampNs = SystemClock.elapsedRealtimeNanos()
                    frameQueue.pollLast()
                    frameQueue.offerLast(rgba)
                    maybeWriteRawVideoFrame(rgba, image.width, image.height, timestampNs)
                } catch (error: Exception) {
                    Log.e(TAG, "Failed to process camera frame", error)
                } finally {
                    image.close()
                }
            }, handler)

            rawImageReader?.setOnImageAvailableListener({ reader ->
                val image = reader.acquireLatestImage() ?: return@setOnImageAvailableListener
                synchronized(rawPhotoLock) {
                    latestRawImage?.close()
                    latestRawImage = image
                    pendingRawImageLatch?.countDown()
                }
            }, handler)

            stillImageReader?.setOnImageAvailableListener({ reader ->
                val image = reader.acquireLatestImage() ?: return@setOnImageAvailableListener
                try {
                    if (image.format != ImageFormat.JPEG) {
                        Log.e(TAG, "Unexpected still image format: ${image.format}")
                        synchronized(photoLock) {
                            pendingPhotoLatch?.countDown()
                            pendingPhotoLatch = null
                        }
                        return@setOnImageAvailableListener
                    }
                    val plane = image.planes.firstOrNull() ?: run {
                        synchronized(photoLock) {
                            pendingPhotoLatch?.countDown()
                            pendingPhotoLatch = null
                        }
                        return@setOnImageAvailableListener
                    }
                    val buffer = plane.buffer
                    val bytes = ByteArray(buffer.remaining())
                    buffer.get(bytes)
                    synchronized(photoLock) {
                        latestPhotoData = bytes
                        pendingPhotoLatch?.countDown()
                        pendingPhotoLatch = null
                    }
                } catch (error: Exception) {
                    Log.e(TAG, "Failed to process still image", error)
                    synchronized(photoLock) {
                        pendingPhotoLatch?.countDown()
                        pendingPhotoLatch = null
                    }
                } finally {
                    image.close()
                }
            }, handler)

            val openLatch = CountDownLatch(1)
            var opened = false

            manager.openCamera(cameraId, object : CameraDevice.StateCallback() {
                override fun onOpened(camera: CameraDevice) {
                    cameraDevice = camera
                    opened = true
                    openLatch.countDown()
                }

                override fun onDisconnected(camera: CameraDevice) {
                    Log.e(TAG, "Camera disconnected: $cameraId")
                    camera.close()
                    if (cameraDevice === camera) {
                        cameraDevice = null
                    }
                    openLatch.countDown()
                }

                override fun onError(camera: CameraDevice, error: Int) {
                    Log.e(TAG, "Camera open error for $cameraId: $error")
                    camera.close()
                    if (cameraDevice === camera) {
                        cameraDevice = null
                    }
                    openLatch.countDown()
                }
            }, handler)

            val completed = openLatch.await(OPEN_TIMEOUT_SECONDS, TimeUnit.SECONDS)
            if (!completed || !opened) {
                Log.e(TAG, "Timed out opening camera: $cameraId")
                closeCamera()
                return false
            }

            true
        } catch (error: SecurityException) {
            Log.e(TAG, "Missing camera permission", error)
            closeCamera()
            false
        } catch (error: Exception) {
            Log.e(TAG, "Failed to open camera", error)
            closeCamera()
            false
        }
    }

    /**
     * Start frame capture.
     */
    fun startCapture(): Boolean {
        return createCaptureSession(includeRecorderSurface = false)
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
     * Capture a high-quality still image using Camera2 still-capture pipeline.
     */
    fun capturePhoto(): Boolean {
        val session = captureSession ?: run {
            Log.e(TAG, "capturePhoto called before capture session start")
            return false
        }
        val device = cameraDevice ?: run {
            Log.e(TAG, "capturePhoto called before camera open")
            return false
        }
        val stillReader = stillImageReader ?: run {
            Log.e(TAG, "capturePhoto called before still image reader init")
            return false
        }
        val handler = backgroundHandler ?: run {
            Log.e(TAG, "capturePhoto called without background handler")
            return false
        }

        synchronized(photoLock) {
            latestPhotoData = null
            pendingPhotoLatch = CountDownLatch(1)
        }

        return try {
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
                        synchronized(photoLock) {
                            pendingPhotoLatch?.countDown()
                            pendingPhotoLatch = null
                        }
                    }
                },
                handler,
            )

            val latch = synchronized(photoLock) { pendingPhotoLatch } ?: return false
            val completed = latch.await(PHOTO_TIMEOUT_SECONDS, TimeUnit.SECONDS)
            if (!completed) {
                Log.e(TAG, "Timed out waiting for still image")
                synchronized(photoLock) {
                    pendingPhotoLatch = null
                }
                return false
            }

            synchronized(photoLock) { latestPhotoData != null }
        } catch (error: Exception) {
            Log.e(TAG, "Failed to capture still image", error)
            synchronized(photoLock) {
                pendingPhotoLatch?.countDown()
                pendingPhotoLatch = null
            }
            false
        }
    }

    /**
     * Consume captured still image bytes (JPEG).
     */
    fun consumePhotoData(): ByteArray? {
        synchronized(photoLock) {
            val data = latestPhotoData
            latestPhotoData = null
            return data
        }
    }

    /**
     * Capture RAW photo using RAW_SENSOR + DNG container.
     */
    fun captureRawPhoto(): Boolean {
        val session = captureSession ?: run {
            Log.e(TAG, "captureRawPhoto called before capture session start")
            return false
        }
        val device = cameraDevice ?: run {
            Log.e(TAG, "captureRawPhoto called before camera open")
            return false
        }
        val reader = rawImageReader ?: run {
            Log.e(TAG, "captureRawPhoto called on camera without RAW_SENSOR support")
            return false
        }
        val characteristics = currentCharacteristics ?: run {
            Log.e(TAG, "captureRawPhoto called without camera characteristics")
            return false
        }
        val handler = backgroundHandler ?: run {
            Log.e(TAG, "captureRawPhoto called without background handler")
            return false
        }

        val imageLatch = CountDownLatch(1)
        val resultLatch = CountDownLatch(1)
        var captureResult: TotalCaptureResult? = null

        synchronized(rawPhotoLock) {
            latestRawPhotoData = null
            latestRawImage?.close()
            latestRawImage = null
            pendingRawImageLatch = imageLatch
        }

        return try {
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
                        captureResult = result
                        resultLatch.countDown()
                    }

                    override fun onCaptureFailed(
                        session: CameraCaptureSession,
                        request: CaptureRequest,
                        failure: android.hardware.camera2.CaptureFailure,
                    ) {
                        Log.e(TAG, "RAW still capture failed: $failure")
                        resultLatch.countDown()
                    }
                },
                handler,
            )

            val imageReady = imageLatch.await(PHOTO_TIMEOUT_SECONDS, TimeUnit.SECONDS)
            val resultReady = resultLatch.await(PHOTO_TIMEOUT_SECONDS, TimeUnit.SECONDS)
            if (!imageReady || !resultReady) {
                Log.e(TAG, "Timed out waiting for RAW still capture")
                synchronized(rawPhotoLock) {
                    pendingRawImageLatch = null
                }
                return false
            }

            val image = synchronized(rawPhotoLock) {
                pendingRawImageLatch = null
                val current = latestRawImage
                latestRawImage = null
                current
            } ?: run {
                Log.e(TAG, "RAW image was not available after capture")
                return false
            }

            val result = captureResult ?: run {
                image.close()
                Log.e(TAG, "RAW capture result was unavailable")
                return false
            }

            val dngBytes = try {
                val output = ByteArrayOutputStream()
                DngCreator(characteristics, result).use { creator ->
                    creator.writeImage(output, image)
                }
                output.toByteArray()
            } finally {
                image.close()
            }

            synchronized(rawPhotoLock) {
                latestRawPhotoData = dngBytes
            }

            true
        } catch (error: Exception) {
            Log.e(TAG, "Failed to capture RAW photo", error)
            synchronized(rawPhotoLock) {
                pendingRawImageLatch = null
                latestRawImage?.close()
                latestRawImage = null
            }
            false
        }
    }

    /**
     * Consume captured RAW photo bytes (DNG).
     */
    fun consumeRawPhotoData(): ByteArray? {
        synchronized(rawPhotoLock) {
            val data = latestRawPhotoData
            latestRawPhotoData = null
            return data
        }
    }

    /**
     * Start video recording via MediaRecorder.
     */
    fun startRecording(path: String): Boolean {
        if (isRecording) {
            Log.e(TAG, "startRecording called while already recording")
            return false
        }
        if (isRawVideoRecording) {
            Log.e(TAG, "startRecording called while RAW recording is active")
            return false
        }

        if (!prepareRecorder(path)) {
            return false
        }

        if (!createCaptureSession(includeRecorderSurface = true)) {
            releaseRecorder()
            return false
        }

        return try {
            mediaRecorder?.start()
            recordingStartElapsedRealtimeMs = SystemClock.elapsedRealtime()
            isRecording = true
            true
        } catch (error: Exception) {
            Log.e(TAG, "Failed to start MediaRecorder", error)
            releaseRecorder()
            stopCapture()
            createCaptureSession(includeRecorderSurface = false)
            false
        }
    }

    /**
     * Stop video recording.
     */
    fun stopRecording(): Boolean {
        return stopRecordingInternal(restorePreviewSession = true)
    }

    private fun stopRecordingInternal(restorePreviewSession: Boolean): Boolean {
        if (!isRecording) {
            return true
        }

        val recorder = mediaRecorder ?: run {
            Log.e(TAG, "stopRecording called with missing MediaRecorder")
            isRecording = false
            recordingStartElapsedRealtimeMs = 0
            return false
        }

        var stopped = true
        try {
            recorder.stop()
        } catch (error: RuntimeException) {
            Log.e(TAG, "Failed to stop MediaRecorder cleanly", error)
            stopped = false
        } catch (error: Exception) {
            Log.e(TAG, "Failed to stop MediaRecorder", error)
            stopped = false
        }

        isRecording = false
        recordingStartElapsedRealtimeMs = 0
        releaseRecorder()

        if (!restorePreviewSession) {
            stopCapture()
            return stopped
        }

        stopCapture()
        val previewRestored = createCaptureSession(includeRecorderSurface = false)
        return stopped && previewRestored
    }
    fun getRecordingDurationMs(): Long {
        if (!isRecording || recordingStartElapsedRealtimeMs == 0L) {
            return 0L
        }
        return (SystemClock.elapsedRealtime() - recordingStartElapsedRealtimeMs).coerceAtLeast(0L)
    }

    /**
     * Start RAW video frame stream recording.
     */
    fun startRawRecording(path: String): Boolean {
        val outputPath = path.trim()
        if (outputPath.isEmpty()) {
            Log.e(TAG, "RAW recording path must not be empty")
            return false
        }
        if (isRawVideoRecording) {
            Log.e(TAG, "startRawRecording called while already recording RAW")
            return false
        }
        if (isRecording) {
            Log.e(TAG, "startRawRecording called while standard recording is active")
            return false
        }

        return try {
            val file = File(outputPath)
            if (file.exists() && !file.delete()) {
                Log.e(TAG, "Failed to remove existing RAW output file: $outputPath")
                return false
            }
            file.parentFile?.mkdirs()
            val stream = FileOutputStream(file)
            writeRawVideoHeader(stream, frameWidth, frameHeight, frameRate, isBgra = false)
            synchronized(rawVideoLock) {
                rawVideoOutput = stream
                rawVideoRecordingStartElapsedRealtimeMs = SystemClock.elapsedRealtime()
                isRawVideoRecording = true
            }
            true
        } catch (error: Exception) {
            Log.e(TAG, "Failed to start RAW recording", error)
            stopRawVideoRecordingInternal()
            false
        }
    }

    /**
     * Stop RAW video frame stream recording.
     */
    fun stopRawRecording(): Boolean {
        return stopRawVideoRecordingInternal()
    }
    fun getRawRecordingDurationMs(): Long {
        if (!isRawVideoRecording || rawVideoRecordingStartElapsedRealtimeMs == 0L) {
            return 0L
        }
        return (SystemClock.elapsedRealtime() - rawVideoRecordingStartElapsedRealtimeMs).coerceAtLeast(0L)
    }

    /**
     * Get latest frame as RGBA bytes.
     * Returns null if no new frame is available.
     */
    fun getFrame(): ByteArray? {
        return frameQueue.pollFirst()
    }

    /**
     * Wait for the next available frame and consume it.
     * Returns null on timeout or if no frame is available.
     */
    fun waitForNextFrame(timeoutMs: Int): ByteArray? {
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
     * Get current frame size.
     */
    fun getFrameSize(): IntArray {
        return intArrayOf(frameWidth, frameHeight)
    }

    /**
     * Close camera resources.
     */
    fun closeCamera() {
        stopRecordingInternal(restorePreviewSession = false)
        stopCapture()

        cameraDevice?.close()
        cameraDevice = null

        previewImageReader?.close()
        previewImageReader = null
        stillImageReader?.close()
        stillImageReader = null
        rawImageReader?.close()
        rawImageReader = null

        releaseRecorder()
        stopRawVideoRecordingInternal()

        frameQueue.clear()
        synchronized(photoLock) {
            latestPhotoData = null
            pendingPhotoLatch?.countDown()
            pendingPhotoLatch = null
        }
        synchronized(rawPhotoLock) {
            latestRawPhotoData = null
            latestRawImage?.close()
            latestRawImage = null
            pendingRawImageLatch?.countDown()
            pendingRawImageLatch = null
        }

        currentCameraId = null
        currentCharacteristics = null
        cameraManager = null
        recordingStartElapsedRealtimeMs = 0L
        isRecording = false

        stopBackgroundThread()
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

    private fun createCaptureSession(includeRecorderSurface: Boolean): Boolean {
        val device = cameraDevice ?: run {
            Log.e(TAG, "createCaptureSession called before camera open")
            return false
        }
        val previewReader = previewImageReader ?: run {
            Log.e(TAG, "createCaptureSession called before preview reader init")
            return false
        }
        val stillReader = stillImageReader ?: run {
            Log.e(TAG, "createCaptureSession called before still reader init")
            return false
        }
        val handler = backgroundHandler ?: run {
            Log.e(TAG, "createCaptureSession called without background handler")
            return false
        }

        val surfaces = mutableListOf<Surface>(
            previewReader.surface,
            stillReader.surface,
        )
        rawImageReader?.surface?.let { surfaces.add(it) }

        if (includeRecorderSurface) {
            val surface = recorderSurface ?: run {
                Log.e(TAG, "Recorder surface is missing while starting recording session")
                return false
            }
            surfaces.add(surface)
        }

        stopCapture()

        val sessionLatch = CountDownLatch(1)
        var configured = false
        val callback = object : CameraCaptureSession.StateCallback() {
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
                    if (includeRecorderSurface) {
                        val recordingSurface = recorderSurface
                            ?: error("Recorder surface lost during session configuration")
                        builder.addTarget(recordingSurface)
                    }
                    applyRequestControls(builder, forStillCapture = false)
                    session.setRepeatingRequest(builder.build(), null, handler)
                    previewRequestBuilder = builder
                    configured = true
                } catch (error: Exception) {
                    Log.e(TAG, "Failed to configure repeating request", error)
                    previewRequestBuilder = null
                    configured = false
                }
                sessionLatch.countDown()
            }

            override fun onConfigureFailed(session: CameraCaptureSession) {
                Log.e(TAG, "Camera capture session configuration failed")
                sessionLatch.countDown()
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
                        callback,
                    ),
                )
            } else {
                device.createCaptureSession(surfaces, callback, handler)
            }
        } catch (error: Exception) {
            Log.e(TAG, "Failed to create capture session", error)
            return false
        }

        val completed = sessionLatch.await(SESSION_TIMEOUT_SECONDS, TimeUnit.SECONDS)
        if (!completed || !configured) {
            Log.e(TAG, "Timed out configuring camera capture session")
            stopCapture()
            return false
        }

        return true
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
            val recorder = MediaRecorder()
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

    private fun stopRawVideoRecordingInternal(): Boolean {
        val output = synchronized(rawVideoLock) {
            val stream = rawVideoOutput
            rawVideoOutput = null
            isRawVideoRecording = false
            rawVideoRecordingStartElapsedRealtimeMs = 0L
            stream
        }

        return try {
            output?.flush()
            output?.close()
            true
        } catch (error: Exception) {
            Log.e(TAG, "Failed to stop RAW recording stream", error)
            false
        }
    }

    private fun maybeWriteRawVideoFrame(
        rgba: ByteArray,
        width: Int,
        height: Int,
        timestampNs: Long,
    ) {
        val output = synchronized(rawVideoLock) {
            if (!isRawVideoRecording) {
                return
            }
            rawVideoOutput
        } ?: return

        try {
            val expected = width.toLong() * height.toLong() * 4L
            if (expected != rgba.size.toLong()) {
                Log.e(TAG, "RAW frame byte count mismatch: expected=$expected actual=${rgba.size}")
                return
            }
            writeU64LE(output, timestampNs)
            writeU32LE(output, rgba.size)
            output.write(rgba)
        } catch (error: Exception) {
            Log.e(TAG, "Failed writing RAW video frame", error)
            stopRawVideoRecordingInternal()
        }
    }

    private fun writeRawVideoHeader(
        output: FileOutputStream,
        width: Int,
        height: Int,
        fps: Int,
        isBgra: Boolean,
    ) {
        // Header layout:
        // magic(4)='WKRV', version(u8)=1, pixel_format(u8), reserved(u16)=0,
        // width(u32), height(u32), fps(u32)
        output.write(byteArrayOf('W'.code.toByte(), 'K'.code.toByte(), 'R'.code.toByte(), 'V'.code.toByte()))
        output.write(byteArrayOf(1, if (isBgra) 1 else 2))
        writeU16LE(output, 0)
        writeU32LE(output, width)
        writeU32LE(output, height)
        writeU32LE(output, fps)
    }

    private fun writeU16LE(output: FileOutputStream, value: Int) {
        output.write(byteArrayOf(
            (value and 0xFF).toByte(),
            ((value ushr 8) and 0xFF).toByte(),
        ))
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

    private fun yuv420ToRgba(image: Image): ByteArray {
        val width = image.width
        val height = image.height

        val yPlane = image.planes[0]
        val uPlane = image.planes[1]
        val vPlane = image.planes[2]

        val yBuffer = yPlane.buffer
        val uBuffer = uPlane.buffer
        val vBuffer = vPlane.buffer

        val yRowStride = yPlane.rowStride
        val yPixelStride = yPlane.pixelStride

        val uRowStride = uPlane.rowStride
        val uPixelStride = uPlane.pixelStride

        val vRowStride = vPlane.rowStride
        val vPixelStride = vPlane.pixelStride

        val rgba = ByteArray(width * height * 4)

        for (row in 0 until height) {
            val yRowOffset = row * yRowStride
            val uvRowOffsetU = (row / 2) * uRowStride
            val uvRowOffsetV = (row / 2) * vRowStride

            for (col in 0 until width) {
                val yIndex = yRowOffset + col * yPixelStride
                val uvCol = col / 2
                val uIndex = uvRowOffsetU + uvCol * uPixelStride
                val vIndex = uvRowOffsetV + uvCol * vPixelStride

                val y = yBuffer.get(yIndex).toInt() and 0xFF
                val u = uBuffer.get(uIndex).toInt() and 0xFF
                val v = vBuffer.get(vIndex).toInt() and 0xFF

                val c = y - 16
                val d = u - 128
                val e = v - 128

                val r = ((298 * c + 409 * e + 128) shr 8).coerceIn(0, 255)
                val g = ((298 * c - 100 * d - 208 * e + 128) shr 8).coerceIn(0, 255)
                val b = ((298 * c + 516 * d + 128) shr 8).coerceIn(0, 255)

                val rgbaIndex = (row * width + col) * 4
                rgba[rgbaIndex] = r.toByte()
                rgba[rgbaIndex + 1] = g.toByte()
                rgba[rgbaIndex + 2] = b.toByte()
                rgba[rgbaIndex + 3] = 0xFF.toByte()
            }
        }

        return rgba
    }
}

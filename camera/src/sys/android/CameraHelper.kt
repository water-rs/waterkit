package waterkit.camera

import android.content.Context
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CaptureRequest
import android.media.ImageReader
import android.graphics.ImageFormat
import android.os.Handler
import android.os.HandlerThread
import android.view.Surface

/**
 * Camera helper for waterkit-camera crate.
 * Uses Camera2 API for camera enumeration and streaming.
 */
object CameraHelper {
    private var cameraDevice: CameraDevice? = null
    private var captureSession: CameraCaptureSession? = null
    private var imageReader: ImageReader? = null
    private var backgroundThread: HandlerThread? = null
    private var backgroundHandler: Handler? = null
    private var latestFrame: ByteArray? = null
    private var frameWidth: Int = 1280
    private var frameHeight: Int = 720
    private val frameLock = Object()

    /**
     * List available cameras.
     * Returns array of [id, name, isFrontFacing] arrays.
     */
    @JvmStatic
    fun listCameras(context: Context): Array<Array<String>> {
        val cameraManager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        val cameras = mutableListOf<Array<String>>()
        
        for (cameraId in cameraManager.cameraIdList) {
            val characteristics = cameraManager.getCameraCharacteristics(cameraId)
            val facing = characteristics.get(CameraCharacteristics.LENS_FACING)
            val isFront = facing == CameraCharacteristics.LENS_FACING_FRONT
            val name = if (isFront) "Front Camera" else "Back Camera"
            
            cameras.add(arrayOf(cameraId, name, isFront.toString()))
        }
        
        return cameras.toTypedArray()
    }

    /**
     * Open a camera by ID.
     */
    @JvmStatic
    fun openCamera(context: Context, cameraId: String): Boolean {
        try {
            startBackgroundThread()
            
            val cameraManager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
            
            // Create ImageReader for frame capture
            imageReader = ImageReader.newInstance(
                frameWidth, frameHeight,
                ImageFormat.YUV_420_888, 2
            )
            
            imageReader?.setOnImageAvailableListener({ reader ->
                val image = reader.acquireLatestImage()
                if (image != null) {
                    try {
                        // Convert YUV to RGBA
                        val yBuffer = image.planes[0].buffer
                        val uBuffer = image.planes[1].buffer
                        val vBuffer = image.planes[2].buffer
                        
                        val ySize = yBuffer.remaining()
                        val uSize = uBuffer.remaining()
                        val vSize = vBuffer.remaining()
                        
                        val nv21 = ByteArray(ySize + uSize + vSize)
                        yBuffer.get(nv21, 0, ySize)
                        vBuffer.get(nv21, ySize, vSize)
                        uBuffer.get(nv21, ySize + vSize, uSize)
                        
                        // Convert NV21 to RGBA
                        val rgba = convertNV21ToRGBA(nv21, image.width, image.height)
                        
                        synchronized(frameLock) {
                            latestFrame = rgba
                        }
                    } finally {
                        image.close()
                    }
                }
            }, backgroundHandler)
            
            // Open camera (requires permission already granted)
            cameraManager.openCamera(cameraId, object : CameraDevice.StateCallback() {
                override fun onOpened(camera: CameraDevice) {
                    cameraDevice = camera
                }
                
                override fun onDisconnected(camera: CameraDevice) {
                    camera.close()
                    cameraDevice = null
                }
                
                override fun onError(camera: CameraDevice, error: Int) {
                    camera.close()
                    cameraDevice = null
                }
            }, backgroundHandler)
            
            return true
        } catch (e: Exception) {
            e.printStackTrace()
            return false
        }
    }

    /**
     * Start capturing frames.
     */
    @JvmStatic
    fun startCapture(): Boolean {
        val device = cameraDevice ?: return false
        val reader = imageReader ?: return false
        
        try {
            val surface = reader.surface
            
            device.createCaptureSession(
                listOf(surface),
                object : CameraCaptureSession.StateCallback() {
                    override fun onConfigured(session: CameraCaptureSession) {
                        captureSession = session
                        
                        val captureRequest = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW)
                        captureRequest.addTarget(surface)
                        captureRequest.set(CaptureRequest.CONTROL_MODE, CaptureRequest.CONTROL_MODE_AUTO)
                        
                        session.setRepeatingRequest(captureRequest.build(), null, backgroundHandler)
                    }
                    
                    override fun onConfigureFailed(session: CameraCaptureSession) {
                        // Configuration failed
                    }
                },
                backgroundHandler
            )
            
            return true
        } catch (e: Exception) {
            e.printStackTrace()
            return false
        }
    }

    /**
     * Stop capturing frames.
     */
    @JvmStatic
    fun stopCapture() {
        captureSession?.close()
        captureSession = null
    }

    /**
     * Get the latest captured frame as RGBA bytes.
     */
    @JvmStatic
    fun getFrame(): ByteArray? {
        synchronized(frameLock) {
            val frame = latestFrame
            latestFrame = null
            return frame
        }
    }

    /**
     * Get current frame dimensions.
     */
    @JvmStatic
    fun getFrameSize(): IntArray {
        return intArrayOf(frameWidth, frameHeight)
    }

    /**
     * Close the camera.
     */
    @JvmStatic
    fun closeCamera() {
        captureSession?.close()
        captureSession = null
        cameraDevice?.close()
        cameraDevice = null
        imageReader?.close()
        imageReader = null
        stopBackgroundThread()
    }

    private fun startBackgroundThread() {
        backgroundThread = HandlerThread("CameraBackground").also { it.start() }
        backgroundHandler = Handler(backgroundThread!!.looper)
    }

    private fun stopBackgroundThread() {
        backgroundThread?.quitSafely()
        try {
            backgroundThread?.join()
            backgroundThread = null
            backgroundHandler = null
        } catch (e: InterruptedException) {
            e.printStackTrace()
        }
    }

    /**
     * Convert NV21 (YUV 4:2:0) to RGBA.
     */
    private fun convertNV21ToRGBA(nv21: ByteArray, width: Int, height: Int): ByteArray {
        val rgba = ByteArray(width * height * 4)
        val frameSize = width * height
        
        for (j in 0 until height) {
            for (i in 0 until width) {
                val yIndex = j * width + i
                val uvIndex = frameSize + (j / 2) * width + (i / 2) * 2
                
                val y = (nv21[yIndex].toInt() and 0xFF) - 16
                val v = (nv21[uvIndex].toInt() and 0xFF) - 128
                val u = (nv21[uvIndex + 1].toInt() and 0xFF) - 128
                
                var r = (1.164 * y + 1.596 * v).toInt()
                var g = (1.164 * y - 0.813 * v - 0.391 * u).toInt()
                var b = (1.164 * y + 2.018 * u).toInt()
                
                r = r.coerceIn(0, 255)
                g = g.coerceIn(0, 255)
                b = b.coerceIn(0, 255)
                
                val rgbaIndex = (j * width + i) * 4
                rgba[rgbaIndex] = r.toByte()
                rgba[rgbaIndex + 1] = g.toByte()
                rgba[rgbaIndex + 2] = b.toByte()
                rgba[rgbaIndex + 3] = 255.toByte()
            }
        }
        
        return rgba
    }
}

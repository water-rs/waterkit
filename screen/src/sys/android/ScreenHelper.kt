package waterkit.screen

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.PixelFormat
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.ImageReader
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Handler
import android.os.HandlerThread
import android.provider.Settings
import android.util.DisplayMetrics
import android.view.WindowManager

/**
 * Screen capture helper for waterkit-screen crate.
 * Uses MediaProjection API for screen capture on Android.
 *
 * Usage:
 * 1. Call initWithContext() with app context
 * 2. Call requestPermission() from an Activity to get the permission intent
 * 3. Start the intent with startActivityForResult() with REQUEST_CODE
 * 4. In onActivityResult, call onPermissionResult() with the result
 * 5. Call startCapture() to begin capturing
 * 6. Call getFrame() to get the latest captured frame
 * 7. Call stopCapture() when done
 */
object ScreenHelper {
    const val REQUEST_CODE = 1001

    private var context: Context? = null
    private var projectionManager: MediaProjectionManager? = null
    private var mediaProjection: MediaProjection? = null
    private var virtualDisplay: VirtualDisplay? = null
    private var imageReader: ImageReader? = null
    private var backgroundThread: HandlerThread? = null
    private var backgroundHandler: Handler? = null

    private var latestFrame: ByteArray? = null
    private var frameWidth: Int = 0
    private var frameHeight: Int = 0
    private val frameLock = Object()

    private var screenDensity: Int = 0
    private var screenWidth: Int = 0
    private var screenHeight: Int = 0

    /**
     * Initialize with application context.
     */
    @JvmStatic
    fun initWithContext(ctx: Context): Boolean {
        return try {
            context = ctx.applicationContext
            projectionManager = ctx.getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager

            // Get screen metrics
            val windowManager = ctx.getSystemService(Context.WINDOW_SERVICE) as WindowManager
            val metrics = DisplayMetrics()
            @Suppress("DEPRECATION")
            windowManager.defaultDisplay.getRealMetrics(metrics)

            screenDensity = metrics.densityDpi
            screenWidth = metrics.widthPixels
            screenHeight = metrics.heightPixels
            frameWidth = screenWidth
            frameHeight = screenHeight

            true
        } catch (e: Exception) {
            e.printStackTrace()
            false
        }
    }

    /**
     * Get the screen capture permission intent.
     * This must be started with startActivityForResult().
     */
    @JvmStatic
    fun getPermissionIntent(): Intent? {
        return projectionManager?.createScreenCaptureIntent()
    }

    /**
     * Handle the permission result from onActivityResult.
     * Returns true if permission was granted.
     */
    @JvmStatic
    fun onPermissionResult(resultCode: Int, data: Intent?): Boolean {
        if (resultCode != Activity.RESULT_OK || data == null) {
            return false
        }

        try {
            mediaProjection = projectionManager?.getMediaProjection(resultCode, data)
            return mediaProjection != null
        } catch (e: Exception) {
            e.printStackTrace()
            return false
        }
    }

    /**
     * Start screen capture.
     * Must be called after onPermissionResult() returns true.
     */
    @JvmStatic
    fun startCapture(): Boolean {
        val projection = mediaProjection ?: return false

        return try {
            startBackgroundThread()

            // Create ImageReader for capturing frames
            imageReader = ImageReader.newInstance(
                frameWidth, frameHeight,
                PixelFormat.RGBA_8888, 2
            )

            imageReader?.setOnImageAvailableListener({ reader ->
                val image = reader.acquireLatestImage()
                if (image != null) {
                    try {
                        val planes = image.planes
                        val buffer = planes[0].buffer
                        val pixelStride = planes[0].pixelStride
                        val rowStride = planes[0].rowStride
                        val rowPadding = rowStride - pixelStride * frameWidth

                        // Create bitmap from the image
                        val bitmap = Bitmap.createBitmap(
                            frameWidth + rowPadding / pixelStride,
                            frameHeight,
                            Bitmap.Config.ARGB_8888
                        )
                        bitmap.copyPixelsFromBuffer(buffer)

                        // Crop if there's padding
                        val croppedBitmap = if (rowPadding > 0) {
                            Bitmap.createBitmap(bitmap, 0, 0, frameWidth, frameHeight)
                        } else {
                            bitmap
                        }

                        // Convert to RGBA byte array
                        val rgba = ByteArray(frameWidth * frameHeight * 4)
                        val intArray = IntArray(frameWidth * frameHeight)
                        croppedBitmap.getPixels(intArray, 0, frameWidth, 0, 0, frameWidth, frameHeight)

                        for (i in intArray.indices) {
                            val pixel = intArray[i]
                            rgba[i * 4] = ((pixel shr 16) and 0xFF).toByte() // R
                            rgba[i * 4 + 1] = ((pixel shr 8) and 0xFF).toByte() // G
                            rgba[i * 4 + 2] = (pixel and 0xFF).toByte() // B
                            rgba[i * 4 + 3] = ((pixel shr 24) and 0xFF).toByte() // A
                        }

                        synchronized(frameLock) {
                            latestFrame = rgba
                        }

                        if (croppedBitmap !== bitmap) {
                            croppedBitmap.recycle()
                        }
                        bitmap.recycle()
                    } finally {
                        image.close()
                    }
                }
            }, backgroundHandler)

            // Create VirtualDisplay
            virtualDisplay = projection.createVirtualDisplay(
                "WaterkitScreen",
                frameWidth, frameHeight, screenDensity,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
                imageReader?.surface, null, backgroundHandler
            )

            true
        } catch (e: Exception) {
            e.printStackTrace()
            false
        }
    }

    /**
     * Stop screen capture.
     */
    @JvmStatic
    fun stopCapture() {
        virtualDisplay?.release()
        virtualDisplay = null
        imageReader?.close()
        imageReader = null
        mediaProjection?.stop()
        mediaProjection = null
        stopBackgroundThread()
    }

    /**
     * Get the latest captured frame as RGBA bytes.
     * Returns null if no frame is available.
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
    fun getFrameDimensions(): IntArray {
        return intArrayOf(frameWidth, frameHeight)
    }

    /**
     * Get screen brightness (0.0 to 1.0).
     */
    @JvmStatic
    fun getBrightness(): Float {
        val ctx = context ?: return 1.0f
        return try {
            val brightness = Settings.System.getInt(
                ctx.contentResolver,
                Settings.System.SCREEN_BRIGHTNESS
            )
            brightness / 255.0f
        } catch (e: Exception) {
            1.0f
        }
    }

    /**
     * Set screen brightness (0.0 to 1.0).
     * Note: Requires WRITE_SETTINGS permission.
     */
    @JvmStatic
    fun setBrightness(value: Float): Boolean {
        val ctx = context ?: return false
        return try {
            val brightness = (value.coerceIn(0.0f, 1.0f) * 255).toInt()
            Settings.System.putInt(
                ctx.contentResolver,
                Settings.System.SCREEN_BRIGHTNESS,
                brightness
            )
            true
        } catch (e: Exception) {
            e.printStackTrace()
            false
        }
    }

    /**
     * Check if MediaProjection permission has been granted.
     */
    @JvmStatic
    fun hasPermission(): Boolean {
        return mediaProjection != null
    }

    private fun startBackgroundThread() {
        backgroundThread = HandlerThread("ScreenCapture").also { it.start() }
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
}

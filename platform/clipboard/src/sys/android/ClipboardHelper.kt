package waterkit.clipboard

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import java.io.File
import java.io.InputStream

/**
 * Helper class for clipboard operations on Android.
 *
 * Note: Some operations (setImageFromPath, setBinary) require the host app to configure
 * a FileProvider in AndroidManifest.xml for proper URI sharing.
 */
object ClipboardHelper {

    /**
     * The clipboard service. Its absence is a failure the Rust side reports,
     * never an empty clipboard; the exception reaches it through JNI.
     */
    private fun clipboard(context: Context): ClipboardManager =
        checkNotNull(context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager) {
            "the clipboard service is unavailable"
        }

    // ============== Query Operations ==============

    @JvmStatic
    fun hasText(context: Context): Boolean {
        val clipboard = clipboard(context)
        val description = clipboard.primaryClipDescription ?: return false
        return description.hasMimeType("text/plain")
    }

    @JvmStatic
    fun hasHtml(context: Context): Boolean {
        val clipboard = clipboard(context)
        val description = clipboard.primaryClipDescription ?: return false
        return description.hasMimeType("text/html")
    }

    @JvmStatic
    fun hasImage(context: Context): Boolean {
        val clipboard = clipboard(context)
        val description = clipboard.primaryClipDescription ?: return false
        return description.hasMimeType("image/*")
    }

    @JvmStatic
    fun hasFiles(context: Context): Boolean {
        val clipboard = clipboard(context)
        val clip = clipboard.primaryClip ?: return false
        if (clip.itemCount == 0) return false
        val uri = clip.getItemAt(0).uri ?: return false
        return uri.scheme == "file"
    }

    // ============== Read Operations ==============

    @JvmStatic
    fun getText(context: Context): String? {
        val clipboard = clipboard(context)
        val clip = clipboard.primaryClip ?: return null
        if (clip.itemCount == 0) return null
        return clip.getItemAt(0).text?.toString()
    }

    @JvmStatic
    fun getHtml(context: Context): String? {
        val clipboard = clipboard(context)
        val clip = clipboard.primaryClip ?: return null
        if (clip.itemCount == 0) return null
        return clip.getItemAt(0).htmlText
    }

    @JvmStatic
    fun getFileUri(context: Context): String? {
        val clipboard = clipboard(context)
        val clip = clipboard.primaryClip ?: return null
        if (clip.itemCount == 0) return null
        val uri = clip.getItemAt(0).uri ?: return null
        if (uri.scheme == "file") {
            return uri.toString()
        }
        return null
    }

    /**
     * Get image width from clipboard.
     * Returns -1 if no image is available.
     */
    @JvmStatic
    fun getImageWidth(context: Context): Int {
        val bitmap = getClipboardBitmap(context) ?: return -1
        return bitmap.width
    }

    /**
     * Get image height from clipboard.
     * Returns -1 if no image is available.
     */
    @JvmStatic
    fun getImageHeight(context: Context): Int {
        val bitmap = getClipboardBitmap(context) ?: return -1
        return bitmap.height
    }

    /**
     * Get image as RGBA byte array.
     * Returns null if no image is available.
     */
    @JvmStatic
    fun getImageRgba(context: Context): ByteArray? {
        val bitmap = getClipboardBitmap(context) ?: return null

        val width = bitmap.width
        val height = bitmap.height
        val rgba = ByteArray(width * height * 4)

        // Convert bitmap to RGBA
        val pixels = IntArray(width * height)
        bitmap.getPixels(pixels, 0, width, 0, 0, width, height)

        for (i in pixels.indices) {
            val pixel = pixels[i]
            // Android ARGB -> RGBA
            rgba[i * 4] = ((pixel shr 16) and 0xFF).toByte()     // R
            rgba[i * 4 + 1] = ((pixel shr 8) and 0xFF).toByte()  // G
            rgba[i * 4 + 2] = (pixel and 0xFF).toByte()          // B
            rgba[i * 4 + 3] = ((pixel shr 24) and 0xFF).toByte() // A
        }

        return rgba
    }

    /**
     * Get binary data for a specific MIME type.
     */
    @JvmStatic
    fun getBinary(context: Context, mime: String): ByteArray? {
        val clipboard = clipboard(context)
        val clip = clipboard.primaryClip ?: return null
        if (clip.itemCount == 0) return null

        val item = clip.getItemAt(0)
        val uri = item.uri ?: return null

        return openUri(context, uri).use { inputStream -> inputStream.readBytes() }
    }

    private fun getClipboardBitmap(context: Context): Bitmap? {
        val clipboard = clipboard(context)
        val clip = clipboard.primaryClip ?: return null
        if (clip.itemCount == 0) return null

        val item = clip.getItemAt(0)
        val uri = item.uri ?: return null

        // `decodeStream` answers null for content that is not an image.
        return openUri(context, uri).use { inputStream -> BitmapFactory.decodeStream(inputStream) }
    }

    /**
     * Open the content of [uri]. Failures, such as a file that no longer
     * exists or a provider that denies access, throw to the caller.
     */
    private fun openUri(context: Context, uri: Uri): InputStream =
        checkNotNull(context.contentResolver.openInputStream(uri)) {
            "the content provider of $uri crashed"
        }

    // ============== Write Operations ==============

    @JvmStatic
    fun setText(context: Context, text: String) {
        val clipboard = clipboard(context)
        val clip = ClipData.newPlainText("text", text)
        clipboard.setPrimaryClip(clip)
    }

    @JvmStatic
    fun setHtml(context: Context, html: String, altText: String) {
        val clipboard = clipboard(context)
        val plainText = if (altText.isNotEmpty()) {
            altText
        } else {
            android.text.Html.fromHtml(html, android.text.Html.FROM_HTML_MODE_COMPACT).toString()
        }
        val clip = ClipData.newHtmlText("html", plainText, html)
        clipboard.setPrimaryClip(clip)
    }

    @JvmStatic
    fun setFileUri(context: Context, uri: String) {
        val clipboard = clipboard(context)
        val clip = ClipData.newRawUri("file", Uri.parse(uri))
        clipboard.setPrimaryClip(clip)
    }

    /**
     * Set image from a file path.
     * Returns false if the file does not exist; any other failure throws.
     *
     * Note: This uses a file:// URI which only works within the same app.
     * For cross-app sharing, the host app must implement FileProvider.
     */
    @JvmStatic
    fun setImageFromPath(context: Context, path: String): Boolean {
        val file = File(path)
        if (!file.exists()) return false

        // Use file:// URI (works within app, but not for cross-app sharing)
        val uri = Uri.fromFile(file)

        val clip = ClipData.newUri(context.contentResolver, "image", uri)
        clipboard(context).setPrimaryClip(clip)
        return true
    }

    /**
     * Set binary data with MIME type.
     *
     * Note: This saves data to cache and uses a file:// URI which only works within the same app.
     * For cross-app sharing, the host app must implement FileProvider.
     */
    @JvmStatic
    fun setBinary(context: Context, data: ByteArray, mime: String) {
        // Save to cache file
        val cacheDir = context.cacheDir
        val extension = mime.substringAfter("/", "bin")
        val dataFile = File(cacheDir, "clipboard_data.$extension")
        dataFile.writeBytes(data)

        // Use file:// URI (works within app, but not for cross-app sharing)
        val uri = Uri.fromFile(dataFile)

        val clip = ClipData.newUri(context.contentResolver, "data", uri)
        clipboard(context).setPrimaryClip(clip)
    }

    // ============== Watch Operations ==============

    /**
     * Register a primary-clip-change listener.
     *
     * `OnPrimaryClipChangedListener` is available on every Android version
     * WaterKit supports (API 11+), and `ClipboardManager` delivers each clip
     * notification on the main thread — including changes whose MIME type set
     * matches the previous clip.
     *
     * Returns false when the clipboard service is unavailable.
     */
    @JvmStatic
    fun startWatching(
        context: Context,
        listener: ClipboardManager.OnPrimaryClipChangedListener
    ): Boolean {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return false
        clipboard.addPrimaryClipChangedListener(listener)
        return true
    }

    /**
     * Unregister a listener previously registered by [startWatching].
     */
    @JvmStatic
    fun stopWatching(
        context: Context,
        listener: ClipboardManager.OnPrimaryClipChangedListener
    ) {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return
        clipboard.removePrimaryClipChangedListener(listener)
    }

    // ============== Control Operations ==============

    @JvmStatic
    fun clear(context: Context) {
        val clipboard = clipboard(context)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            clipboard.clearPrimaryClip()
        } else {
            clipboard.setPrimaryClip(ClipData.newPlainText("", ""))
        }
    }
}

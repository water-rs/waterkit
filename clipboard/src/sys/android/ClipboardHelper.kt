package waterkit.clipboard

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import java.io.File

/**
 * Helper class for clipboard operations on Android.
 */
object ClipboardHelper {

    // ============== Query Operations ==============

    @JvmStatic
    fun hasText(context: Context): Boolean {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return false
        val description = clipboard.primaryClipDescription ?: return false
        return description.hasMimeType("text/plain")
    }

    @JvmStatic
    fun hasHtml(context: Context): Boolean {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return false
        val description = clipboard.primaryClipDescription ?: return false
        return description.hasMimeType("text/html")
    }

    @JvmStatic
    fun hasImage(context: Context): Boolean {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return false
        val description = clipboard.primaryClipDescription ?: return false
        return description.hasMimeType("image/*")
    }

    @JvmStatic
    fun hasFiles(context: Context): Boolean {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return false
        val clip = clipboard.primaryClip ?: return false
        if (clip.itemCount == 0) return false
        val uri = clip.getItemAt(0).uri ?: return false
        return uri.scheme == "file"
    }

    // ============== Read Operations ==============

    @JvmStatic
    fun getText(context: Context): String? {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return null
        val clip = clipboard.primaryClip ?: return null
        if (clip.itemCount == 0) return null
        return clip.getItemAt(0).text?.toString()
    }

    @JvmStatic
    fun getHtml(context: Context): String? {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return null
        val clip = clipboard.primaryClip ?: return null
        if (clip.itemCount == 0) return null
        return clip.getItemAt(0).htmlText
    }

    @JvmStatic
    fun getFileUri(context: Context): String? {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return null
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
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return null
        val clip = clipboard.primaryClip ?: return null
        if (clip.itemCount == 0) return null

        val item = clip.getItemAt(0)
        val uri = item.uri ?: return null

        return try {
            context.contentResolver.openInputStream(uri)?.use { inputStream ->
                inputStream.readBytes()
            }
        } catch (e: Exception) {
            null
        }
    }

    private fun getClipboardBitmap(context: Context): Bitmap? {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return null
        val clip = clipboard.primaryClip ?: return null
        if (clip.itemCount == 0) return null

        val item = clip.getItemAt(0)
        val uri = item.uri ?: return null

        return try {
            context.contentResolver.openInputStream(uri)?.use { inputStream ->
                BitmapFactory.decodeStream(inputStream)
            }
        } catch (e: Exception) {
            null
        }
    }

    // ============== Write Operations ==============

    @JvmStatic
    fun setText(context: Context, text: String) {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return
        val clip = ClipData.newPlainText("text", text)
        clipboard.setPrimaryClip(clip)
    }

    @JvmStatic
    fun setHtml(context: Context, html: String, altText: String) {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return
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
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return
        val clip = ClipData.newRawUri("file", Uri.parse(uri))
        clipboard.setPrimaryClip(clip)
    }

    /**
     * Set image from a file path.
     * Returns true if successful.
     */
    @JvmStatic
    fun setImageFromPath(context: Context, path: String): Boolean {
        return try {
            val bitmap = BitmapFactory.decodeFile(path) ?: return false

            // Save to cache and create content URI
            val cacheDir = context.cacheDir
            val imageFile = File(cacheDir, "clipboard_image.png")
            imageFile.outputStream().use { out ->
                bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
            }

            val uri = androidx.core.content.FileProvider.getUriForFile(
                context,
                "${context.packageName}.fileprovider",
                imageFile
            )

            val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                ?: return false
            val clip = ClipData.newUri(context.contentResolver, "image", uri)
            clipboard.setPrimaryClip(clip)
            true
        } catch (e: Exception) {
            false
        }
    }

    /**
     * Set binary data with MIME type.
     */
    @JvmStatic
    fun setBinary(context: Context, data: ByteArray, mime: String) {
        try {
            // Save to cache file
            val cacheDir = context.cacheDir
            val extension = mime.substringAfter("/", "bin")
            val dataFile = File(cacheDir, "clipboard_data.$extension")
            dataFile.writeBytes(data)

            val uri = androidx.core.content.FileProvider.getUriForFile(
                context,
                "${context.packageName}.fileprovider",
                dataFile
            )

            val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                ?: return
            val clip = ClipData.newUri(context.contentResolver, "data", uri)
            clipboard.setPrimaryClip(clip)
        } catch (e: Exception) {
            // Ignore errors
        }
    }

    // ============== Control Operations ==============

    @JvmStatic
    fun clear(context: Context) {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            clipboard.clearPrimaryClip()
        } else {
            clipboard.setPrimaryClip(ClipData.newPlainText("", ""))
        }
    }
}

package waterkit.clipboard

import android.content.ClipData
import android.content.ClipboardManager
import android.content.ContentResolver
import android.content.Context
import android.content.pm.PackageManager
import android.content.pm.ProviderInfo
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import java.io.File
import java.io.InputStream

/**
 * Helper class for clipboard operations on Android.
 *
 * Files, images and binary data go on the clipboard as [ClipboardFileProvider]
 * URIs, which the app's manifest must declare as that class documents.
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
    fun hasFiles(context: Context): Boolean = getFiles(context).isNotEmpty()

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

    /**
     * The paths of the local files the clip names, decoded by [Uri.getPath]:
     * those of `file://` URIs and of [ClipboardFileProvider] URIs this app
     * wrote. Other `content://` URIs name no path.
     */
    @JvmStatic
    fun getFiles(context: Context): Array<String> {
        val clip = clipboard(context).primaryClip ?: return emptyArray()
        val authority by lazy { fileProvider(context)?.authority }
        return (0 until clip.itemCount).mapNotNull { index ->
            val uri = clip.getItemAt(index).uri ?: return@mapNotNull null
            when {
                uri.scheme == ContentResolver.SCHEME_FILE -> uri.path
                uri.scheme == ContentResolver.SCHEME_CONTENT && uri.authority == authority ->
                    uri.path
                else -> null
            }
        }.toTypedArray()
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

    /**
     * Put the files at the absolute [paths] on the clipboard, one item per
     * file, as [ClipboardFileProvider] URIs that other apps can open.
     */
    @JvmStatic
    fun setFiles(context: Context, paths: Array<String>) {
        val resolver = context.contentResolver
        val uris = providerUris(context, paths.asList())
        val clip = ClipData.newUri(resolver, "files", uris.first())
        uris.drop(1).forEach { uri -> clip.addItem(resolver, ClipData.Item(uri)) }
        clipboard(context).setPrimaryClip(clip)
    }

    /**
     * The [ClipboardFileProvider] URIs of the files at the absolute [paths].
     * Throws when the app's manifest does not declare the provider as it
     * documents.
     */
    private fun providerUris(context: Context, paths: List<String>): List<Uri> {
        val provider = checkNotNull(fileProvider(context)) {
            "copying files needs the app's manifest to declare " +
                "<provider android:name=\"${ClipboardFileProvider::class.java.name}\" " +
                "android:authorities=\"\${applicationId}.waterkit.clipboard\" " +
                "android:exported=\"false\" android:grantUriPermissions=\"true\" />"
        }
        check(!provider.exported && provider.grantUriPermissions) {
            "${provider.name} must be declared with android:exported=\"false\" and " +
                "android:grantUriPermissions=\"true\", so that only the apps the clipboard " +
                "grants read access to can open the files"
        }
        return paths.map { path -> ClipboardFileProvider.uri(provider.authority, path) }
    }

    /** This app's declaration of [ClipboardFileProvider], if its manifest has one. */
    private fun fileProvider(context: Context): ProviderInfo? {
        val packageManager = context.packageManager
        val packageInfo = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            packageManager.getPackageInfo(
                context.packageName,
                PackageManager.PackageInfoFlags.of(PackageManager.GET_PROVIDERS.toLong()),
            )
        } else {
            @Suppress("DEPRECATION")
            packageManager.getPackageInfo(context.packageName, PackageManager.GET_PROVIDERS)
        }
        return packageInfo.providers?.find { it.name == ClipboardFileProvider::class.java.name }
    }

    /**
     * Set image from a file path.
     * Returns false if the file does not exist; any other failure throws.
     */
    @JvmStatic
    fun setImageFromPath(context: Context, path: String): Boolean {
        val file = File(path)
        if (!file.exists()) return false

        val uri = providerUris(context, listOf(file.absolutePath)).single()
        val clip = ClipData.newUri(context.contentResolver, "image", uri)
        clipboard(context).setPrimaryClip(clip)
        return true
    }

    /**
     * Set binary data with MIME type, saved to a file in the app's cache.
     */
    @JvmStatic
    fun setBinary(context: Context, data: ByteArray, mime: String) {
        // Save to cache file
        val cacheDir = context.cacheDir
        val extension = mime.substringAfter("/", "bin")
        val dataFile = File(cacheDir, "clipboard_data.$extension")
        dataFile.writeBytes(data)

        val uri = providerUris(context, listOf(dataFile.absolutePath)).single()
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

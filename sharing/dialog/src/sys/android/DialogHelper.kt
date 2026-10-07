package waterkit.dialog

import android.app.AlertDialog
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Dialog utilities for Android.
 */
class DialogHelper {
    companion object {
        @JvmStatic
        fun showDialog(context: Context, title: String, message: String) {
            if (Looper.myLooper() == Looper.getMainLooper()) {
                 // Called on main thread, cannot block.
                 // Show async as best effort.
                 AlertDialog.Builder(context)
                     .setTitle(title)
                     .setMessage(message)
                     .setPositiveButton("OK", null)
                     .show()
                 return
            }

            val latch = CountDownLatch(1)

            Handler(Looper.getMainLooper()).post {
                try {
                    AlertDialog.Builder(context)
                        .setTitle(title)
                        .setMessage(message)
                        .setPositiveButton("OK", null)
                        .setOnDismissListener { latch.countDown() }
                        .show()
                } catch (e: Exception) {
                    e.printStackTrace()
                    latch.countDown()
                }
            }

            try {
                latch.await()
            } catch (e: InterruptedException) {
                e.printStackTrace()
            }
        }


        @JvmStatic
        fun showConfirm(context: Context, title: String, message: String): Boolean {
            if (Looper.myLooper() == Looper.getMainLooper()) {
                 return false
            }

            val latch = CountDownLatch(1)
            val result = AtomicBoolean(false)

            Handler(Looper.getMainLooper()).post {
                try {
                    AlertDialog.Builder(context)
                        .setTitle(title)
                        .setMessage(message)
                        .setPositiveButton("OK") { _, _ ->
                            result.set(true)
                        }
                        .setNegativeButton("Cancel") { _, _ ->
                            result.set(false)
                        }
                        .setOnDismissListener { latch.countDown() }
                        .show()
                } catch (e: Exception) {
                    e.printStackTrace()
                    latch.countDown()
                }
            }

            try {
                latch.await()
            } catch (e: InterruptedException) {
                e.printStackTrace()
            }
            return result.get()
        }

        @JvmStatic
        fun photoPickIntent(type: Int): Intent {
            val intent = Intent(Intent.ACTION_GET_CONTENT)
            intent.addCategory(Intent.CATEGORY_OPENABLE)
            if (type == 1) {
                intent.type = "video/*"
            } else {
                intent.type = "image/*"
            }
            return intent
        }

        @JvmStatic
        fun openDocumentIntent(extensions: Array<String>, allowMultiple: Boolean): Intent {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                putExtra(Intent.EXTRA_ALLOW_MULTIPLE, allowMultiple)
            }

            val mimeTypes = extensions
                .map { it.trim().lowercase() }
                .filter { it.isNotEmpty() }
                .mapNotNull { extension ->
                    MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension)
                }
                .distinct()

            when (mimeTypes.size) {
                0 -> intent.type = "*/*"
                1 -> intent.type = mimeTypes[0]
                else -> {
                    intent.type = "*/*"
                    intent.putExtra(Intent.EXTRA_MIME_TYPES, mimeTypes.toTypedArray())
                }
            }
            return intent
        }

        @JvmStatic
        fun selectedUri(data: Intent): String? {
            return data.data?.toString()
                ?: data.clipData?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.uri?.toString()
        }

        @JvmStatic
        fun selectedUris(data: Intent): Array<String> {
            val clipData = data.clipData
            if (clipData != null && clipData.itemCount > 0) {
                return Array(clipData.itemCount) { index ->
                    clipData.getItemAt(index).uri.toString()
                }
            }
            return data.data?.toString()?.let { arrayOf(it) } ?: emptyArray()
        }

        @JvmStatic
        fun loadMedia(context: Context, uriString: String): String? {
            val uri = android.net.Uri.parse(uriString)
            return copyUriToCache(context, uri)
        }

        private fun copyUriToCache(ctx: Context, uri: android.net.Uri): String? {
            try {
                val inputStream = ctx.contentResolver.openInputStream(uri) ?: return null
                val extension = inferUriExtension(ctx, uri)
                val fileName = buildString {
                    append("picked_media_")
                    append(System.currentTimeMillis())
                    extension?.let {
                        append('.')
                        append(it)
                    }
                }
                val file = java.io.File(ctx.cacheDir, fileName)
                val outputStream = java.io.FileOutputStream(file)
                inputStream.use { source ->
                    outputStream.use { sink ->
                        source.copyTo(sink)
                    }
                }
                return file.absolutePath
            } catch (e: Exception) {
                e.printStackTrace()
                return null
            }
        }

        private fun inferUriExtension(ctx: Context, uri: android.net.Uri): String? {
            val displayName = ctx.contentResolver.query(
                uri,
                arrayOf(OpenableColumns.DISPLAY_NAME),
                null,
                null,
                null,
            )?.use { cursor ->
                if (!cursor.moveToFirst()) {
                    return@use null
                }
                val columnIndex = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (columnIndex < 0) {
                    return@use null
                }
                cursor.getString(columnIndex)
            }

            val displayExtension = displayName
                ?.substringAfterLast('.', "")
                ?.lowercase()
                ?.takeIf { it.isNotEmpty() }
            if (displayExtension != null) {
                return displayExtension
            }

            val mimeType = ctx.contentResolver.getType(uri) ?: return null
            return MimeTypeMap.getSingleton()
                .getExtensionFromMimeType(mimeType)
                ?.lowercase()
                ?.takeIf { it.isNotEmpty() }
        }
    }
}

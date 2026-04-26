package waterkit.dialog

import android.app.AlertDialog
import android.app.Activity
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
 *
 * Note: Photo picker requires the host app to forward activity results.
 * Use [preparePhotoPick] and [handleActivityResult] for photo selection.
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

        /** Request code for photo picker - app must use this when calling startActivityForResult. */
        const val REQUEST_CODE_PHOTO_PICK = 9876
        /** Request code for generic file picker. */
        const val REQUEST_CODE_FILE_PICK = 9877
        /** Request code for generic multiple-file picker. */
        const val REQUEST_CODE_FILE_PICK_MULTIPLE = 9878

        private enum class PickerKind {
            Photo,
            File,
            FileMultiple,
        }

        private data class PendingPicker(
            val requestId: Long,
            val kind: PickerKind,
        )

        private val pendingPickers: MutableMap<Int, PendingPicker> = mutableMapOf()

        @JvmStatic
        external fun onPhotoPickerResult(requestId: Long, uri: String?)

        @JvmStatic
        external fun onFilePickerResult(requestId: Long, uri: String?)

        @JvmStatic
        external fun onFilePickerMultipleResult(requestId: Long, uris: String?)

        private fun launchPicker(
            context: Context,
            intent: Intent,
            requestCode: Int,
            requestId: Long,
            kind: PickerKind,
        ) {
            val activity = context as? Activity
                ?: throw IllegalStateException("Context must be an Activity for picker APIs")

            synchronized(DialogHelper::class.java) {
                check(pendingPickers[requestCode] == null) {
                    "Another picker request is already pending for requestCode=$requestCode"
                }
                pendingPickers[requestCode] = PendingPicker(requestId = requestId, kind = kind)
            }

            Handler(Looper.getMainLooper()).post {
                try {
                    activity.startActivityForResult(intent, requestCode)
                } catch (e: Exception) {
                    val pending = synchronized(DialogHelper::class.java) {
                        pendingPickers.remove(requestCode)
                    } ?: return@post

                    when (pending.kind) {
                        PickerKind.Photo -> onPhotoPickerResult(pending.requestId, null)
                        PickerKind.File -> onFilePickerResult(pending.requestId, null)
                        PickerKind.FileMultiple -> onFilePickerMultipleResult(pending.requestId, null)
                    }
                }
            }
        }

        /**
         * Prepare a photo pick intent. The calling Activity must:
         * 1. Call startActivityForResult with the returned intent and REQUEST_CODE_PHOTO_PICK
         * 2. In onActivityResult, call [handleActivityResult]
         *
         * @param type 0 for images, 1 for videos
         * @return Intent to start for photo picking
         */
        @JvmStatic
        fun preparePhotoPick(type: Int): Intent {
            val intent = Intent(Intent.ACTION_GET_CONTENT)
            intent.addCategory(Intent.CATEGORY_OPENABLE)
            if (type == 1) {
                intent.type = "video/*"
            } else {
                intent.type = "image/*"
            }
            return intent
        }

        /**
         * Handle activity result for photo picker.
         *
         * @param requestCode The request code from onActivityResult
         * @param resultCode The result code from onActivityResult
         * @param data The intent data from onActivityResult
         * @return The selected URI as string, or null if cancelled/failed
         */
        @JvmStatic
        fun handleActivityResult(requestCode: Int, resultCode: Int, data: Intent?): String? {
            val pending = synchronized(DialogHelper::class.java) {
                pendingPickers.remove(requestCode)
            } ?: return null

            if (requestCode != REQUEST_CODE_PHOTO_PICK && requestCode != REQUEST_CODE_FILE_PICK) {
                return null
            }
            val uri = if (resultCode == Activity.RESULT_OK && data != null) {
                when (pending.kind) {
                    PickerKind.FileMultiple -> {
                        val clipData = data.clipData
                        if (clipData != null && clipData.itemCount > 0) {
                            buildString {
                                for (index in 0 until clipData.itemCount) {
                                    if (index > 0) {
                                        append('\u0000')
                                    }
                                    append(clipData.getItemAt(index).uri.toString())
                                }
                            }
                        } else {
                            data.data?.toString()
                        }
                    }
                    else -> data.data?.toString()
                }
            } else {
                null
            }

            when (pending.kind) {
                PickerKind.Photo -> onPhotoPickerResult(pending.requestId, uri)
                PickerKind.File -> onFilePickerResult(pending.requestId, uri)
                PickerKind.FileMultiple -> onFilePickerMultipleResult(pending.requestId, uri)
            }
            return uri
        }

        /**
         * Pick a photo asynchronously.
         *
         * The host app must forward `onActivityResult` to [handleActivityResult].
         */
        @JvmStatic
        fun pickPhoto(context: Context, type: Int, requestId: Long) {
            val intent = preparePhotoPick(type)
            launchPicker(context, intent, REQUEST_CODE_PHOTO_PICK, requestId, PickerKind.Photo)
        }

        @JvmStatic
        fun pickFile(context: Context, extensionsCsv: String, requestId: Long) {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }

            val mimeTypes = extensionsCsv
                .split(',')
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

            launchPicker(context, intent, REQUEST_CODE_FILE_PICK, requestId, PickerKind.File)
        }

        @JvmStatic
        fun pickMultipleFiles(context: Context, extensionsCsv: String, requestId: Long) {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
            }

            val mimeTypes = extensionsCsv
                .split(',')
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

            launchPicker(
                context,
                intent,
                REQUEST_CODE_FILE_PICK_MULTIPLE,
                requestId,
                PickerKind.FileMultiple,
            )
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

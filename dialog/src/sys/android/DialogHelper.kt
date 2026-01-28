package waterkit.dialog

import android.app.AlertDialog
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper
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

        /** Pending callback for photo picker result. */
        private var pendingPhotoCallback: ((String?) -> Unit)? = null

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
            if (requestCode != REQUEST_CODE_PHOTO_PICK) {
                return null
            }
            if (resultCode == Activity.RESULT_OK && data != null) {
                return data.data?.toString()
            }
            return null
        }

        /**
         * Pick a photo synchronously. This is a legacy API that only works if the context
         * is an Activity that has been set up to forward activity results properly.
         *
         * For most use cases, prefer [preparePhotoPick] + [handleActivityResult].
         *
         * @return Selected URI as string, or null if not supported/cancelled
         */
        @JvmStatic
        fun pickPhoto(context: Context, type: Int): String? {
            // Without AndroidX Fragment support, we can't easily do synchronous photo picking.
            // Apps should use preparePhotoPick/handleActivityResult flow instead.
            // Return null to indicate this path is not supported.
            println("WaterKit: pickPhoto requires app-level integration. Use preparePhotoPick/handleActivityResult.")
            return null
        }

        @JvmStatic
        fun loadMedia(context: Context, uriString: String): String? {
            val uri = android.net.Uri.parse(uriString)
            return copyUriToCache(context, uri)
        }

        private fun copyUriToCache(ctx: Context, uri: android.net.Uri): String? {
            try {
                val inputStream = ctx.contentResolver.openInputStream(uri) ?: return null
                val fileName = "picked_media_" + System.currentTimeMillis()
                val file = java.io.File(ctx.cacheDir, fileName)
                val outputStream = java.io.FileOutputStream(file)
                inputStream.copyTo(outputStream)
                inputStream.close()
                outputStream.close()
                return file.absolutePath
            } catch (e: Exception) {
                e.printStackTrace()
                return null
            }
        }
    }
}

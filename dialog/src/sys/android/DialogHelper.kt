package waterkit.dialog

import android.app.AlertDialog
import android.content.Context
import android.os.Handler
import android.os.Looper
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicBoolean

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
        fun pickPhoto(context: Context, type: Int): String? {
            if (Looper.myLooper() == Looper.getMainLooper()) {
                return null
            }

            val latch = CountDownLatch(1)
            // Use an array to hold the result string (or null)
            val resultWrapper = arrayOfNulls<String>(1)

            Handler(Looper.getMainLooper()).post {
                try {
                    // We need to launch an ephemeral Activity to handle the result.
                    // Or if context is FragmentActivity, attach a fragment.
                    // For now, let's try to use a classic Intent.ACTION_GET_CONTENT with a localized listener if possible? No.
                    // We need a class to receive the result.
                    // Since we cannot easily add a new Activity class to the manifest dynamically, 
                    // we might need to rely on the app having a specific setup or use a Headless Fragment if context is FragmentActivity.
                    
                    if (context is androidx.fragment.app.FragmentActivity) {
                        val fragmentManager = context.supportFragmentManager
                        val tag = "waterkit_photo_picker"
                        var fragment = fragmentManager.findFragmentByTag(tag) as? PhotoPickerFragment
                        if (fragment == null) {
                            fragment = PhotoPickerFragment()
                            fragmentManager.beginTransaction().add(fragment, tag).commitNowAllowingStateLoss()
                        }
                        
                        fragment.pick(type) { path ->
                            resultWrapper[0] = path
                            latch.countDown()
                        }
                    } else {
                        // Fallback or Error: Context is not FragmentActivity
                        println("WaterKit: Context is not FragmentActivity, cannot use Photo Picker")
                        latch.countDown()
                    }
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
            return resultWrapper[0]
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

    // Inner Fragment to handle ActivityResult
    class PhotoPickerFragment : androidx.fragment.app.Fragment() {
        private var callback: ((String?) -> Unit)? = null

        // Using standard Intent instead of ActivityResultContracts for broader compatibility without complex dependency setup in this snippet if possible,
        // but ActivityResultContracts is much cleaner. We'll use the classic onActivityResult for max compatibility if we don't assume androidx.activity:1.2+.
        // Actually, let's use the new API if possible, but fallback to classic for simplicity in a single file without R8/ProGuard issues on missing classes?
        // Let's stick to simple Intent.ACTION_GET_CONTENT.

        private val REQUEST_CODE_PICK = 9876

        fun pick(type: Int, cb: (String?) -> Unit) {
            this.callback = cb
            val intent = android.content.Intent(android.content.Intent.ACTION_GET_CONTENT)
            intent.addCategory(android.content.Intent.CATEGORY_OPENABLE)
            if (type == 1) {
                intent.type = "video/*"
            } else {
                intent.type = "image/*"
            }
            startActivityForResult(intent, REQUEST_CODE_PICK)
        }

        override fun onActivityResult(requestCode: Int, resultCode: Int, data: android.content.Intent?) {
            super.onActivityResult(requestCode, resultCode, data)
            if (requestCode == REQUEST_CODE_PICK) {
                if (resultCode == android.app.Activity.RESULT_OK && data != null) {
                    val uri = data.data
                    if (uri != null) {
                        // Return the URI directly (Opaque Handle)
                        callback?.invoke(uri.toString())
                    } else {
                        callback?.invoke(null)
                    }
                } else {
                    callback?.invoke(null)
                }
                // Cleanup
                parentFragmentManager.beginTransaction().remove(this).commitAllowingStateLoss()
            }
        }
    }
}

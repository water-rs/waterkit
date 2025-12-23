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
    }
}

package waterkit.clipboard

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import java.io.ByteArrayOutputStream

class ClipboardHelper {
    companion object {
        @JvmStatic
        fun getText(context: Context): String? {
            val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            val clip = clipboard?.primaryClip
            if (clip != null && clip.itemCount > 0) {
                return clip.getItemAt(0).text?.toString()
            }
            return null
        }

        @JvmStatic
        fun setText(context: Context, text: String) {
            val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            val clip = ClipData.newPlainText("text", text)
            clipboard?.setPrimaryClip(clip)
        }
        
        @JvmStatic
        fun hasImage(context: Context): Boolean {
             val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
             val description = clipboard?.primaryClipDescription
             if (description != null) {
                 return description.hasMimeType("image/*")
             }
             return false
        }

        @JvmStatic
        fun getImage(context: Context): ByteArray? {
            val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            val clip = clipboard?.primaryClip
            if (clip != null && clip.itemCount > 0) {
                val item = clip.getItemAt(0)
                val uri = item.uri
                if (uri != null) {
                    try {
                        val inputStream = context.contentResolver.openInputStream(uri)
                        val buffer = ByteArrayOutputStream()
                        val data = ByteArray(1024)
                        var nRead: Int
                        while (inputStream!!.read(data, 0, data.size).also { nRead = it } != -1) {
                            buffer.write(data, 0, nRead)
                        }
                        return buffer.toByteArray()
                    } catch (e: Exception) {
                        e.printStackTrace()
                    }
                }
            }
            return null
        }

        // setImage is complex without FileProvider, skipping for now or implementing later.
    }
}

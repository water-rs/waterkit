package waterkit.share

import android.content.Context
import android.content.Intent
import android.net.Uri

object ShareHelper {
    @JvmStatic
    fun shareText(context: Context, text: String, subject: String?) {
        val intent = Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_TEXT, text)
            subject?.let { putExtra(Intent.EXTRA_SUBJECT, it) }
        }
        context.startActivity(Intent.createChooser(intent, "Share"))
    }

    @JvmStatic
    fun shareFile(context: Context, filePath: String, mimeType: String) {
        val file = java.io.File(filePath)
        val uri = Uri.fromFile(file)
        val intent = Intent(Intent.ACTION_SEND).apply {
            type = mimeType
            putExtra(Intent.EXTRA_STREAM, uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        context.startActivity(Intent.createChooser(intent, "Share"))
    }
}

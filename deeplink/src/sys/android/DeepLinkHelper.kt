package waterkit.deeplink

import android.content.Context
import android.content.Intent
import android.net.Uri

object DeepLinkHelper {
    @JvmStatic
    fun openUrl(context: Context, url: String): Boolean {
        return try {
            val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
            context.startActivity(intent)
            true
        } catch (e: Exception) {
            false
        }
    }

    @JvmStatic
    fun canOpenUrl(context: Context, url: String): Boolean {
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
        return intent.resolveActivity(context.packageManager) != null
    }

    @JvmStatic
    fun getIntentUrl(intent: Intent?): String? {
        return intent?.data?.toString()
    }
}

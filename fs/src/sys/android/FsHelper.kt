package waterkit.fs

import android.content.Context
import android.os.Environment
import java.io.File

class FsHelper {
    companion object {
        @JvmStatic
        fun getDocumentsDir(context: Context): String? {
             return context.getExternalFilesDir(Environment.DIRECTORY_DOCUMENTS)?.absolutePath
        }

        @JvmStatic
        fun getCacheDir(context: Context): String? {
            return context.cacheDir.absolutePath
        }
    }
}

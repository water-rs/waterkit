package waterkit.clipboard

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import java.io.File

/**
 * Serves, read-only, the files the clipboard carries.
 *
 * A `file://` URI cannot leave the app on Android 7 and later, so each copied
 * file goes on the clipboard as `content://<authority><absolute path>`, the
 * path encoded by [Uri.Builder.path] and decoded by [Uri.getPath]. The
 * provider is not exported: an app reads a URI only through the read grant
 * the clipboard service gives the app that reads the clip, which needs the
 * declaration to set `android:grantUriPermissions="true"`.
 *
 * The app's manifest declares it, under any authority:
 *
 * ```xml
 * <provider
 *     android:name="waterkit.clipboard.ClipboardFileProvider"
 *     android:authorities="${applicationId}.waterkit.clipboard"
 *     android:exported="false"
 *     android:grantUriPermissions="true" />
 * ```
 */
class ClipboardFileProvider : ContentProvider() {

    override fun onCreate(): Boolean = true

    override fun getType(uri: Uri): String =
        MimeTypeMap.getSingleton().getMimeTypeFromExtension(file(uri).extension.lowercase())
            ?: "application/octet-stream"

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        require(mode == "r") { "$uri is read-only, so it cannot be opened in mode \"$mode\"" }
        return ParcelFileDescriptor.open(file(uri), ParcelFileDescriptor.MODE_READ_ONLY)
    }

    /** The [OpenableColumns] of the file, as every reader of a shared file asks. */
    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): Cursor {
        val file = file(uri)
        val columns = (projection ?: OPENABLE_COLUMNS).filter { it in OPENABLE_COLUMNS }
        val row = columns.map { column ->
            when (column) {
                OpenableColumns.DISPLAY_NAME -> file.name
                else -> file.length()
            }
        }
        return MatrixCursor(columns.toTypedArray(), 1).apply { addRow(row) }
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri =
        throw UnsupportedOperationException("the clipboard's files are read-only")

    override fun update(
        uri: Uri,
        values: ContentValues?,
        selection: String?,
        selectionArgs: Array<out String>?,
    ): Int = throw UnsupportedOperationException("the clipboard's files are read-only")

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int =
        throw UnsupportedOperationException("the clipboard's files are read-only")

    private fun file(uri: Uri): File =
        File(checkNotNull(uri.path) { "$uri names no file" })

    companion object {
        private val OPENABLE_COLUMNS = arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)

        /** The URI that serves the file at the absolute [path]. */
        fun uri(authority: String, path: String): Uri =
            Uri.Builder().scheme("content").authority(authority).path(path).build()
    }
}

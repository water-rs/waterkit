package waterkit.vision

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.graphics.Point
import android.graphics.Rect
import android.media.ExifInterface
import android.media.Image
import com.google.mlkit.vision.common.InputImage
import java.io.ByteArrayInputStream

/**
 * The `InputImage` builders and shared bits ML Kit's barcode and text
 * helpers both use. Compiled by the packager onto the application's
 * classpath whenever either feature is enabled; the crate declares it in
 * both `feature.*` kotlin-sources lists, where it deduplicates by name.
 */
object MlKitInput {
    /** Wraps a camera analysis frame's `android.media.Image`; no pixel copy. */
    @JvmStatic
    fun mediaInput(image: Image, rotationDegrees: Int): InputImage =
        InputImage.fromMediaImage(image, rotationDegrees)

    /** Wraps an already-upright `Bitmap` for ML Kit. */
    @JvmStatic
    fun bitmapInput(bitmap: Bitmap): InputImage = InputImage.fromBitmap(bitmap, 0)

    /** Decodes image bytes to an upright `Bitmap`, honoring EXIF orientation. */
    @JvmStatic
    fun decode(bytes: ByteArray): Bitmap {
        val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
            ?: throw IllegalArgumentException("image bytes do not decode")
        val exif = runCatching {
            ExifInterface(ByteArrayInputStream(bytes)).getAttributeInt(
                ExifInterface.TAG_ORIENTATION,
                ExifInterface.ORIENTATION_NORMAL,
            )
        }.getOrDefault(ExifInterface.ORIENTATION_NORMAL)
        return upright(bitmap, exif)
    }

    /** Builds an upright `Bitmap` from tightly packed RGBA bytes. */
    @JvmStatic
    fun rgbaBitmap(rgba: ByteArray, width: Int, height: Int, exifOrientation: Int): Bitmap {
        require(rgba.size == width * height * 4) {
            "RGBA buffer ${rgba.size} != ${width}x${height}x4"
        }
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        bitmap.copyPixelsFromBuffer(java.nio.ByteBuffer.wrap(rgba))
        return upright(bitmap, exifOrientation)
    }

    /** Flattened corner points, or the bounding box's corners when null. */
    internal fun corners(points: Array<Point>?, box: Rect?): IntArray {
        if (points != null && points.size == 4) {
            return intArrayOf(
                points[0].x, points[0].y,
                points[1].x, points[1].y,
                points[2].x, points[2].y,
                points[3].x, points[3].y,
            )
        }
        if (box != null) {
            return intArrayOf(
                box.left, box.top,
                box.right, box.top,
                box.right, box.bottom,
                box.left, box.bottom,
            )
        }
        return IntArray(0)
    }

    /** Returns `bitmap` transformed to upright when `exif` rotates or mirrors it. */
    private fun upright(bitmap: Bitmap, exif: Int): Bitmap {
        val matrix = Matrix()
        when (exif) {
            ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> matrix.setScale(-1f, 1f)
            ExifInterface.ORIENTATION_ROTATE_180 -> matrix.setRotate(180f)
            ExifInterface.ORIENTATION_FLIP_VERTICAL -> matrix.setScale(1f, -1f)
            ExifInterface.ORIENTATION_TRANSPOSE -> {
                matrix.setScale(-1f, 1f)
                matrix.postRotate(270f)
            }
            ExifInterface.ORIENTATION_ROTATE_90 -> matrix.setRotate(90f)
            ExifInterface.ORIENTATION_TRANSVERSE -> {
                matrix.setScale(-1f, 1f)
                matrix.postRotate(90f)
            }
            ExifInterface.ORIENTATION_ROTATE_270 -> matrix.setRotate(270f)
            else -> return bitmap
        }
        return Bitmap.createBitmap(bitmap, 0, 0, bitmap.width, bitmap.height, matrix, false)
    }
}

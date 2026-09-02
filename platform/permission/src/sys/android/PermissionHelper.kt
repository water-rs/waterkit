package waterkit.permission

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager

/**
 * Helper class for checking and requesting permissions on Android.
 * Compiled to DEX and embedded in the Rust library.
 */
object PermissionHelper {
    
    // Permission type constants (must match Rust enum)
    const val PERMISSION_LOCATION = 0
    const val PERMISSION_CAMERA = 1
    const val PERMISSION_MICROPHONE = 2
    const val PERMISSION_PHOTOS = 3
    const val PERMISSION_CONTACTS = 4
    const val PERMISSION_CALENDAR = 5

    // Status constants (must match Rust enum)
    const val STATUS_NOT_DETERMINED = 0
    const val STATUS_RESTRICTED = 1
    const val STATUS_DENIED = 2
    const val STATUS_GRANTED = 3

    /**
     * Check if a permission is granted.
     */
    @JvmStatic
    fun checkPermission(activity: Activity, permissionType: Int): Int {
        val permission = getPermissionString(permissionType) ?: return STATUS_NOT_DETERMINED

        return if (activity.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED) {
            STATUS_GRANTED
        } else {
            STATUS_DENIED
        }
    }

    /**
     * Request a permission from the user.
     */
    @JvmStatic
    fun requestPermission(activity: Activity, permissionType: Int, requestCode: Int) {
        val permission = getPermissionString(permissionType) ?: return
        activity.requestPermissions(arrayOf(permission), requestCode)
    }

    private fun getPermissionString(permissionType: Int): String? = when (permissionType) {
        PERMISSION_LOCATION -> Manifest.permission.ACCESS_FINE_LOCATION
        PERMISSION_CAMERA -> Manifest.permission.CAMERA
        PERMISSION_MICROPHONE -> Manifest.permission.RECORD_AUDIO
        PERMISSION_PHOTOS -> Manifest.permission.READ_MEDIA_IMAGES
        PERMISSION_CONTACTS -> Manifest.permission.READ_CONTACTS
        PERMISSION_CALENDAR -> Manifest.permission.READ_CALENDAR
        else -> null
    }
}

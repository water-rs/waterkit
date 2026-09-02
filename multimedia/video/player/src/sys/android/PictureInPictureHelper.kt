package waterkit.video

import android.app.Activity
import android.app.Application
import android.app.PictureInPictureParams
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.util.Rational

/** Platform bridge owned by the UI-independent WaterKit video player. */
object PictureInPictureHelper : Application.ActivityLifecycleCallbacks {
    const val RESULT_ENTERED = 0
    const val RESULT_PLATFORM_UNSUPPORTED = 1
    const val RESULT_DEVICE_UNSUPPORTED = 2
    const val RESULT_ACTIVITY_UNAVAILABLE = 3
    const val RESULT_ACTIVITY_NOT_DECLARED = 4
    const val RESULT_ENTER_FAILED = 5

    @Volatile
    private var initialized = false

    @Volatile
    private var currentActivity: Activity? = null

    @Volatile
    private var controllerState = ControllerState(
        active = false,
        playing = false,
        aspectWidth = 0,
        aspectHeight = 0,
    )

    @JvmStatic
    fun enterPictureInPicture(
        context: Context,
        aspectWidth: Int,
        aspectHeight: Int
    ): Int {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return RESULT_PLATFORM_UNSUPPORTED
        }

        ensureInitialized(context)

        val activity = currentActivity ?: (context as? Activity)
            ?: return RESULT_ACTIVITY_UNAVAILABLE

        return enterPictureInPicture(activity, aspectWidth, aspectHeight)
    }

    @JvmStatic
    fun updateControllerState(
        context: Context,
        active: Boolean,
        playing: Boolean,
        aspectWidth: Int,
        aspectHeight: Int
    ) {
        ensureInitialized(context)
        controllerState = ControllerState(
            active = active,
            playing = playing,
            aspectWidth = aspectWidth.coerceAtLeast(0),
            aspectHeight = aspectHeight.coerceAtLeast(0),
        )
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val activity = currentActivity ?: (context as? Activity) ?: return
            activity.setPictureInPictureParams(
                pictureInPictureParams(
                    aspectWidth = controllerState.aspectWidth,
                    aspectHeight = controllerState.aspectHeight,
                    autoEnterEnabled = active && playing,
                )
            )
        }
    }

    @JvmStatic
    fun isPictureInPictureActive(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return false
        }

        ensureInitialized(context)

        val activity = currentActivity ?: (context as? Activity) ?: return false
        return activity.isInPictureInPictureMode
    }

    private fun enterPictureInPicture(
        activity: Activity,
        aspectWidth: Int,
        aspectHeight: Int
    ): Int {
        if (!activity.packageManager.hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)) {
            return RESULT_DEVICE_UNSUPPORTED
        }

        val params = pictureInPictureParams(aspectWidth, aspectHeight, false)

        return try {
            if (activity.enterPictureInPictureMode(params)) {
                RESULT_ENTERED
            } else {
                RESULT_ENTER_FAILED
            }
        } catch (_: IllegalStateException) {
            RESULT_ACTIVITY_NOT_DECLARED
        }
    }

    private fun pictureInPictureParams(
        aspectWidth: Int,
        aspectHeight: Int,
        autoEnterEnabled: Boolean,
    ): PictureInPictureParams = PictureInPictureParams.Builder().apply {
        if (aspectWidth > 0 && aspectHeight > 0) {
            setAspectRatio(Rational(aspectWidth, aspectHeight))
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            setAutoEnterEnabled(autoEnterEnabled)
        }
    }.build()

    @Synchronized
    private fun ensureInitialized(context: Context) {
        if (initialized) {
            return
        }

        val application = context.applicationContext as? Application
            ?: error("PictureInPictureHelper requires an Application context")
        application.registerActivityLifecycleCallbacks(this)
        initialized = true
    }

    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {
        currentActivity = activity
    }

    override fun onActivityStarted(activity: Activity) {
        currentActivity = activity
    }

    override fun onActivityResumed(activity: Activity) {
        currentActivity = activity
    }

    override fun onActivityPaused(activity: Activity) = Unit

    override fun onActivityStopped(activity: Activity) = Unit

    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit

    override fun onActivityDestroyed(activity: Activity) {
        if (currentActivity === activity) {
            currentActivity = null
        }
    }

    private data class ControllerState(
        val active: Boolean,
        val playing: Boolean,
        val aspectWidth: Int,
        val aspectHeight: Int,
    )
}

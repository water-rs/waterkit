package waterkit.video

import android.app.Activity
import android.app.Application
import android.app.PictureInPictureParams
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.util.Rational

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
    }

    @JvmStatic
    fun onUserLeaveHint(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return
        }

        ensureInitialized(context)
        val activity = currentActivity ?: (context as? Activity) ?: return
        maybeEnterAutomaticPictureInPicture(activity)
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

    private fun maybeEnterAutomaticPictureInPicture(activity: Activity) {
        val state = controllerState
        if (!state.active || !state.playing) {
            return
        }

        if (activity.isChangingConfigurations || activity.isFinishing || activity.isDestroyed) {
            return
        }

        if (activity.isInPictureInPictureMode) {
            return
        }

        enterPictureInPicture(activity, state.aspectWidth, state.aspectHeight)
    }

    private fun enterPictureInPicture(
        activity: Activity,
        aspectWidth: Int,
        aspectHeight: Int
    ): Int {
        if (!activity.packageManager.hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)) {
            return RESULT_DEVICE_UNSUPPORTED
        }

        val params = PictureInPictureParams.Builder().apply {
            if (aspectWidth > 0 && aspectHeight > 0) {
                setAspectRatio(Rational(aspectWidth, aspectHeight))
            }
        }.build()

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

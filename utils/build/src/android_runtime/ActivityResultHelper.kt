package waterkit.build

import android.content.Context
import android.content.Intent
import android.content.IntentSender
import android.os.Handler
import android.os.Looper
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResult
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContract
import androidx.activity.result.contract.ActivityResultContracts
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver

object ActivityResultHelper {
    @JvmStatic
    fun startActivityForResult(
        context: Context,
        requestId: Long,
        intent: Intent,
    ): Boolean = launch(
        context,
        requestId,
        ActivityResultContracts.StartActivityForResult(),
    ) { intent }

    @JvmStatic
    fun startIntentSenderForResult(
        context: Context,
        requestId: Long,
        intentSender: IntentSender,
    ): Boolean = launch(
        context,
        requestId,
        ActivityResultContracts.StartIntentSenderForResult(),
    ) {
        IntentSenderRequest.Builder(intentSender).build()
    }

    private fun <I> launch(
        context: Context,
        requestId: Long,
        contract: ActivityResultContract<I, ActivityResult>,
        input: () -> I,
    ): Boolean {
        val activity = context as? ComponentActivity ?: return false
        Handler(Looper.getMainLooper()).post {
            if (activity.lifecycle.currentState == Lifecycle.State.DESTROYED) {
                deliverDestroyed(requestId)
                return@post
            }

            val request = PendingRequest<I>(activity, requestId)
            try {
                request.register(contract)
                request.observe()
                request.launch(input())
            } catch (error: Exception) {
                request.settle {
                    deliverLaunchFailure(requestId, error.toString())
                }
            }
        }
        return true
    }

    @JvmStatic
    external fun deliverResult(requestId: Long, resultCode: Int, data: Intent?)

    @JvmStatic
    external fun deliverDestroyed(requestId: Long)

    @JvmStatic
    external fun deliverLaunchFailure(requestId: Long, description: String)

    private class PendingRequest<I>(
        private val activity: ComponentActivity,
        private val requestId: Long,
    ) {
        private val key = "waterkit.activity-result.$requestId"
        private var launcher: ActivityResultLauncher<I>? = null
        private var observer: LifecycleEventObserver? = null
        private var settled = false

        fun register(contract: ActivityResultContract<I, ActivityResult>) {
            launcher = activity.activityResultRegistry.register(key, contract) { result ->
                settle {
                    deliverResult(requestId, result.resultCode, result.data)
                }
            }
        }

        fun observe() {
            val lifecycleObserver = LifecycleEventObserver { _, event ->
                if (event == Lifecycle.Event.ON_DESTROY) {
                    settle {
                        deliverDestroyed(requestId)
                    }
                }
            }
            observer = lifecycleObserver
            activity.lifecycle.addObserver(lifecycleObserver)
        }

        fun launch(input: I) {
            if (settled) {
                return
            }
            launcher?.launch(input) ?: error("activity result launcher was not registered")
        }

        fun settle(deliver: () -> Unit) {
            if (settled) {
                return
            }
            settled = true
            launcher?.unregister()
            observer?.let(activity.lifecycle::removeObserver)
            deliver()
        }
    }
}

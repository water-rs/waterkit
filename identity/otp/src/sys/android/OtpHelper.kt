package waterkit.otp

import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.Parcelable
import android.provider.Telephony
import android.telephony.SmsManager
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import com.google.android.gms.auth.api.phone.SmsRetriever
import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.api.CommonStatusCodes
import com.google.android.gms.common.api.Status
import waterkit.build.NativeCallback
import waterkit.build.NativeChannel

/** Events an OTP request streams into Rust through its [NativeChannel]. */
sealed interface OtpEvent {
    object Started : OtpEvent
    class Message(val text: String) : OtpEvent
    object Timeout : OtpEvent
    object Denied : OtpEvent
    class Failed(val error: String) : OtpEvent
}

/**
 * One in-flight OTP request. Owns its broadcast receiver / consent launcher and
 * the [NativeChannel] its events stream through; the Rust request handle keeps
 * a global reference and cancels through [cancel].
 */
class OtpRequest(
    private val context: Context,
    private val channel: NativeChannel,
) {
    private var receiver: BroadcastReceiver? = null
    private var launcher: ActivityResultLauncher<Intent>? = null
    private var registered = false
    private var cancelled = false

    fun startSmsRetriever() {
        OtpHelper.runOnMain {
            if (cancelled) return@runOnMain
            try {
                registerRetrieverReceiver(consent = false)
                SmsRetriever.getClient(context).startSmsRetriever()
                    .addOnSuccessListener { channel.send(OtpEvent.Started) }
                    .addOnFailureListener { error ->
                        finish(OtpEvent.Failed(error.message ?: "SMS Retriever failed"))
                    }
            } catch (error: Exception) {
                finish(OtpEvent.Failed(error.message ?: "SMS Retriever setup failed"))
            }
        }
    }

    @Suppress("DEPRECATION")
    fun createAppSpecificSmsToken(callback: NativeCallback) {
        OtpHelper.runOnMain {
            try {
                callback.complete(createAppSpecificSmsTokenOnMain())
            } catch (error: Exception) {
                callback.fail(error.message ?: "SMS token request failed")
            }
        }
    }

    @Suppress("DEPRECATION")
    private fun createAppSpecificSmsTokenOnMain(): String {
        check(!cancelled) { "SMS token request was cancelled" }
        check(!registered) { "OTP request already has a listener" }
        val action =
            "${context.packageName}${OtpHelper.TOKEN_ACTION_SUFFIX}${System.identityHashCode(this)}"
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(receiverContext: Context, intent: Intent) {
                val message = Telephony.Sms.Intents.getMessagesFromIntent(intent)
                    .joinToString(separator = "") { it.messageBody }
                finish(OtpEvent.Message(message))
            }
        }
        registered = true
        this.receiver = receiver
        try {
            OtpHelper.registerPrivateReceiver(context, receiver, IntentFilter(action))
        } catch (error: Exception) {
            registered = false
            this.receiver = null
            throw error
        }

        val pendingIntentFlags =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                PendingIntent.FLAG_ONE_SHOT or PendingIntent.FLAG_MUTABLE
            } else {
                PendingIntent.FLAG_ONE_SHOT
            }
        val pendingIntent = PendingIntent.getBroadcast(
            context,
            System.identityHashCode(this),
            Intent(action).setPackage(context.packageName),
            pendingIntentFlags,
        )
        val smsManager =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                context.getSystemService(SmsManager::class.java)
                    ?: throw IllegalStateException("SmsManager is unavailable")
            } else {
                SmsManager.getDefault()
            }
        return smsManager.createAppSpecificSmsToken(pendingIntent)
            ?: throw IllegalStateException("SmsManager returned no app-specific token")
    }

    fun startSmsUserConsent(sender: String?) {
        OtpHelper.runOnMain {
            if (cancelled) return@runOnMain
            try {
                registerRetrieverReceiver(consent = true)
                SmsRetriever.getClient(context).startSmsUserConsent(sender)
                    .addOnSuccessListener { channel.send(OtpEvent.Started) }
                    .addOnFailureListener { error ->
                        finish(OtpEvent.Failed(error.message ?: "SMS User Consent failed"))
                    }
            } catch (error: Exception) {
                finish(OtpEvent.Failed(error.message ?: "SMS User Consent setup failed"))
            }
        }
    }

    /**
     * Cancels the request: unregisters its receiver / consent launcher and ends
     * the event stream. Runs on the main looper; called again is a no-op.
     */
    fun cancel() {
        OtpHelper.runOnMain {
            cancelled = true
            if (unregister()) {
                channel.close()
            }
        }
    }

    private fun registerRetrieverReceiver(consent: Boolean) {
        check(!registered) { "OTP request already has a listener" }
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(receiverContext: Context, intent: Intent) {
                val status = intent.parcelableExtra<Status>(SmsRetriever.EXTRA_STATUS)
                when (status?.statusCode) {
                    CommonStatusCodes.SUCCESS -> {
                        if (consent) {
                            val consentIntent =
                                intent.parcelableExtra<Intent>(SmsRetriever.EXTRA_CONSENT_INTENT)
                            if (consentIntent == null || !OtpHelper.validConsentIntent(context, consentIntent)) {
                                finish(
                                    OtpEvent.Failed("SMS User Consent returned an untrusted intent"),
                                )
                            } else {
                                launchConsent(consentIntent)
                            }
                        } else {
                            val message = intent.getStringExtra(SmsRetriever.EXTRA_SMS_MESSAGE)
                            if (message == null) {
                                finish(
                                    OtpEvent.Failed("SMS Retriever returned no message"),
                                )
                            } else {
                                finish(OtpEvent.Message(message))
                            }
                        }
                    }

                    CommonStatusCodes.TIMEOUT -> finish(OtpEvent.Timeout)
                    else -> finish(
                        OtpEvent.Failed("SMS API returned unknown status ${status?.statusCode}"),
                    )
                }
            }
        }
        registered = true
        this.receiver = receiver
        try {
            OtpHelper.registerPlayServicesReceiver(context, receiver)
        } catch (error: Exception) {
            registered = false
            this.receiver = null
            throw error
        }
    }

    private fun launchConsent(consentIntent: Intent) {
        val activity = context as? ComponentActivity
        if (activity == null) {
            finish(
                OtpEvent.Failed(
                    "SMS User Consent requires the Android context to be a ComponentActivity",
                ),
            )
            return
        }

        try {
            val launcher = activity.activityResultRegistry.register(
                "${OtpHelper.CONSENT_KEY_PREFIX}${System.identityHashCode(this)}",
                ActivityResultContracts.StartActivityForResult(),
            ) { result ->
                if (result.resultCode == Activity.RESULT_OK) {
                    val message =
                        result.data?.getStringExtra(SmsRetriever.EXTRA_SMS_MESSAGE)
                    if (message == null) {
                        finish(
                            OtpEvent.Failed("SMS User Consent returned no message"),
                        )
                    } else {
                        finish(OtpEvent.Message(message))
                    }
                } else {
                    finish(OtpEvent.Denied)
                }
            }
            this.launcher = launcher
            launcher.launch(consentIntent)
        } catch (error: Exception) {
            finish(OtpEvent.Failed(error.message ?: "SMS consent prompt failed"))
        }
    }

    /** Terminal event: unregisters the request, then sends it and closes the channel. */
    private fun finish(event: OtpEvent) {
        OtpHelper.runOnMain {
            if (unregister()) {
                channel.send(event)
                channel.close()
            }
        }
    }

    private fun unregister(): Boolean {
        if (!registered) return false
        registered = false
        receiver?.let { context.unregisterReceiver(it) }
        receiver = null
        launcher?.unregister()
        launcher = null
        return true
    }

    @Suppress("DEPRECATION")
    private inline fun <reified T : Parcelable> Intent.parcelableExtra(name: String): T? {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            getParcelableExtra(name, T::class.java)
        } else {
            getParcelableExtra(name) as? T
        }
    }
}

/** Shared main-looper helpers and the static entry points of the OTP backend. */
object OtpHelper {
    private const val PLAY_SERVICES = 1 shl 0
    private const val TELEPHONY_MESSAGING = 1 shl 1
    const val TOKEN_ACTION_SUFFIX = ".waterkit.otp.SMS_TOKEN."
    const val CONSENT_KEY_PREFIX = "waterkit.otp.consent."

    internal const val URI_PERMISSION_FLAGS =
        Intent.FLAG_GRANT_READ_URI_PERMISSION or
            Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
            Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION or
            Intent.FLAG_GRANT_PREFIX_URI_PERMISSION

    private val mainHandler = Handler(Looper.getMainLooper())

    @JvmStatic
    fun capabilities(context: Context): Int {
        var result = 0
        if (com.google.android.gms.common.GoogleApiAvailability.getInstance()
                .isGooglePlayServicesAvailable(context) == ConnectionResult.SUCCESS
        ) {
            result = result or PLAY_SERVICES
        }
        val messagingFeature = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            PackageManager.FEATURE_TELEPHONY_MESSAGING
        } else {
            PackageManager.FEATURE_TELEPHONY
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
            context.packageManager.hasSystemFeature(messagingFeature)
        ) {
            result = result or TELEPHONY_MESSAGING
        }
        return result
    }

    @JvmStatic
    @Suppress("DEPRECATION")
    fun signingCertificate(context: Context): ByteArray {
        val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            PackageManager.GET_SIGNING_CERTIFICATES
        } else {
            PackageManager.GET_SIGNATURES
        }
        val packageInfo =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                context.packageManager.getPackageInfo(
                    context.packageName,
                    PackageManager.PackageInfoFlags.of(flags.toLong()),
                )
            } else {
                context.packageManager.getPackageInfo(context.packageName, flags)
            }
        val signers =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                packageInfo.signingInfo?.apkContentsSigners
            } else {
                packageInfo.signatures
            } ?: throw IllegalStateException("application signing certificates are missing")
        check(signers.size == 1) {
            "SMS Retriever requires exactly one current application signer"
        }
        return signers.single().toByteArray()
    }

    internal fun validConsentIntent(context: Context, intent: Intent): Boolean {
        val packageName = intent.resolveActivity(context.packageManager)?.packageName
        return packageName == "com.google.android.gms" &&
            intent.flags and URI_PERMISSION_FLAGS == 0
    }

    internal fun registerPlayServicesReceiver(context: Context, receiver: BroadcastReceiver) {
        val filter = IntentFilter(SmsRetriever.SMS_RETRIEVED_ACTION)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(
                receiver,
                filter,
                SmsRetriever.SEND_PERMISSION,
                mainHandler,
                Context.RECEIVER_EXPORTED,
            )
        } else {
            context.registerReceiver(
                receiver,
                filter,
                SmsRetriever.SEND_PERMISSION,
                mainHandler,
            )
        }
    }

    internal fun registerPrivateReceiver(
        context: Context,
        receiver: BroadcastReceiver,
        filter: IntentFilter,
    ) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            context.registerReceiver(receiver, filter)
        }
    }

    internal inline fun runOnMain(crossinline block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            block()
        } else {
            mainHandler.post { block() }
        }
    }

}

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
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutionException
import java.util.concurrent.FutureTask
import waterkit.build.NativeChannel

/** Events an OTP request streams into Rust through its [NativeChannel]. */
sealed interface OtpEvent {
    object Started : OtpEvent
    class Message(val text: String) : OtpEvent
    object Timeout : OtpEvent
    object Denied : OtpEvent
    class Failed(val error: String) : OtpEvent
}

object OtpHelper {
    private const val PLAY_SERVICES = 1 shl 0
    private const val TELEPHONY_MESSAGING = 1 shl 1
    private const val TOKEN_ACTION_SUFFIX = ".waterkit.otp.SMS_TOKEN."
    private const val CONSENT_KEY_PREFIX = "waterkit.otp.consent."

    private const val URI_PERMISSION_FLAGS =
        Intent.FLAG_GRANT_READ_URI_PERMISSION or
            Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
            Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION or
            Intent.FLAG_GRANT_PREFIX_URI_PERMISSION

    private data class Registration(
        val context: Context,
        var receiver: BroadcastReceiver? = null,
        var launcher: ActivityResultLauncher<Intent>? = null,
    )

    private val registrations = ConcurrentHashMap<NativeChannel, Registration>()
    private val cancelled = ConcurrentHashMap.newKeySet<NativeChannel>()
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

    @JvmStatic
    fun startSmsRetriever(context: Context, channel: NativeChannel) {
        runOnMain {
            if (cancelled.remove(channel)) return@runOnMain
            try {
                registerRetrieverReceiver(context, channel, consent = false)
                SmsRetriever.getClient(context).startSmsRetriever()
                    .addOnSuccessListener { channel.send(OtpEvent.Started) }
                    .addOnFailureListener { error ->
                        finish(channel, OtpEvent.Failed(error.message ?: "SMS Retriever failed"))
                    }
            } catch (error: Exception) {
                finish(channel, OtpEvent.Failed(error.message ?: "SMS Retriever setup failed"))
            }
        }
    }

    @JvmStatic
    @Suppress("DEPRECATION")
    fun createAppSpecificSmsToken(context: Context, channel: NativeChannel): String =
        runOnMainSync {
            check(!cancelled.remove(channel)) { "SMS token request was cancelled" }
            val action = "${context.packageName}$TOKEN_ACTION_SUFFIX${System.identityHashCode(channel)}"
            val registration = Registration(context)
            check(registrations.putIfAbsent(channel, registration) == null) {
                "duplicate OTP request channel"
            }
            val receiver = object : BroadcastReceiver() {
                override fun onReceive(receiverContext: Context, intent: Intent) {
                    val message = Telephony.Sms.Intents.getMessagesFromIntent(intent)
                        .joinToString(separator = "") { it.messageBody }
                    finish(channel, OtpEvent.Message(message))
                }
            }
            registration.receiver = receiver
            try {
                registerPrivateReceiver(context, receiver, IntentFilter(action))
            } catch (error: Exception) {
                registrations.remove(channel, registration)
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
                System.identityHashCode(channel),
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
            smsManager.createAppSpecificSmsToken(pendingIntent)
                ?: throw IllegalStateException("SmsManager returned no app-specific token")
        }

    @JvmStatic
    fun startSmsUserConsent(context: Context, sender: String?, channel: NativeChannel) {
        runOnMain {
            if (cancelled.remove(channel)) return@runOnMain
            try {
                registerRetrieverReceiver(context, channel, consent = true)
                SmsRetriever.getClient(context).startSmsUserConsent(sender)
                    .addOnSuccessListener { channel.send(OtpEvent.Started) }
                    .addOnFailureListener { error ->
                        finish(channel, OtpEvent.Failed(error.message ?: "SMS User Consent failed"))
                    }
            } catch (error: Exception) {
                finish(channel, OtpEvent.Failed(error.message ?: "SMS User Consent setup failed"))
            }
        }
    }

    @JvmStatic
    @Suppress("UNUSED_PARAMETER")
    fun cancel(context: Context, channel: NativeChannel) {
        cancelled.add(channel)
        check(mainHandler.post {
            cancelled.remove(channel)
            if (unregister(channel)) {
                channel.close()
            }
        }) { "main looper rejected OTP cancellation" }
    }

    private fun registerRetrieverReceiver(context: Context, channel: NativeChannel, consent: Boolean) {
        val registration = Registration(context)
        check(registrations.putIfAbsent(channel, registration) == null) {
            "duplicate OTP request channel"
        }
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(receiverContext: Context, intent: Intent) {
                val status = intent.parcelableExtra<Status>(SmsRetriever.EXTRA_STATUS)
                when (status?.statusCode) {
                    CommonStatusCodes.SUCCESS -> {
                        if (consent) {
                            val consentIntent =
                                intent.parcelableExtra<Intent>(SmsRetriever.EXTRA_CONSENT_INTENT)
                            if (consentIntent == null || !validConsentIntent(context, consentIntent)) {
                                finish(
                                    channel,
                                    OtpEvent.Failed("SMS User Consent returned an untrusted intent"),
                                )
                            } else {
                                launchConsent(context, channel, consentIntent)
                            }
                        } else {
                            val message = intent.getStringExtra(SmsRetriever.EXTRA_SMS_MESSAGE)
                            if (message == null) {
                                finish(
                                    channel,
                                    OtpEvent.Failed("SMS Retriever returned no message"),
                                )
                            } else {
                                finish(channel, OtpEvent.Message(message))
                            }
                        }
                    }

                    CommonStatusCodes.TIMEOUT -> finish(channel, OtpEvent.Timeout)
                    else -> finish(
                        channel,
                        OtpEvent.Failed("SMS API returned unknown status ${status?.statusCode}"),
                    )
                }
            }
        }
        registration.receiver = receiver
        try {
            registerPlayServicesReceiver(context, receiver)
        } catch (error: Exception) {
            registrations.remove(channel, registration)
            throw error
        }
    }

    private fun launchConsent(context: Context, channel: NativeChannel, consentIntent: Intent) {
        val activity = context as? ComponentActivity
        if (activity == null) {
            finish(
                channel,
                OtpEvent.Failed(
                    "SMS User Consent requires the Android context to be a ComponentActivity",
                ),
            )
            return
        }

        try {
            val launcher = activity.activityResultRegistry.register(
                "$CONSENT_KEY_PREFIX${System.identityHashCode(channel)}",
                ActivityResultContracts.StartActivityForResult(),
            ) { result ->
                if (result.resultCode == Activity.RESULT_OK) {
                    val message =
                        result.data?.getStringExtra(SmsRetriever.EXTRA_SMS_MESSAGE)
                    if (message == null) {
                        finish(
                            channel,
                            OtpEvent.Failed("SMS User Consent returned no message"),
                        )
                    } else {
                        finish(channel, OtpEvent.Message(message))
                    }
                } else {
                    finish(channel, OtpEvent.Denied)
                }
            }
            registrations[channel]?.launcher = launcher
            launcher.launch(consentIntent)
        } catch (error: Exception) {
            finish(channel, OtpEvent.Failed(error.message ?: "SMS consent prompt failed"))
        }
    }

    private fun validConsentIntent(context: Context, intent: Intent): Boolean {
        val packageName = intent.resolveActivity(context.packageManager)?.packageName
        return packageName == "com.google.android.gms" &&
            intent.flags and URI_PERMISSION_FLAGS == 0
    }

    private fun registerPlayServicesReceiver(context: Context, receiver: BroadcastReceiver) {
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

    private fun registerPrivateReceiver(
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

    /** Terminal event: unregisters the request, then sends it and closes the channel. */
    private fun finish(channel: NativeChannel, event: OtpEvent) {
        runOnMain {
            if (unregister(channel)) {
                channel.send(event)
                channel.close()
            }
        }
    }

    private fun unregister(channel: NativeChannel): Boolean {
        val registration = registrations.remove(channel) ?: return false
        registration.receiver?.let { registration.context.unregisterReceiver(it) }
        registration.launcher?.unregister()
        return true
    }

    private inline fun runOnMain(crossinline block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            block()
        } else {
            mainHandler.post { block() }
        }
    }

    private fun <T> runOnMainSync(block: () -> T): T {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            return block()
        }
        val task = FutureTask(block)
        check(mainHandler.post(task)) { "main looper rejected OTP operation" }
        return try {
            task.get()
        } catch (error: ExecutionException) {
            throw (error.cause ?: error)
        }
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

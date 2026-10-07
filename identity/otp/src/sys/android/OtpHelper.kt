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

    private val registrations = ConcurrentHashMap<Long, Registration>()
    private val cancelled = ConcurrentHashMap.newKeySet<Long>()
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
    fun startSmsRetriever(context: Context, id: Long) {
        runOnMain {
            if (cancelled.remove(id)) return@runOnMain
            try {
                registerRetrieverReceiver(context, id, consent = false)
                SmsRetriever.getClient(context).startSmsRetriever()
                    .addOnSuccessListener { onStarted(id) }
                    .addOnFailureListener { error ->
                        finish(id) { onFailed(id, error.message ?: "SMS Retriever failed") }
                    }
            } catch (error: Exception) {
                finish(id) { onFailed(id, error.message ?: "SMS Retriever setup failed") }
            }
        }
    }

    @JvmStatic
    @Suppress("DEPRECATION")
    fun createAppSpecificSmsToken(context: Context, id: Long): String =
        runOnMainSync {
            check(!cancelled.remove(id)) { "SMS token request was cancelled" }
            val action = "${context.packageName}$TOKEN_ACTION_SUFFIX$id"
            val registration = Registration(context)
            check(registrations.putIfAbsent(id, registration) == null) {
                "duplicate OTP request id $id"
            }
            val receiver = object : BroadcastReceiver() {
                override fun onReceive(receiverContext: Context, intent: Intent) {
                    val message = Telephony.Sms.Intents.getMessagesFromIntent(intent)
                        .joinToString(separator = "") { it.messageBody }
                    finish(id) { onMessage(id, message) }
                }
            }
            registration.receiver = receiver
            try {
                registerPrivateReceiver(context, receiver, IntentFilter(action))
            } catch (error: Exception) {
                registrations.remove(id, registration)
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
                id.toInt(),
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
    fun startSmsUserConsent(context: Context, sender: String?, id: Long) {
        runOnMain {
            if (cancelled.remove(id)) return@runOnMain
            try {
                registerRetrieverReceiver(context, id, consent = true)
                SmsRetriever.getClient(context).startSmsUserConsent(sender)
                    .addOnSuccessListener { onStarted(id) }
                    .addOnFailureListener { error ->
                        finish(id) { onFailed(id, error.message ?: "SMS User Consent failed") }
                    }
            } catch (error: Exception) {
                finish(id) { onFailed(id, error.message ?: "SMS User Consent setup failed") }
            }
        }
    }

    @JvmStatic
    @Suppress("UNUSED_PARAMETER")
    fun cancel(context: Context, id: Long) {
        cancelled.add(id)
        check(mainHandler.post {
            unregister(id)
            cancelled.remove(id)
        }) { "main looper rejected OTP cancellation" }
    }

    private fun registerRetrieverReceiver(context: Context, id: Long, consent: Boolean) {
        val registration = Registration(context)
        check(registrations.putIfAbsent(id, registration) == null) {
            "duplicate OTP request id $id"
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
                                finish(id) {
                                    onFailed(id, "SMS User Consent returned an untrusted intent")
                                }
                            } else {
                                launchConsent(context, id, consentIntent)
                            }
                        } else {
                            val message = intent.getStringExtra(SmsRetriever.EXTRA_SMS_MESSAGE)
                            if (message == null) {
                                finish(id) {
                                    onFailed(id, "SMS Retriever returned no message")
                                }
                            } else {
                                finish(id) { onMessage(id, message) }
                            }
                        }
                    }

                    CommonStatusCodes.TIMEOUT -> finish(id) { onTimeout(id) }
                    else -> finish(id) {
                        onFailed(
                            id,
                            "SMS API returned unknown status ${status?.statusCode}",
                        )
                    }
                }
            }
        }
        registration.receiver = receiver
        try {
            registerPlayServicesReceiver(context, receiver)
        } catch (error: Exception) {
            registrations.remove(id, registration)
            throw error
        }
    }

    private fun launchConsent(context: Context, id: Long, consentIntent: Intent) {
        val activity = context as? ComponentActivity
        if (activity == null) {
            finish(id) {
                onFailed(
                    id,
                    "SMS User Consent requires the Android context to be a ComponentActivity",
                )
            }
            return
        }

        try {
            val launcher = activity.activityResultRegistry.register(
                "$CONSENT_KEY_PREFIX$id",
                ActivityResultContracts.StartActivityForResult(),
            ) { result ->
                if (result.resultCode == Activity.RESULT_OK) {
                    val message =
                        result.data?.getStringExtra(SmsRetriever.EXTRA_SMS_MESSAGE)
                    if (message == null) {
                        finish(id) {
                            onFailed(id, "SMS User Consent returned no message")
                        }
                    } else {
                        finish(id) { onMessage(id, message) }
                    }
                } else {
                    finish(id) { onDenied(id) }
                }
            }
            registrations[id]?.launcher = launcher
            launcher.launch(consentIntent)
        } catch (error: Exception) {
            finish(id) { onFailed(id, error.message ?: "SMS consent prompt failed") }
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

    private fun finish(id: Long, callback: () -> Unit) {
        runOnMain {
            if (unregister(id)) callback()
        }
    }

    private fun unregister(id: Long): Boolean {
        val registration = registrations.remove(id) ?: return false
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

    @JvmStatic
    external fun onStarted(id: Long)

    @JvmStatic
    external fun onMessage(id: Long, text: String)

    @JvmStatic
    external fun onTimeout(id: Long)

    @JvmStatic
    external fun onDenied(id: Long)

    @JvmStatic
    external fun onFailed(id: Long, message: String)
}

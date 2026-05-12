package waterkit.biometric

import android.content.Context
import android.content.pm.PackageManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import android.os.Handler
import android.os.Looper
import java.util.concurrent.Executor

class BiometricHelper {
    companion object {
        @JvmStatic
        fun isAvailable(context: Context): Boolean {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) {
                return false
            }
            // Simple check: does the device have hardware?
            // For more robust check we should use BiometricManager but that is AndroidX or API 29+
            // For API 28 (Pie), we can check PackageManager
            val pm = context.packageManager
            return pm.hasSystemFeature(PackageManager.FEATURE_FINGERPRINT) ||
                   pm.hasSystemFeature(PackageManager.FEATURE_FACE) ||
                   pm.hasSystemFeature(PackageManager.FEATURE_IRIS)
        }

        @JvmStatic
        fun getBiometricType(context: Context): Int {
            // 0: None, 1: Fingerprint, 2: Face, 3: Iris
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) {
                return 0
            }
            val pm = context.packageManager
            if (pm.hasSystemFeature(PackageManager.FEATURE_FINGERPRINT)) return 1
            if (pm.hasSystemFeature(PackageManager.FEATURE_FACE)) return 2
            if (pm.hasSystemFeature(PackageManager.FEATURE_IRIS)) return 3
            return 0
        }

        @JvmStatic
        fun authenticate(context: Context, reason: String, callbackPtr: Long) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) {
                onResult(callbackPtr, false, "Android version not supported (requires API 28+)")
                return
            }

            val mainHandler = Handler(Looper.getMainLooper())
            mainHandler.post {
                try {
                    val executor = Executor { command -> mainHandler.post(command) }
                    
                    val prompt = BiometricPrompt.Builder(context)
                        .setTitle("Authentication Required")
                        .setDescription(reason)
                        .setNegativeButton("Cancel", executor) { _, _ ->
                            onResult(callbackPtr, false, "Cancelled by user")
                        }
                        .build()

                    prompt.authenticate(
                        CancellationSignal(),
                        executor,
                        object : BiometricPrompt.AuthenticationCallback() {
                            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                                super.onAuthenticationSucceeded(result)
                                onResult(callbackPtr, true, null)
                            }

                            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                                super.onAuthenticationError(errorCode, errString)
                                onResult(callbackPtr, false, errString.toString())
                            }

                            override fun onAuthenticationFailed() {
                                super.onAuthenticationFailed()
                                // Failed attempt, but prompt stays open. 
                                // We don't necessarily fail the whole transaction yet, 
                                // but if we want to notify, we could.
                                // Typically we wait for Error or Success.
                            }
                        }
                    )
                } catch (e: Exception) {
                    onResult(callbackPtr, false, e.message ?: "Unknown error")
                }
            }
        }

        // Native method to call back to Rust
        @JvmStatic
        external fun onResult(callbackPtr: Long, success: Boolean, error: String?)
    }
}

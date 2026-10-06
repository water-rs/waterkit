package waterkit.wallet

import android.app.Activity
import android.content.Context
import android.content.Intent
import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.GoogleApiAvailability
import com.google.android.gms.pay.Pay
import com.google.android.gms.pay.PayApiAvailabilityStatus
import com.google.android.gms.pay.PayClient

class WalletHelper {
    companion object {
        private const val REQUEST_CODE = 0x5741

        @Volatile
        private var pendingSave: Long? = null

        @JvmStatic
        fun requestAvailability(context: Context, requestId: Long) {
            if (
                GoogleApiAvailability.getInstance()
                    .isGooglePlayServicesAvailable(context) != ConnectionResult.SUCCESS
            ) {
                onAvailability(requestId, false)
                return
            }

            Pay.getClient(context)
                .getPayApiAvailabilityStatus(PayClient.RequestType.SAVE_PASSES)
                .addOnSuccessListener { status ->
                    onAvailability(requestId, status == PayApiAvailabilityStatus.AVAILABLE)
                }
                .addOnFailureListener { error ->
                    onAvailabilityError(requestId, error.toString())
                }
        }

        @JvmStatic
        fun savePassesJwt(context: Context, requestId: Long, jwt: String): Boolean {
            val activity = context as? Activity
                ?: throw IllegalStateException(
                    "waterkit-wallet: the published Android context must be an Activity",
                )

            synchronized(WalletHelper::class.java) {
                if (pendingSave != null) {
                    return false
                }
                pendingSave = requestId
            }

            activity.runOnUiThread {
                try {
                    Pay.getClient(activity).savePassesJwt(jwt, activity, REQUEST_CODE)
                } catch (error: Exception) {
                    synchronized(WalletHelper::class.java) {
                        if (pendingSave == requestId) {
                            pendingSave = null
                        }
                    }
                    onSaveFailed(requestId, error.toString())
                }
            }
            return true
        }

        /**
         * The host Activity must forward its `onActivityResult` arguments here.
         * The result is handled only for this helper's request code.
         */
        @JvmStatic
        fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?): Boolean {
            if (requestCode != REQUEST_CODE) {
                return false
            }

            val requestId = synchronized(WalletHelper::class.java) {
                val id = pendingSave
                    ?: throw IllegalStateException(
                        "waterkit-wallet: received wallet activity result with no save pending",
                    )
                pendingSave = null
                id
            }
            onSaveResult(
                requestId,
                resultCode,
                data?.getStringExtra(PayClient.EXTRA_API_ERROR_MESSAGE),
            )
            return true
        }

        @JvmStatic
        external fun onAvailability(requestId: Long, available: Boolean)

        @JvmStatic
        external fun onAvailabilityError(requestId: Long, message: String)

        @JvmStatic
        external fun onSaveResult(requestId: Long, resultCode: Int, errorMessage: String?)

        @JvmStatic
        external fun onSaveFailed(requestId: Long, message: String)
    }
}

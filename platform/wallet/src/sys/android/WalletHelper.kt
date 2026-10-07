package waterkit.wallet

import android.content.Context
import android.content.Intent
import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.GoogleApiAvailability
import com.google.android.gms.pay.Pay
import com.google.android.gms.pay.PayApiAvailabilityStatus
import com.google.android.gms.pay.PayClient

class WalletHelper {
    companion object {
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
        fun savePassesIntent(context: Context, jwt: String): Intent =
            Intent(context, SavePassesActivity::class.java)
                .putExtra(SavePassesActivity.EXTRA_JWT, jwt)

        @JvmStatic
        fun apiErrorMessage(data: Intent): String? =
            data.getStringExtra(PayClient.EXTRA_API_ERROR_MESSAGE)

        @JvmStatic
        external fun onAvailability(requestId: Long, available: Boolean)

        @JvmStatic
        external fun onAvailabilityError(requestId: Long, message: String)
    }
}

package waterkit.wallet

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import com.google.android.gms.pay.Pay
import com.google.android.gms.pay.PayClient

/**
 * Library-owned trampoline that hosts the Google Wallet save flow. The Wallet
 * API delivers its result to this activity's `onActivityResult`, which
 * forwards it as this activity's own result so the caller's activity-result
 * contract receives it. It is translucent, excluded from recents, and shows no
 * UI of its own.
 */
class SavePassesActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState != null) {
            // A recreation is already inside the save flow; starting the
            // request again would launch a second wallet UI.
            return
        }
        val jwt =
            intent.getStringExtra(EXTRA_JWT)
                ?: error("waterkit-wallet: SavePassesActivity requires the $EXTRA_JWT extra")
        try {
            Pay.getClient(this).savePassesJwt(jwt, this, REQUEST_CODE)
        } catch (error: Exception) {
            setResult(
                PayClient.SavePassesResult.INTERNAL_ERROR,
                Intent().putExtra(PayClient.EXTRA_API_ERROR_MESSAGE, error.toString()),
            )
            finish()
        }
    }

    @Deprecated("a plain Activity cannot use the AndroidX activity-result registry")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode == REQUEST_CODE) {
            setResult(resultCode, data)
        }
        finish()
    }

    companion object {
        const val EXTRA_JWT = "waterkit.wallet.EXTRA_JWT"
        private const val REQUEST_CODE = 0x5741
    }
}

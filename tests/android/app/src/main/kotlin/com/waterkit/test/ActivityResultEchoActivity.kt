package com.waterkit.test

import android.app.Activity
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.IntentSender
import java.util.concurrent.atomic.AtomicInteger

class ActivityResultEchoActivity : Activity() {
    override fun onCreate(savedInstanceState: android.os.Bundle?) {
        super.onCreate(savedInstanceState)
        setResult(
            intent.getIntExtra("result_code", RESULT_OK),
            Intent().putExtra("token", intent.getStringExtra("token")),
        )
        finish()
    }

    companion object {
        private val nextRequestCode = AtomicInteger(1)

        @JvmStatic
        fun intent(context: Context, token: String, resultCode: Int): Intent {
            return Intent(context, ActivityResultEchoActivity::class.java).apply {
                putExtra("token", token)
                putExtra("result_code", resultCode)
            }
        }

        @JvmStatic
        fun intentSender(context: Context, token: String, resultCode: Int): IntentSender {
            val pendingIntent = PendingIntent.getActivity(
                context,
                nextRequestCode.getAndIncrement(),
                intent(context, token, resultCode),
                PendingIntent.FLAG_IMMUTABLE,
            )
            return pendingIntent.intentSender
        }

        @JvmStatic
        fun token(data: Intent?): String? {
            return data?.getStringExtra("token")
        }
    }
}

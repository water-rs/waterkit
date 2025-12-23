package waterkit.notification

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.Context.NOTIFICATION_SERVICE
import android.os.Build
import android.app.Notification

class NotificationHelper {
    companion object {
        @JvmStatic
        fun showNotification(context: Context, title: String, body: String) {
            val manager = context.getSystemService(NOTIFICATION_SERVICE) as NotificationManager
            val channelId = "water_notification_channel"

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                val channel = NotificationChannel(channelId, "Notifications", NotificationManager.IMPORTANCE_DEFAULT)
                manager.createNotificationChannel(channel)
            }

            val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Notification.Builder(context, channelId)
            } else {
                Notification.Builder(context)
            }

            builder.setContentTitle(title)
                .setContentText(body)
                .setSmallIcon(android.R.drawable.ic_dialog_info)
                .setAutoCancel(true)

            manager.notify(System.currentTimeMillis().toInt(), builder.build())
        }
    }
}

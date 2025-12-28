package waterkit.notification

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Context.NOTIFICATION_SERVICE
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.app.Notification
import org.json.JSONArray

class NotificationHelper {
    companion object {
        @JvmStatic
        fun showNotification(
            context: Context,
            title: String,
            body: String,
            importance: Int,
            actionsJson: String
        ) {
            val manager = context.getSystemService(NOTIFICATION_SERVICE) as NotificationManager

            // Create channel ID based on importance level
            val channelId = when (importance) {
                2 -> "waterkit_low"
                3 -> "waterkit_default"
                4 -> "waterkit_high"
                5 -> "waterkit_max"
                else -> "waterkit_default"
            }

            val channelName = when (importance) {
                2 -> "Low Priority"
                3 -> "Default"
                4 -> "Time Sensitive"
                5 -> "Critical"
                else -> "Default"
            }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                val channel = NotificationChannel(channelId, channelName, importance)
                manager.createNotificationChannel(channel)
            }

            val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Notification.Builder(context, channelId)
            } else {
                @Suppress("DEPRECATION")
                Notification.Builder(context).setPriority(
                    when (importance) {
                        2 -> Notification.PRIORITY_LOW
                        3 -> Notification.PRIORITY_DEFAULT
                        4 -> Notification.PRIORITY_HIGH
                        5 -> Notification.PRIORITY_MAX
                        else -> Notification.PRIORITY_DEFAULT
                    }
                )
            }

            builder.setContentTitle(title)
                .setContentText(body)
                .setSmallIcon(android.R.drawable.ic_dialog_info)
                .setAutoCancel(true)

            // Parse and add actions
            if (actionsJson.isNotEmpty()) {
                try {
                    val actions = JSONArray(actionsJson)
                    for (i in 0 until actions.length()) {
                        val action = actions.getJSONObject(i)
                        val label = action.getString("label")
                        val url = action.getString("url")

                        // Create intent to open URL
                        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
                        val pendingIntent = PendingIntent.getActivity(
                            context,
                            i,
                            intent,
                            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
                        )

                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                            builder.addAction(
                                Notification.Action.Builder(
                                    null,  // No icon
                                    label,
                                    pendingIntent
                                ).build()
                            )
                        } else {
                            @Suppress("DEPRECATION")
                            builder.addAction(0, label, pendingIntent)
                        }
                    }
                } catch (e: Exception) {
                    // Ignore JSON parsing errors
                }
            }

            manager.notify(System.currentTimeMillis().toInt(), builder.build())
        }
    }
}

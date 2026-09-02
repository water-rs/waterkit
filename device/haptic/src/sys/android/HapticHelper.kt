package waterkit.haptic

import android.content.Context
import android.os.Build
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager

class HapticHelper {
    companion object {
        private fun getVibrator(context: Context): Vibrator? {
            return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                val manager = context.getSystemService(Context.VIBRATOR_MANAGER_SERVICE) as? VibratorManager
                manager?.defaultVibrator
            } else {
                @Suppress("DEPRECATION")
                context.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
            }
        }

        @JvmStatic
        fun isAvailable(context: Context): Boolean {
            return getVibrator(context)?.hasVibrator() == true
        }

        @JvmStatic
        fun impact(context: Context, intensity: Float) {
            val vibrator = getVibrator(context) ?: return
            val amplitude = (intensity * 255).toInt().coerceIn(1, 255)
            val duration = when {
                intensity < 0.35f -> 10L
                intensity < 0.65f -> 20L
                intensity < 0.85f -> 35L
                else -> 50L
            }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                vibrator.vibrate(VibrationEffect.createOneShot(duration, amplitude))
            } else {
                @Suppress("DEPRECATION")
                vibrator.vibrate(duration)
            }
        }

        @JvmStatic
        fun selection(context: Context) {
            val vibrator = getVibrator(context) ?: return
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                vibrator.vibrate(VibrationEffect.createPredefined(VibrationEffect.EFFECT_TICK))
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                vibrator.vibrate(VibrationEffect.createOneShot(10, VibrationEffect.DEFAULT_AMPLITUDE))
            } else {
                @Suppress("DEPRECATION")
                vibrator.vibrate(10)
            }
        }

        @JvmStatic
        fun notification(context: Context, notificationType: Int) {
            val vibrator = getVibrator(context) ?: return

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                val (timings, amplitudes) = when (notificationType) {
                    0 -> longArrayOf(0, 50, 50, 100) to intArrayOf(0, 200, 0, 255) // success
                    1 -> longArrayOf(0, 50, 100, 50) to intArrayOf(0, 255, 0, 200) // warning
                    else -> longArrayOf(0, 50, 50, 50, 50, 100) to intArrayOf(0, 255, 0, 255, 0, 255) // error
                }
                vibrator.vibrate(VibrationEffect.createWaveform(timings, amplitudes, -1))
            } else {
                @Suppress("DEPRECATION")
                vibrator.vibrate(100)
            }
        }

        @JvmStatic
        fun playPattern(context: Context, timings: LongArray, amplitudes: IntArray): Boolean {
            val vibrator = getVibrator(context) ?: return false

            return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                try {
                    vibrator.vibrate(VibrationEffect.createWaveform(timings, amplitudes, -1))
                    true
                } catch (e: Exception) {
                    false
                }
            } else {
                @Suppress("DEPRECATION")
                try {
                    vibrator.vibrate(timings, -1)
                    true
                } catch (e: Exception) {
                    false
                }
            }
        }
    }
}

package waterkit.haptic

import android.content.Context
import android.os.Build
import android.os.VibrationEffect
import android.os.Vibrator

class HapticHelper {
    companion object {
        // Constants matching Rust side
        const val STYLE_LIGHT = 0
        const val STYLE_MEDIUM = 1
        const val STYLE_HEAVY = 2
        const val STYLE_RIGID = 3
        const val STYLE_SOFT = 4
        const val STYLE_SELECTION = 5
        const val STYLE_SUCCESS = 6
        const val STYLE_WARNING = 7
        const val STYLE_ERROR = 8

        @JvmStatic
        fun feedback(context: Context, style: Int) {
            val vibrator = context.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
            if (vibrator == null || !vibrator.hasVibrator()) {
                return
            }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                // API 29+
                val effect = when (style) {
                    STYLE_LIGHT -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_TICK)
                    STYLE_MEDIUM -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_CLICK)
                    STYLE_HEAVY -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_HEAVY_CLICK)
                    STYLE_RIGID -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_DOUBLE_CLICK) // Best approximation
                    STYLE_SOFT -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_TICK)
                    STYLE_SELECTION -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_TICK)
                    STYLE_SUCCESS -> VibrationEffect.createWaveform(longArrayOf(0, 50, 50, 100), -1)
                    STYLE_WARNING -> VibrationEffect.createWaveform(longArrayOf(0, 50, 100, 50), -1)
                    STYLE_ERROR -> VibrationEffect.createWaveform(longArrayOf(0, 50, 50, 50, 50, 100), -1)
                    else -> null
                }
                if (effect != null) {
                    vibrator.vibrate(effect)
                }
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                // API 26+
                val effect = when (style) {
                    STYLE_LIGHT -> VibrationEffect.createOneShot(10, VibrationEffect.DEFAULT_AMPLITUDE)
                    STYLE_MEDIUM -> VibrationEffect.createOneShot(20, VibrationEffect.DEFAULT_AMPLITUDE)
                    STYLE_HEAVY -> VibrationEffect.createOneShot(50, VibrationEffect.DEFAULT_AMPLITUDE)
                    else -> VibrationEffect.createOneShot(20, VibrationEffect.DEFAULT_AMPLITUDE)
                }
                vibrator.vibrate(effect)
            } else {
                // Older devices
                vibrator.vibrate(20)
            }
        }
    }
}

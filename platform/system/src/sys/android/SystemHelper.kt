package com.waterkit.system

import android.app.ActivityManager
import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Build
import android.os.PowerManager

/**
 * System queries for `waterkit-system`.
 *
 * Every method lets a platform failure throw, so the Rust side reports it as an
 * error; a value the device does not offer is `null` instead.
 */
object SystemHelper {
    /**
     * The active network's transport: 0 none, 1 Wi-Fi, 2 cellular,
     * 3 Ethernet, 4 Bluetooth, 5 VPN, 6 other.
     */
    @JvmStatic
    fun getConnectivity(context: Context): Int {
        val cm = checkNotNull(context.getSystemService(ConnectivityManager::class.java)) {
            "ConnectivityManager service is unavailable"
        }
        val network = cm.activeNetwork ?: return 0
        // The network can disconnect between the two calls.
        val caps = cm.getNetworkCapabilities(network) ?: return 0

        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) return 1
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)) return 2
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) return 3
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_BLUETOOTH)) return 4
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) return 5
        return 6
    }

    /**
     * `PowerManager.getCurrentThermalStatus()`, or `null` before API level 29,
     * which has no thermal status.
     */
    @JvmStatic
    fun getThermalState(context: Context): Int? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return null
        val pm = checkNotNull(context.getSystemService(PowerManager::class.java)) {
            "PowerManager service is unavailable"
        }
        return pm.currentThermalStatus
    }

    /** Used and total physical memory, in bytes. */
    class MemoryLoad(
        @JvmField val used: Long,
        @JvmField val total: Long,
    )

    @JvmStatic
    fun getMemoryLoad(context: Context): MemoryLoad {
        val am = checkNotNull(context.getSystemService(ActivityManager::class.java)) {
            "ActivityManager service is unavailable"
        }
        val memInfo = ActivityManager.MemoryInfo()
        am.getMemoryInfo(memInfo)
        return MemoryLoad(memInfo.totalMem - memInfo.availMem, memInfo.totalMem)
    }
}

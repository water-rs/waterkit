package com.waterkit.system

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Build
import android.os.PowerManager
import android.app.ActivityManager
import java.io.RandomAccessFile

object SystemHelper {
    // Previous CPU stats for delta calculation
    private var prevCpuStats: LongArray? = null

    fun getConnectivity(context: Context): Int {
        val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
        val network = cm?.activeNetwork ?: return 0 // None
        val caps = cm.getNetworkCapabilities(network) ?: return 0

        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) return 1 // Wifi
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)) return 2 // Cellular
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) return 3 // Ethernet
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_BLUETOOTH)) return 4 // Bluetooth
        if (caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) return 5 // Vpn
        return 6 // Other
    }

    fun getThermalState(context: Context): Int {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val pm = context.getSystemService(Context.POWER_SERVICE) as? PowerManager
            return pm?.currentThermalStatus ?: -1
        }
        return -1 // Unknown
    }

    data class LoadInfo(val cpu: Float, val memUsed: Long, val memTotal: Long)

    fun getSystemLoad(context: Context): LoadInfo {
        val actManager = context.getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager
        val memInfo = ActivityManager.MemoryInfo()
        actManager?.getMemoryInfo(memInfo)

        val cpuUsage = getCpuUsage()

        return LoadInfo(cpuUsage, memInfo.totalMem - memInfo.availMem, memInfo.totalMem)
    }

    private fun getCpuUsage(): Float {
        try {
            val reader = RandomAccessFile("/proc/stat", "r")
            val line = reader.readLine()
            reader.close()

            // Line format: cpu  user nice system idle iowait irq softirq steal guest guest_nice
            val parts = line.split("\\s+".toRegex())
            if (parts.size < 8) return 0.0f

            val user = parts[1].toLongOrNull() ?: 0L
            val nice = parts[2].toLongOrNull() ?: 0L
            val system = parts[3].toLongOrNull() ?: 0L
            val idle = parts[4].toLongOrNull() ?: 0L
            val iowait = parts[5].toLongOrNull() ?: 0L
            val irq = parts[6].toLongOrNull() ?: 0L
            val softirq = parts[7].toLongOrNull() ?: 0L
            val steal = if (parts.size > 8) parts[8].toLongOrNull() ?: 0L else 0L

            val total = user + nice + system + idle + iowait + irq + softirq + steal
            val used = user + nice + system + irq + softirq + steal

            val currentStats = longArrayOf(total, used)
            val prev = prevCpuStats

            if (prev != null) {
                val diffTotal = total - prev[0]
                val diffUsed = used - prev[1]
                prevCpuStats = currentStats
                if (diffTotal > 0) {
                    return (diffUsed.toFloat() / diffTotal.toFloat()) * 100.0f
                }
            }

            prevCpuStats = currentStats
            // First call - return instantaneous
            if (total > 0) {
                return (used.toFloat() / total.toFloat()) * 100.0f
            }
        } catch (e: Exception) {
            // Ignore errors
        }
        return 0.0f
    }
}


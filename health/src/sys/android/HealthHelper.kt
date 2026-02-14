package waterkit.health

object HealthHelper {
    // Health Connect API requires Activity context and is callback-based.
    // This helper provides the JNI bridge surface for Rust.

    @JvmStatic
    fun isAvailable(): Boolean {
        return try {
            Class.forName("androidx.health.connect.client.HealthConnectClient")
            true
        } catch (e: ClassNotFoundException) {
            false
        }
    }
}

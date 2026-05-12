package waterkit.health

import android.content.Context
import android.content.Intent
import androidx.health.connect.client.HealthConnectClient
import androidx.health.connect.client.permission.HealthPermission
import androidx.health.connect.client.records.ActiveCaloriesBurnedRecord
import androidx.health.connect.client.records.DistanceRecord
import androidx.health.connect.client.records.HeartRateRecord
import androidx.health.connect.client.records.HeightRecord
import androidx.health.connect.client.records.OxygenSaturationRecord
import androidx.health.connect.client.records.Record
import androidx.health.connect.client.records.SleepSessionRecord
import androidx.health.connect.client.records.StepsRecord
import androidx.health.connect.client.records.WeightRecord
import androidx.health.connect.client.records.metadata.Metadata
import androidx.health.connect.client.request.ReadRecordsRequest
import androidx.health.connect.client.time.TimeRangeFilter
import androidx.health.connect.client.units.Energy
import androidx.health.connect.client.units.Length
import androidx.health.connect.client.units.Mass
import androidx.health.connect.client.units.Percentage
import java.time.Duration
import java.time.Instant
import java.time.ZoneOffset
import kotlin.math.roundToLong
import kotlin.reflect.KClass
import kotlinx.coroutines.runBlocking

object HealthHelper {
    @JvmStatic
    fun isAvailable(): Boolean {
        return try {
            Class.forName("androidx.health.connect.client.HealthConnectClient")
            true
        } catch (e: ClassNotFoundException) {
            false
        }
    }

    @JvmStatic
    fun requestAuthorization(context: Context, readTypesCsv: String, writeTypesCsv: String): String? {
        return runBlocking {
            try {
                val client = HealthConnectClient.getOrCreate(context.applicationContext)
                val required = linkedSetOf<String>()

                parseTypes(readTypesCsv).forEach { type ->
                    required += readPermission(type)
                }
                parseTypes(writeTypesCsv).forEach { type ->
                    required += writePermission(type)
                }

                if (required.isEmpty()) {
                    return@runBlocking null
                }

                val granted = client.permissionController.getGrantedPermissions()
                val missing = required.filterNot(granted::contains)
                if (missing.isEmpty()) {
                    null
                } else {
                    val intent = HealthConnectClient.getHealthConnectManageDataIntent(
                        context.applicationContext
                    )
                    intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    context.applicationContext.startActivity(intent)
                    "Missing Health Connect permissions: ${missing.joinToString(",")}"
                }
            } catch (error: Throwable) {
                error.message ?: "Health Connect authorization check failed"
            }
        }
    }

    @JvmStatic
    fun querySamples(
        context: Context,
        dataType: String,
        startIso: String,
        endIso: String
    ): String {
        return runBlocking {
            val client = HealthConnectClient.getOrCreate(context.applicationContext)
            val start = Instant.parse(startIso)
            val end = Instant.parse(endIso)
            val timeRange = TimeRangeFilter.between(start, end)

            when (dataType) {
                "steps" -> {
                    ensurePermission(client, readPermission(dataType))
                    client.readRecords(ReadRecordsRequest(StepsRecord::class, timeRange)).records
                        .joinToString("\n") { record ->
                            formatSample(
                                value = record.count.toDouble(),
                                unit = "count",
                                start = record.startTime,
                                end = record.endTime,
                                source = record.metadata.dataOrigin.packageName
                            )
                        }
                }

                "heartRate" -> {
                    ensurePermission(client, readPermission(dataType))
                    client.readRecords(ReadRecordsRequest(HeartRateRecord::class, timeRange)).records
                        .joinToString("\n") { record ->
                            val average = if (record.samples.isEmpty()) {
                                0.0
                            } else {
                                record.samples.map { it.beatsPerMinute.toDouble() }.average()
                            }
                            formatSample(
                                value = average,
                                unit = "bpm",
                                start = record.startTime,
                                end = record.endTime,
                                source = record.metadata.dataOrigin.packageName
                            )
                        }
                }

                "activeEnergy" -> {
                    ensurePermission(client, readPermission(dataType))
                    client.readRecords(
                        ReadRecordsRequest(ActiveCaloriesBurnedRecord::class, timeRange)
                    ).records.joinToString("\n") { record ->
                        formatSample(
                            value = record.energy.inCalories,
                            unit = "kcal",
                            start = record.startTime,
                            end = record.endTime,
                            source = record.metadata.dataOrigin.packageName
                        )
                    }
                }

                "distance" -> {
                    ensurePermission(client, readPermission(dataType))
                    client.readRecords(ReadRecordsRequest(DistanceRecord::class, timeRange)).records
                        .joinToString("\n") { record ->
                            formatSample(
                                value = record.distance.inMeters,
                                unit = "m",
                                start = record.startTime,
                                end = record.endTime,
                                source = record.metadata.dataOrigin.packageName
                            )
                        }
                }

                "weight" -> {
                    ensurePermission(client, readPermission(dataType))
                    client.readRecords(ReadRecordsRequest(WeightRecord::class, timeRange)).records
                        .joinToString("\n") { record ->
                            formatSample(
                                value = record.weight.inKilograms,
                                unit = "kg",
                                start = record.time,
                                end = record.time,
                                source = record.metadata.dataOrigin.packageName
                            )
                        }
                }

                "height" -> {
                    ensurePermission(client, readPermission(dataType))
                    client.readRecords(ReadRecordsRequest(HeightRecord::class, timeRange)).records
                        .joinToString("\n") { record ->
                            formatSample(
                                value = record.height.inMeters,
                                unit = "m",
                                start = record.time,
                                end = record.time,
                                source = record.metadata.dataOrigin.packageName
                            )
                        }
                }

                "bloodOxygen" -> {
                    ensurePermission(client, readPermission(dataType))
                    client.readRecords(
                        ReadRecordsRequest(OxygenSaturationRecord::class, timeRange)
                    ).records.joinToString("\n") { record ->
                        formatSample(
                            value = record.percentage.value,
                            unit = "%",
                            start = record.time,
                            end = record.time,
                            source = record.metadata.dataOrigin.packageName
                        )
                    }
                }

                "sleep" -> {
                    ensurePermission(client, readPermission(dataType))
                    client.readRecords(
                        ReadRecordsRequest(SleepSessionRecord::class, timeRange)
                    ).records.joinToString("\n") { record ->
                        formatSample(
                            value = Duration.between(record.startTime, record.endTime).seconds.toDouble(),
                            unit = "s",
                            start = record.startTime,
                            end = record.endTime,
                            source = record.metadata.dataOrigin.packageName
                        )
                    }
                }

                else -> throw IllegalArgumentException("Unsupported health data type: $dataType")
            }
        }
    }

    @JvmStatic
    fun writeSample(
        context: Context,
        dataType: String,
        value: Double,
        unit: String,
        startIso: String,
        endIso: String
    ): String? {
        return runBlocking {
            try {
                val client = HealthConnectClient.getOrCreate(context.applicationContext)
                ensurePermission(client, writePermission(dataType))

                val start = Instant.parse(startIso)
                val end = Instant.parse(endIso)
                val zoneOffset = ZoneOffset.UTC
                val metadata = Metadata.manualEntry()

                val record: Record = when (dataType) {
                    "steps" -> StepsRecord(
                        start,
                        zoneOffset,
                        end,
                        zoneOffset,
                        value.roundToLong(),
                        metadata
                    )

                    "heartRate" -> HeartRateRecord(
                        start,
                        zoneOffset,
                        end,
                        zoneOffset,
                        listOf(HeartRateRecord.Sample(start, value.roundToLong())),
                        metadata
                    )

                    "activeEnergy" -> ActiveCaloriesBurnedRecord(
                        start,
                        zoneOffset,
                        end,
                        zoneOffset,
                        Energy.calories(value),
                        metadata
                    )

                    "distance" -> DistanceRecord(
                        start,
                        zoneOffset,
                        end,
                        zoneOffset,
                        Length.meters(value),
                        metadata
                    )

                    "weight" -> WeightRecord(
                        end,
                        zoneOffset,
                        Mass.kilograms(value),
                        metadata
                    )

                    "height" -> HeightRecord(
                        end,
                        zoneOffset,
                        Length.meters(value),
                        metadata
                    )

                    "bloodOxygen" -> OxygenSaturationRecord(
                        end,
                        zoneOffset,
                        Percentage(value),
                        metadata
                    )

                    "sleep" -> SleepSessionRecord(
                        start,
                        zoneOffset,
                        end,
                        zoneOffset,
                        metadata
                    )

                    else -> throw IllegalArgumentException("Unsupported health data type: $dataType")
                }

                client.insertRecords(listOf(record))
                null
            } catch (error: Throwable) {
                error.message ?: "writeSample failed"
            }
        }
    }

    private suspend fun ensurePermission(client: HealthConnectClient, permission: String) {
        val granted = client.permissionController.getGrantedPermissions()
        require(granted.contains(permission)) {
            "Health Connect permission missing: $permission"
        }
    }

    private fun parseTypes(csv: String): List<String> {
        return csv.split(',').map(String::trim).filter(String::isNotEmpty)
    }

    private fun recordClass(type: String): KClass<out Record> {
        return when (type) {
            "steps" -> StepsRecord::class
            "heartRate" -> HeartRateRecord::class
            "activeEnergy" -> ActiveCaloriesBurnedRecord::class
            "distance" -> DistanceRecord::class
            "weight" -> WeightRecord::class
            "height" -> HeightRecord::class
            "bloodOxygen" -> OxygenSaturationRecord::class
            "sleep" -> SleepSessionRecord::class
            else -> throw IllegalArgumentException("Unsupported health data type: $type")
        }
    }

    private fun readPermission(type: String): String {
        return HealthPermission.getReadPermission(recordClass(type))
    }

    private fun writePermission(type: String): String {
        return HealthPermission.getWritePermission(recordClass(type))
    }

    private fun formatSample(
        value: Double,
        unit: String,
        start: Instant,
        end: Instant,
        source: String?
    ): String {
        val builder = StringBuilder()
        builder.append(value)
        builder.append('\t')
        builder.append(unit)
        builder.append('\t')
        builder.append(start.toString())
        builder.append('\t')
        builder.append(end.toString())
        builder.append('\t')
        builder.append(source.orEmpty())
        return builder.toString()
    }
}

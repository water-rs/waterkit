package waterkit.calendar

import android.Manifest
import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.content.pm.PackageManager
import android.provider.CalendarContract
import java.text.ParsePosition
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone

object CalendarHelper {
    private val isoPatterns = arrayOf(
        "yyyy-MM-dd'T'HH:mm:ss.SSSXXX",
        "yyyy-MM-dd'T'HH:mm:ssXXX",
        "yyyy-MM-dd'T'HH:mm:ss.SSSX",
        "yyyy-MM-dd'T'HH:mm:ssX",
        "yyyy-MM-dd'T'HH:mm:ss.SSS",
        "yyyy-MM-dd'T'HH:mm:ss",
        "yyyy-MM-dd"
    )

    @JvmStatic
    fun hasCalendarPermission(context: Context, write: Boolean): Boolean {
        val canRead = context.checkCallingOrSelfPermission(Manifest.permission.READ_CALENDAR) ==
            PackageManager.PERMISSION_GRANTED
        if (!canRead) {
            return false
        }
        if (!write) {
            return true
        }
        return context.checkCallingOrSelfPermission(Manifest.permission.WRITE_CALENDAR) ==
            PackageManager.PERMISSION_GRANTED
    }

    @JvmStatic
    fun listCalendars(context: Context): Array<String> {
        val cursor = context.contentResolver.query(
            CalendarContract.Calendars.CONTENT_URI,
            arrayOf(
                CalendarContract.Calendars._ID,
                CalendarContract.Calendars.CALENDAR_DISPLAY_NAME,
                CalendarContract.Calendars.CALENDAR_COLOR,
                CalendarContract.Calendars.CALENDAR_ACCESS_LEVEL
            ),
            null, null, null
        ) ?: throw IllegalStateException("Calendar query returned null cursor")
        val result = mutableListOf<String>()
        cursor.use {
            while (it.moveToNext()) {
                val id = it.getString(0)
                    ?: throw IllegalStateException("Calendar row missing id")
                val name = sanitize(it.getString(1))
                val color = String.format("#%06X", 0xFFFFFF and it.getInt(2))
                val readOnly = if (it.getInt(3) < CalendarContract.Calendars.CAL_ACCESS_CONTRIBUTOR) "1" else "0"
                result.add("$id\t$name\t$color\t$readOnly")
            }
        }
        return result.toTypedArray()
    }

    @JvmStatic
    fun fetchEvents(context: Context, startIso: String, endIso: String): Array<String> {
        val startMillis = parseIso8601(startIso)
        val endMillis = parseIso8601(endIso)
        require(endMillis >= startMillis) {
            "end date must be greater than or equal to start date"
        }

        val builder = CalendarContract.Instances.CONTENT_URI.buildUpon()
        ContentUris.appendId(builder, startMillis)
        ContentUris.appendId(builder, endMillis)

        val cursor = context.contentResolver.query(
            builder.build(),
            arrayOf(
                CalendarContract.Instances.EVENT_ID,
                CalendarContract.Instances.TITLE,
                CalendarContract.Instances.DESCRIPTION,
                CalendarContract.Instances.EVENT_LOCATION,
                CalendarContract.Instances.BEGIN,
                CalendarContract.Instances.END,
                CalendarContract.Instances.ALL_DAY,
                CalendarContract.Instances.CALENDAR_ID
            ),
            null,
            null,
            "${CalendarContract.Instances.BEGIN} ASC"
        ) ?: throw IllegalStateException("Calendar instances query returned null cursor")

        val result = mutableListOf<String>()
        cursor.use {
            while (it.moveToNext()) {
                result.add(
                    encodeEventRow(
                        id = it.getLong(0).toString(),
                        title = it.getString(1),
                        description = it.getString(2),
                        location = it.getString(3),
                        startMillis = if (it.isNull(4)) 0L else it.getLong(4),
                        endMillis = if (it.isNull(5)) 0L else it.getLong(5),
                        allDay = !it.isNull(6) && it.getInt(6) != 0,
                        calendarId = if (it.isNull(7)) "" else it.getLong(7).toString()
                    )
                )
            }
        }
        return result.toTypedArray()
    }

    @JvmStatic
    fun fetchEventById(context: Context, eventId: Long): String? {
        require(eventId > 0L) { "eventId must be positive" }

        val cursor = context.contentResolver.query(
            CalendarContract.Events.CONTENT_URI,
            arrayOf(
                CalendarContract.Events._ID,
                CalendarContract.Events.TITLE,
                CalendarContract.Events.DESCRIPTION,
                CalendarContract.Events.EVENT_LOCATION,
                CalendarContract.Events.DTSTART,
                CalendarContract.Events.DTEND,
                CalendarContract.Events.ALL_DAY,
                CalendarContract.Events.CALENDAR_ID
            ),
            "${CalendarContract.Events._ID}=?",
            arrayOf(eventId.toString()),
            null
        ) ?: throw IllegalStateException("Calendar event query returned null cursor")

        cursor.use {
            if (it.moveToFirst()) {
                return encodeEventRow(
                    id = it.getLong(0).toString(),
                    title = it.getString(1),
                    description = it.getString(2),
                    location = it.getString(3),
                    startMillis = if (it.isNull(4)) 0L else it.getLong(4),
                    endMillis = if (it.isNull(5)) 0L else it.getLong(5),
                    allDay = !it.isNull(6) && it.getInt(6) != 0,
                    calendarId = if (it.isNull(7)) "" else it.getLong(7).toString()
                )
            }
        }
        return null
    }

    @JvmStatic
    fun createEventIso(
        context: Context,
        title: String,
        description: String?,
        location: String?,
        startIso: String,
        endIso: String,
        allDay: Boolean,
        calendarId: String?
    ): Long {
        val startMillis = parseIso8601(startIso)
        val endMillis = parseIso8601(endIso)
        require(endMillis >= startMillis) {
            "end date must be greater than or equal to start date"
        }

        val resolvedCalendarId = resolveCalendarId(context, calendarId)
        return createEvent(
            context = context,
            title = title,
            description = description,
            location = location,
            startMillis = startMillis,
            endMillis = endMillis,
            allDay = allDay,
            calendarId = resolvedCalendarId
        )
    }

    @JvmStatic
    fun createEvent(
        context: Context,
        title: String,
        description: String?,
        location: String?,
        startMillis: Long,
        endMillis: Long,
        allDay: Boolean,
        calendarId: Long
    ): Long {
        val resolvedCalendarId = if (calendarId > 0L) {
            calendarId
        } else {
            findWritableCalendarId(context)
        }

        val values = ContentValues().apply {
            put(CalendarContract.Events.CALENDAR_ID, resolvedCalendarId)
            put(CalendarContract.Events.TITLE, title)
            put(CalendarContract.Events.DESCRIPTION, description)
            put(CalendarContract.Events.EVENT_LOCATION, location)
            put(CalendarContract.Events.DTSTART, startMillis)
            put(CalendarContract.Events.DTEND, endMillis)
            put(CalendarContract.Events.ALL_DAY, if (allDay) 1 else 0)
            put(CalendarContract.Events.EVENT_TIMEZONE, TimeZone.getDefault().id)
        }
        val uri = context.contentResolver.insert(CalendarContract.Events.CONTENT_URI, values)
        val eventId = uri?.lastPathSegment?.toLongOrNull()
            ?: throw IllegalStateException("Calendar event insert returned null/invalid id")
        require(eventId > 0L) { "Calendar event id must be positive" }
        return eventId
    }

    @JvmStatic
    fun deleteEvent(context: Context, eventId: Long): Boolean {
        val uri = CalendarContract.Events.CONTENT_URI.buildUpon()
            .appendPath(eventId.toString()).build()
        return context.contentResolver.delete(uri, null, null) > 0
    }

    private fun parseIso8601(value: String): Long {
        val trimmed = value.trim()
        require(trimmed.isNotEmpty()) {
            "ISO date must not be blank"
        }

        for (pattern in isoPatterns) {
            val parser = SimpleDateFormat(pattern, Locale.US).apply {
                isLenient = false
                timeZone = TimeZone.getTimeZone("UTC")
            }
            val position = ParsePosition(0)
            val parsed = parser.parse(trimmed, position)
            if (parsed != null && position.index == trimmed.length) {
                return parsed.time
            }
        }

        throw IllegalArgumentException("Invalid ISO date: $value")
    }

    private fun formatIso8601(epochMillis: Long): String {
        val formatter = SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss'Z'", Locale.US)
        formatter.timeZone = TimeZone.getTimeZone("UTC")
        return formatter.format(Date(epochMillis))
    }

    private fun sanitize(value: String?): String {
        if (value.isNullOrEmpty()) {
            return ""
        }
        return value
            .replace('\t', ' ')
            .replace('\n', ' ')
            .replace('\r', ' ')
    }

    private fun encodeEventRow(
        id: String,
        title: String?,
        description: String?,
        location: String?,
        startMillis: Long,
        endMillis: Long,
        allDay: Boolean,
        calendarId: String
    ): String {
        val allDayFlag = if (allDay) "1" else "0"
        return "${sanitize(id)}\t${sanitize(title)}\t${sanitize(description)}\t" +
            "${sanitize(location)}\t${formatIso8601(startMillis)}\t${formatIso8601(endMillis)}\t" +
            "$allDayFlag\t${sanitize(calendarId)}"
    }

    private fun resolveCalendarId(context: Context, rawCalendarId: String?): Long {
        val parsed = rawCalendarId?.trim()?.toLongOrNull()
        if (parsed != null && parsed > 0L) {
            return parsed
        }
        return findWritableCalendarId(context)
    }

    private fun findWritableCalendarId(context: Context): Long {
        val cursor = context.contentResolver.query(
            CalendarContract.Calendars.CONTENT_URI,
            arrayOf(
                CalendarContract.Calendars._ID,
                CalendarContract.Calendars.CALENDAR_ACCESS_LEVEL
            ),
            null,
            null,
            null
        ) ?: throw IllegalStateException("Writable calendar query returned null cursor")

        cursor.use {
            while (it.moveToNext()) {
                if (it.getInt(1) >= CalendarContract.Calendars.CAL_ACCESS_CONTRIBUTOR) {
                    return it.getLong(0)
                }
            }
        }
        throw IllegalStateException("No writable calendar found")
    }
}

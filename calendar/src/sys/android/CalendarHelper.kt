package waterkit.calendar

import android.content.ContentValues
import android.content.Context
import android.provider.CalendarContract
import java.util.TimeZone

object CalendarHelper {
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
        )
        val result = mutableListOf<String>()
        cursor?.use {
            while (it.moveToNext()) {
                val id = it.getString(0)
                val name = it.getString(1) ?: ""
                val color = String.format("#%06X", 0xFFFFFF and it.getInt(2))
                val readOnly = if (it.getInt(3) < CalendarContract.Calendars.CAL_ACCESS_CONTRIBUTOR) "1" else "0"
                result.add("$id\t$name\t$color\t$readOnly")
            }
        }
        return result.toTypedArray()
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
        val values = ContentValues().apply {
            put(CalendarContract.Events.CALENDAR_ID, calendarId)
            put(CalendarContract.Events.TITLE, title)
            put(CalendarContract.Events.DESCRIPTION, description)
            put(CalendarContract.Events.EVENT_LOCATION, location)
            put(CalendarContract.Events.DTSTART, startMillis)
            put(CalendarContract.Events.DTEND, endMillis)
            put(CalendarContract.Events.ALL_DAY, if (allDay) 1 else 0)
            put(CalendarContract.Events.EVENT_TIMEZONE, TimeZone.getDefault().id)
        }
        val uri = context.contentResolver.insert(CalendarContract.Events.CONTENT_URI, values)
        return uri?.lastPathSegment?.toLongOrNull() ?: -1
    }

    @JvmStatic
    fun deleteEvent(context: Context, eventId: Long): Boolean {
        val uri = CalendarContract.Events.CONTENT_URI.buildUpon()
            .appendPath(eventId.toString()).build()
        return context.contentResolver.delete(uri, null, null) > 0
    }
}

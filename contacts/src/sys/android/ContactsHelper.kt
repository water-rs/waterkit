package waterkit.contacts

import android.content.ContentResolver
import android.content.Context
import android.provider.ContactsContract

object ContactsHelper {
    @JvmStatic
    fun fetchAll(context: Context): Array<String> {
        val resolver = context.contentResolver
        val contacts = mutableListOf<String>()
        val cursor = resolver.query(
            ContactsContract.Contacts.CONTENT_URI,
            arrayOf(
                ContactsContract.Contacts._ID,
                ContactsContract.Contacts.DISPLAY_NAME_PRIMARY
            ),
            null, null, null
        )
        cursor?.use {
            while (it.moveToNext()) {
                val id = it.getString(0)
                val name = it.getString(1) ?: ""
                contacts.add("$id\t$name\t\t\t\t\t\t")
            }
        }
        return contacts.toTypedArray()
    }

    @JvmStatic
    fun deleteContact(context: Context, contactId: String): String? {
        return try {
            val uri = ContactsContract.Contacts.CONTENT_URI.buildUpon()
                .appendPath(contactId).build()
            context.contentResolver.delete(uri, null, null)
            null
        } catch (e: Exception) {
            e.message
        }
    }
}

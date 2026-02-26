package waterkit.contacts

import android.content.ContentProviderOperation
import android.content.ContentResolver
import android.content.Context
import android.provider.ContactsContract

object ContactsHelper {
    private fun sanitize(value: String?): String {
        return value
            ?.replace('\t', ' ')
            ?.replace('\n', ' ')
            ?.replace('\r', ' ')
            ?.trim()
            .orEmpty()
    }

    private fun sanitizeListEntry(value: String?): String {
        return sanitize(value).replace(',', ' ')
    }

    private fun parseContactData(payload: String): ContactData {
        val parts = payload.split('\t', limit = 8)
        fun field(index: Int): String = if (index < parts.size) parts[index] else ""
        val phones = field(3).split(',').map { it.trim() }.filter { it.isNotEmpty() }
        val emails = field(4).split(',').map { it.trim() }.filter { it.isNotEmpty() }
        return ContactData(
            givenName = field(0),
            familyName = field(1),
            organization = field(2),
            phoneNumbers = phones,
            emailAddresses = emails,
            birthday = field(5),
            note = field(6)
        )
    }

    private fun queryContactIds(
        resolver: ContentResolver,
        selection: String? = null,
        selectionArgs: Array<String>? = null
    ): List<String> {
        val ids = mutableListOf<String>()
        val cursor = resolver.query(
            ContactsContract.Contacts.CONTENT_URI,
            arrayOf(ContactsContract.Contacts._ID),
            selection,
            selectionArgs,
            "${ContactsContract.Contacts.DISPLAY_NAME_PRIMARY} COLLATE NOCASE ASC"
        ) ?: throw IllegalStateException("Contacts query returned null cursor")

        cursor.use {
            while (it.moveToNext()) {
                val id = it.getString(0) ?: throw IllegalStateException("Contact row missing _ID")
                ids.add(id)
            }
        }
        return ids
    }

    private fun serializeContact(resolver: ContentResolver, contactId: String): String? {
        val existsCursor = resolver.query(
            ContactsContract.Contacts.CONTENT_URI,
            arrayOf(ContactsContract.Contacts._ID),
            "${ContactsContract.Contacts._ID} = ?",
            arrayOf(contactId),
            null
        ) ?: throw IllegalStateException("Contact exists query returned null cursor")

        existsCursor.use {
            if (!it.moveToFirst()) {
                return null
            }
        }

        var givenName = ""
        var familyName = ""
        var organization = ""
        var birthday = ""
        var note = ""
        val phoneNumbers = mutableListOf<String>()
        val emailAddresses = mutableListOf<String>()

        val dataCursor = resolver.query(
            ContactsContract.Data.CONTENT_URI,
            arrayOf(
                ContactsContract.Data.MIMETYPE,
                ContactsContract.CommonDataKinds.StructuredName.GIVEN_NAME,
                ContactsContract.CommonDataKinds.StructuredName.FAMILY_NAME,
                ContactsContract.CommonDataKinds.Organization.COMPANY,
                ContactsContract.CommonDataKinds.Phone.NUMBER,
                ContactsContract.CommonDataKinds.Email.ADDRESS,
                ContactsContract.CommonDataKinds.Event.START_DATE,
                ContactsContract.CommonDataKinds.Event.TYPE,
                ContactsContract.CommonDataKinds.Note.NOTE
            ),
            "${ContactsContract.Data.CONTACT_ID} = ?",
            arrayOf(contactId),
            null
        ) ?: throw IllegalStateException("Contact data query returned null cursor")

        dataCursor.use {
            while (it.moveToNext()) {
                val mimeType = it.getString(0) ?: continue
                when (mimeType) {
                    ContactsContract.CommonDataKinds.StructuredName.CONTENT_ITEM_TYPE -> {
                        if (givenName.isEmpty()) {
                            givenName = it.getString(1).orEmpty()
                        }
                        if (familyName.isEmpty()) {
                            familyName = it.getString(2).orEmpty()
                        }
                    }

                    ContactsContract.CommonDataKinds.Organization.CONTENT_ITEM_TYPE -> {
                        if (organization.isEmpty()) {
                            organization = it.getString(3).orEmpty()
                        }
                    }

                    ContactsContract.CommonDataKinds.Phone.CONTENT_ITEM_TYPE -> {
                        val number = it.getString(4)?.trim().orEmpty()
                        if (number.isNotEmpty()) {
                            phoneNumbers.add(number)
                        }
                    }

                    ContactsContract.CommonDataKinds.Email.CONTENT_ITEM_TYPE -> {
                        val address = it.getString(5)?.trim().orEmpty()
                        if (address.isNotEmpty()) {
                            emailAddresses.add(address)
                        }
                    }

                    ContactsContract.CommonDataKinds.Event.CONTENT_ITEM_TYPE -> {
                        val eventType = it.getInt(7)
                        if (
                            eventType == ContactsContract.CommonDataKinds.Event.TYPE_BIRTHDAY &&
                            birthday.isEmpty()
                        ) {
                            birthday = it.getString(6).orEmpty()
                        }
                    }

                    ContactsContract.CommonDataKinds.Note.CONTENT_ITEM_TYPE -> {
                        if (note.isEmpty()) {
                            note = it.getString(8).orEmpty()
                        }
                    }
                }
            }
        }

        val phones = phoneNumbers.joinToString(",") { sanitizeListEntry(it) }
        val emails = emailAddresses.joinToString(",") { sanitizeListEntry(it) }
        return listOf(
            sanitize(contactId),
            sanitize(givenName),
            sanitize(familyName),
            sanitize(organization),
            phones,
            emails,
            sanitize(birthday),
            sanitize(note)
        ).joinToString("\t")
    }

    private fun contactIdForRawId(resolver: ContentResolver, rawContactId: String): String? {
        val cursor = resolver.query(
            ContactsContract.RawContacts.CONTENT_URI,
            arrayOf(ContactsContract.RawContacts.CONTACT_ID),
            "${ContactsContract.RawContacts._ID} = ?",
            arrayOf(rawContactId),
            null
        ) ?: throw IllegalStateException("RawContacts lookup returned null cursor")

        cursor.use {
            if (!it.moveToFirst()) {
                return null
            }
            return it.getString(0)
        }
    }

    private fun contactLinesForIds(resolver: ContentResolver, ids: List<String>): Array<String> {
        val contacts = ArrayList<String>(ids.size)
        for (id in ids) {
            val serialized = serializeContact(resolver, id)
                ?: throw IllegalStateException("Contact not found for id: $id")
            contacts.add(serialized)
        }
        return contacts.toTypedArray()
    }

    private fun buildInsertOperations(data: ContactData): ArrayList<ContentProviderOperation> {
        val ops = ArrayList<ContentProviderOperation>()
        ops.add(
            ContentProviderOperation.newInsert(ContactsContract.RawContacts.CONTENT_URI)
                .withValue(ContactsContract.RawContacts.ACCOUNT_TYPE, null)
                .withValue(ContactsContract.RawContacts.ACCOUNT_NAME, null)
                .build()
        )

        if (data.givenName.isNotEmpty() || data.familyName.isNotEmpty()) {
            ops.add(
                ContentProviderOperation.newInsert(ContactsContract.Data.CONTENT_URI)
                    .withValueBackReference(ContactsContract.Data.RAW_CONTACT_ID, 0)
                    .withValue(
                        ContactsContract.Data.MIMETYPE,
                        ContactsContract.CommonDataKinds.StructuredName.CONTENT_ITEM_TYPE
                    )
                    .withValue(
                        ContactsContract.CommonDataKinds.StructuredName.GIVEN_NAME,
                        data.givenName
                    )
                    .withValue(
                        ContactsContract.CommonDataKinds.StructuredName.FAMILY_NAME,
                        data.familyName
                    )
                    .build()
            )
        }

        if (data.organization.isNotEmpty()) {
            ops.add(
                ContentProviderOperation.newInsert(ContactsContract.Data.CONTENT_URI)
                    .withValueBackReference(ContactsContract.Data.RAW_CONTACT_ID, 0)
                    .withValue(
                        ContactsContract.Data.MIMETYPE,
                        ContactsContract.CommonDataKinds.Organization.CONTENT_ITEM_TYPE
                    )
                    .withValue(
                        ContactsContract.CommonDataKinds.Organization.COMPANY,
                        data.organization
                    )
                    .build()
            )
        }

        for (number in data.phoneNumbers) {
            ops.add(
                ContentProviderOperation.newInsert(ContactsContract.Data.CONTENT_URI)
                    .withValueBackReference(ContactsContract.Data.RAW_CONTACT_ID, 0)
                    .withValue(
                        ContactsContract.Data.MIMETYPE,
                        ContactsContract.CommonDataKinds.Phone.CONTENT_ITEM_TYPE
                    )
                    .withValue(ContactsContract.CommonDataKinds.Phone.NUMBER, number)
                    .withValue(
                        ContactsContract.CommonDataKinds.Phone.TYPE,
                        ContactsContract.CommonDataKinds.Phone.TYPE_MOBILE
                    )
                    .build()
            )
        }

        for (address in data.emailAddresses) {
            ops.add(
                ContentProviderOperation.newInsert(ContactsContract.Data.CONTENT_URI)
                    .withValueBackReference(ContactsContract.Data.RAW_CONTACT_ID, 0)
                    .withValue(
                        ContactsContract.Data.MIMETYPE,
                        ContactsContract.CommonDataKinds.Email.CONTENT_ITEM_TYPE
                    )
                    .withValue(ContactsContract.CommonDataKinds.Email.ADDRESS, address)
                    .withValue(
                        ContactsContract.CommonDataKinds.Email.TYPE,
                        ContactsContract.CommonDataKinds.Email.TYPE_OTHER
                    )
                    .build()
            )
        }

        if (data.birthday.isNotEmpty()) {
            ops.add(
                ContentProviderOperation.newInsert(ContactsContract.Data.CONTENT_URI)
                    .withValueBackReference(ContactsContract.Data.RAW_CONTACT_ID, 0)
                    .withValue(
                        ContactsContract.Data.MIMETYPE,
                        ContactsContract.CommonDataKinds.Event.CONTENT_ITEM_TYPE
                    )
                    .withValue(
                        ContactsContract.CommonDataKinds.Event.TYPE,
                        ContactsContract.CommonDataKinds.Event.TYPE_BIRTHDAY
                    )
                    .withValue(
                        ContactsContract.CommonDataKinds.Event.START_DATE,
                        data.birthday
                    )
                    .build()
            )
        }

        if (data.note.isNotEmpty()) {
            ops.add(
                ContentProviderOperation.newInsert(ContactsContract.Data.CONTENT_URI)
                    .withValueBackReference(ContactsContract.Data.RAW_CONTACT_ID, 0)
                    .withValue(
                        ContactsContract.Data.MIMETYPE,
                        ContactsContract.CommonDataKinds.Note.CONTENT_ITEM_TYPE
                    )
                    .withValue(ContactsContract.CommonDataKinds.Note.NOTE, data.note)
                    .build()
            )
        }

        return ops
    }

    data class ContactData(
        val givenName: String,
        val familyName: String,
        val organization: String,
        val phoneNumbers: List<String>,
        val emailAddresses: List<String>,
        val birthday: String,
        val note: String
    )

    @JvmStatic
    fun fetchAll(context: Context): Array<String> {
        val resolver = context.contentResolver
        val ids = queryContactIds(resolver)
        return contactLinesForIds(resolver, ids)
    }

    @JvmStatic
    fun search(context: Context, query: String): Array<String> {
        if (query.isBlank()) {
            return fetchAll(context)
        }

        val resolver = context.contentResolver
        val likeArg = "%${query.trim()}%"
        val selection =
            "${ContactsContract.Contacts.DISPLAY_NAME_PRIMARY} LIKE ? OR " +
                "${ContactsContract.Contacts.DISPLAY_NAME_ALTERNATIVE} LIKE ?"
        val ids = queryContactIds(resolver, selection, arrayOf(likeArg, likeArg))
        return contactLinesForIds(resolver, ids)
    }

    @JvmStatic
    fun getContact(context: Context, contactId: String): String? {
        return serializeContact(context.contentResolver, contactId)
    }

    @JvmStatic
    fun createContact(context: Context, payload: String): String? {
        val resolver = context.contentResolver
        val data = parseContactData(payload)
        val operations = buildInsertOperations(data)
        val results = resolver.applyBatch(ContactsContract.AUTHORITY, operations)
        val rawContactId = results.firstOrNull()?.uri?.lastPathSegment
            ?: throw IllegalStateException("Failed to resolve raw contact id from insert result")

        val contactId = contactIdForRawId(resolver, rawContactId)
            ?: throw IllegalStateException("Failed to resolve contact id for raw contact $rawContactId")
        return serializeContact(resolver, contactId)
    }

    @JvmStatic
    fun deleteContact(context: Context, contactId: String): Boolean {
        val deletedRows = context.contentResolver.delete(
            ContactsContract.RawContacts.CONTENT_URI,
            "${ContactsContract.RawContacts.CONTACT_ID} = ?",
            arrayOf(contactId)
        )
        return deletedRows > 0
    }
}

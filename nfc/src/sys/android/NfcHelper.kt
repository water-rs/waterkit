package waterkit.nfc

import android.content.Context
import android.nfc.NfcAdapter
import android.nfc.NdefMessage
import android.nfc.NdefRecord
import android.nfc.Tag
import android.nfc.tech.Ndef
import android.nfc.tech.NdefFormatable

object NfcHelper {
    @JvmStatic
    fun isAvailable(context: Context): Boolean {
        val adapter = NfcAdapter.getDefaultAdapter(context)
        return adapter != null && adapter.isEnabled
    }

    @JvmStatic
    fun readTag(tag: Tag): String? {
        val ndef = Ndef.get(tag) ?: return null
        return try {
            ndef.connect()
            val message = ndef.ndefMessage ?: return null
            val records = message.records.joinToString(";") { record ->
                val tnf = record.tnf
                val type = record.type.joinToString("") { String.format("%02x", it) }
                val payload = record.payload.joinToString("") { String.format("%02x", it) }
                "$tnf:$type:$payload"
            }
            records
        } catch (e: Exception) {
            null
        } finally {
            try { ndef.close() } catch (_: Exception) {}
        }
    }

    @JvmStatic
    fun writeTag(tag: Tag, recordsJson: String): String? {
        val records = parseRecords(recordsJson)
        val message = NdefMessage(records.toTypedArray())
        val ndef = Ndef.get(tag)
        if (ndef != null) {
            return try {
                ndef.connect()
                if (!ndef.isWritable) return "Tag is read-only"
                if (ndef.maxSize < message.toByteArray().size) return "Message too large for tag"
                ndef.writeNdefMessage(message)
                null
            } catch (e: Exception) {
                e.message ?: "Write failed"
            } finally {
                try { ndef.close() } catch (_: Exception) {}
            }
        }
        val formatable = NdefFormatable.get(tag) ?: return "Tag does not support NDEF"
        return try {
            formatable.connect()
            formatable.format(message)
            null
        } catch (e: Exception) {
            e.message ?: "Format failed"
        } finally {
            try { formatable.close() } catch (_: Exception) {}
        }
    }

    @JvmStatic
    fun getTagId(tag: Tag): String {
        return tag.id.joinToString("") { String.format("%02x", it) }
    }

    private fun parseRecords(json: String): List<NdefRecord> {
        return json.split(";").filter { it.isNotEmpty() }.mapNotNull { recStr ->
            val parts = recStr.split(":", limit = 3)
            if (parts.size != 3) return@mapNotNull null
            val tnf = parts[0].toShortOrNull() ?: return@mapNotNull null
            val type = hexDecode(parts[1])
            val payload = hexDecode(parts[2])
            NdefRecord(tnf, type, ByteArray(0), payload)
        }
    }

    private fun hexDecode(hex: String): ByteArray {
        return hex.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
    }
}

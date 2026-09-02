package waterkit.nfc

import android.content.Context
import android.nfc.NfcAdapter
import android.nfc.NdefMessage
import android.nfc.NdefRecord
import android.nfc.Tag
import android.nfc.tech.Ndef
import android.nfc.tech.NdefFormatable
import android.nfc.tech.IsoDep
import android.nfc.tech.MifareClassic
import android.nfc.tech.NfcA
import android.nfc.tech.NfcF
import android.nfc.tech.NfcV

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

    @JvmStatic
    fun getTagType(tag: Tag): Int {
        val techs = tag.techList.toSet()
        return when {
            techs.contains(MifareClassic::class.java.name) -> 5
            techs.contains(IsoDep::class.java.name) -> 3
            techs.contains(NfcV::class.java.name) -> 4
            techs.contains(NfcF::class.java.name) -> 2
            techs.contains(NfcA::class.java.name) -> 1
            else -> 6
        }
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

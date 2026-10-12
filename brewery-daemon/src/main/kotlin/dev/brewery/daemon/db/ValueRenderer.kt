package dev.brewery.daemon.db

import java.sql.Blob
import java.sql.Clob
import java.sql.ResultSet
import java.sql.SQLException
import java.sql.Types
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.time.OffsetDateTime
import java.time.OffsetTime
import java.time.ZonedDateTime
import java.time.format.DateTimeFormatter

/** Turns JDBC values into the strings of `db/fetch`: null stays null, binary is hex, text is capped, time is ISO-8601. */
object ValueRenderer {
    const val MAX_TEXT_CHARS = 10_000
    const val MAX_BINARY_BYTES = 64
    private const val ELLIPSIS = "…"
    private val HEX = "0123456789abcdef".toCharArray()

    /** Reads column [index] (1-based) of the current row of [rs] and renders it. */
    fun read(rs: ResultSet, index: Int): String? {
        val meta = rs.metaData
        val type = meta.getColumnType(index)
        val typeName = meta.getColumnTypeName(index)?.lowercase() ?: ""
        val value: Any? = try {
            when {
                type == Types.TIMESTAMP_WITH_TIMEZONE || typeName == "timestamptz" -> rs.getObject(index, OffsetDateTime::class.java)
                // SQLite hands back the raw stored number from getObject for a TIMESTAMP column; the typed getters convert.
                type == Types.TIMESTAMP || typeName == "timestamp" || typeName == "datetime" -> rs.getTimestamp(index)
                type == Types.DATE || typeName == "date" -> rs.getDate(index)
                type == Types.TIME || typeName == "time" -> rs.getTime(index)
                else -> rs.getObject(index)
            }
        } catch (_: SQLException) {
            // Values the driver cannot map (odd date formats in SQLite, say) are shown as the raw text.
            rs.getString(index)
        } catch (_: RuntimeException) {
            rs.getString(index)
        }
        return when (value) {
            // Only the head of a LOB is needed: do not pull gigabytes over the wire.
            is Blob -> render(value.getBytes(1, MAX_BINARY_BYTES + 1).also { runCatching { value.free() } })
            is Clob -> render(value.getSubString(1, MAX_TEXT_CHARS + 1).also { runCatching { value.free() } })
            else -> render(value)
        }
    }

    fun render(value: Any?): String? = when (value) {
        null -> null
        is String -> text(value)
        is ByteArray -> hex(value)
        is java.sql.Timestamp -> value.toLocalDateTime().format(DateTimeFormatter.ISO_LOCAL_DATE_TIME)
        is java.sql.Date -> value.toLocalDate().format(DateTimeFormatter.ISO_LOCAL_DATE)
        is java.sql.Time -> value.toLocalTime().format(DateTimeFormatter.ISO_LOCAL_TIME)
        is LocalDateTime -> value.format(DateTimeFormatter.ISO_LOCAL_DATE_TIME)
        is LocalDate -> value.format(DateTimeFormatter.ISO_LOCAL_DATE)
        is LocalTime -> value.format(DateTimeFormatter.ISO_LOCAL_TIME)
        is OffsetDateTime -> value.format(DateTimeFormatter.ISO_OFFSET_DATE_TIME)
        is OffsetTime -> value.format(DateTimeFormatter.ISO_OFFSET_TIME)
        is ZonedDateTime -> value.format(DateTimeFormatter.ISO_OFFSET_DATE_TIME)
        is java.math.BigDecimal -> text(value.toPlainString())
        is java.sql.Array -> render(runCatching { value.array }.getOrNull() ?: value.toString())
        is Array<*> -> value.joinToString(prefix = "[", postfix = "]") { render(it) ?: "null" }.let(::text)
        is Iterable<*> -> value.joinToString(prefix = "[", postfix = "]") { render(it) ?: "null" }.let(::text)
        else -> text(value.toString())
    }

    private fun text(s: String): String = if (s.length > MAX_TEXT_CHARS) s.substring(0, MAX_TEXT_CHARS) + ELLIPSIS else s

    private fun hex(bytes: ByteArray): String {
        val n = minOf(bytes.size, MAX_BINARY_BYTES)
        val sb = StringBuilder(2 + n * 2 + 1).append("0x")
        for (i in 0 until n) {
            val b = bytes[i].toInt() and 0xFF
            sb.append(HEX[b ushr 4]).append(HEX[b and 0x0F])
        }
        if (bytes.size > n) sb.append(ELLIPSIS)
        return sb.toString()
    }
}

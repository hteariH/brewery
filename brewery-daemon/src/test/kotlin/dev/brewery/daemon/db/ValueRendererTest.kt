package dev.brewery.daemon.db

import java.math.BigDecimal
import java.sql.Timestamp
import java.time.LocalDate
import java.time.LocalTime
import java.time.OffsetDateTime
import java.time.ZoneOffset
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ValueRendererTest {
    @Test
    fun `null stays null and scalars use their plain text`() {
        assertNull(ValueRenderer.render(null))
        assertEquals("42", ValueRenderer.render(42))
        assertEquals("true", ValueRenderer.render(true))
        assertEquals("1000", ValueRenderer.render(BigDecimal("1E+3")))
        assertEquals("", ValueRenderer.render(""))
    }

    @Test
    fun `binary is hex with a 0x prefix limited to 64 bytes`() {
        assertEquals("0x010203", ValueRenderer.render(byteArrayOf(1, 2, 3)))
        assertEquals("0x", ValueRenderer.render(ByteArray(0)))
        val exactly = ValueRenderer.render(ByteArray(64) { 0xAB.toByte() })!!
        assertEquals(2 + 128, exactly.length)
        val longer = ValueRenderer.render(ByteArray(100) { 0xAB.toByte() })!!
        assertEquals("0x" + "ab".repeat(64) + "…", longer)
    }

    @Test
    fun `long text is cut at 10000 characters with an ellipsis`() {
        assertEquals("a".repeat(10_000), ValueRenderer.render("a".repeat(10_000)))
        assertEquals("a".repeat(10_000) + "…", ValueRenderer.render("a".repeat(10_001)))
    }

    @Test
    fun `temporal values are ISO-8601`() {
        assertEquals("2024-03-05T10:20:30", ValueRenderer.render(Timestamp.valueOf("2024-03-05 10:20:30")))
        assertEquals("2024-03-05T10:20:30.123", ValueRenderer.render(Timestamp.valueOf("2024-03-05 10:20:30.123")))
        assertEquals("2024-03-05", ValueRenderer.render(java.sql.Date.valueOf("2024-03-05")))
        assertEquals("2024-03-05", ValueRenderer.render(LocalDate.of(2024, 3, 5)))
        assertEquals("10:20:30", ValueRenderer.render(java.sql.Time.valueOf("10:20:30")))
        assertEquals("10:20:00", ValueRenderer.render(LocalTime.of(10, 20)))
        assertEquals(
            "2024-03-05T10:20:30+02:00",
            ValueRenderer.render(OffsetDateTime.of(2024, 3, 5, 10, 20, 30, 0, ZoneOffset.ofHours(2))),
        )
    }

    @Test
    fun `arrays render as a bracketed list`() {
        assertEquals("[1, 2, null]", ValueRenderer.render(arrayOf<Any?>(1, 2, null)))
        assertTrue(ValueRenderer.render(listOf("x"))!!.contains("x"))
    }

    @Test
    fun `time keeps its fractional seconds`() {
        assertEquals("10:20:30.123", ValueRenderer.render(java.sql.Time(java.sql.Time.valueOf("10:20:30").time + 123)))
    }

    @Test
    fun `truncation does not split a surrogate pair`() {
        val text = "a".repeat(9_999) + "😀" + "tail"
        assertEquals("a".repeat(9_999) + "…", ValueRenderer.render(text))
    }

    @Test
    fun `primitive arrays render their elements`() {
        assertEquals("[1, 2]", ValueRenderer.render(intArrayOf(1, 2)))
        assertEquals("[1.5]", ValueRenderer.render(doubleArrayOf(1.5)))
    }
}

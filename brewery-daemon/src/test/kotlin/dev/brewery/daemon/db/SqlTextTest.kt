package dev.brewery.daemon.db

import dev.brewery.daemon.db.SqlText.Kind
import kotlin.test.Test
import kotlin.test.assertEquals

class SqlTextTest {
    private fun kind(sql: String) = SqlText.classify(sql)

    @Test
    fun `plain reads`() {
        for (sql in listOf(
            "select 1", "  SELECT * FROM t", "-- hi\nselect 1", "/* c */ with x as (select 1) select * from x",
            "(select 1) union (select 2)", "VALUES (1)", "table t", "show server_version", "explain select 1",
            "SELECT updated_at, deleted FROM t",
        )) assertEquals(Kind.READ, kind(sql), sql)
    }

    @Test
    fun `reads that write or lock are not plain`() {
        for (sql in listOf(
            "select * from t for update", "SELECT * FROM t FOR NO KEY UPDATE", "select * from t for share",
            "select * from t for key share", "insert into t values (1) returning id", "update t set a = 1 returning a",
            "with d as (delete from t returning *) select * from d", "select * into copy from t", "explain analyze update t set a = 1",
        )) assertEquals(Kind.OTHER, kind(sql), sql)
    }

    @Test
    fun `transaction control`() {
        assertEquals(Kind.TX_BEGIN, kind("BEGIN"))
        assertEquals(Kind.TX_BEGIN, kind("  /* x */ start transaction isolation level serializable"))
        for (sql in listOf("COMMIT", "commit;", "END", "ROLLBACK", "abort")) assertEquals(Kind.TX_END, kind(sql), sql)
        for (sql in listOf("ROLLBACK TO SAVEPOINT s", "COMMIT PREPARED 'x'", "ROLLBACK PREPARED 'x'", "SAVEPOINT s", "VACUUM", "UPDATE t SET a = 1")) {
            assertEquals(Kind.OTHER, kind(sql), sql)
        }
    }
}

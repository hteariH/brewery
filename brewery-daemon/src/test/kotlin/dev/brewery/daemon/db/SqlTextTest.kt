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

    @Test
    fun `chained and prepared transaction ends`() {
        for (sql in listOf("COMMIT AND CHAIN", "rollback and chain;", "COMMIT WORK AND CHAIN", "END TRANSACTION AND CHAIN")) {
            assertEquals(Kind.TX_BEGIN, kind(sql), sql)
        }
        for (sql in listOf("COMMIT AND NO CHAIN", "ROLLBACK WORK AND NO CHAIN", "PREPARE TRANSACTION 'x'")) {
            assertEquals(Kind.TX_END, kind(sql), sql)
        }
    }

    @Test
    fun `a script of several statements is never a plain read`() {
        for (sql in listOf("SELECT 1; BEGIN", "select 1; insert into t values (1)", "SELECT 1;\nSELECT 2")) {
            assertEquals(Kind.OTHER, kind(sql), sql)
        }
        for (sql in listOf("SELECT 1;", "select ';' ; -- trailing", "select 1 /* ; */")) assertEquals(Kind.READ, kind(sql), sql)
    }

    @Test
    fun `keywords inside literals identifiers and comments do not matter`() {
        val d = "$"
        val plain = listOf(
            "select * from t where op = 'DELETE'",
            "SELECT * FROM audit_log WHERE op <> 'DELETE'",
            "select 'it''s an update' from t",
            """select "update" from t""",
            """select "returning" from t""",
            "select `into` from t",
            "select 1 -- for update",
            "select 1 /* insert /* nested */ still comment */",
            "select ${d}${d}insert${d}${d}",
            "select ${d}tag${d} delete ${d}${d} ${d}tag${d} from t",
            """select E'it\'s an update' from t""",
            "select ${d}1 from t where a = ${d}2",
            "select 'x into y'",
            "select 'unterminated for update",
        )
        for (sql in plain) assertEquals(Kind.READ, kind(sql), sql)
        val notPlain = listOf(
            "select 'a' from t for update",
            """select "a" from t where x = 'y' returning z""",
            "select 1 -- c\n into t2",
            "select ${d}${d}x${d}${d} from t for share",
        )
        for (sql in notPlain) assertEquals(Kind.OTHER, kind(sql), sql)
    }
}

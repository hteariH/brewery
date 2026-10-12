package dev.brewery.daemon.db

import java.nio.file.Files
import java.nio.file.Path
import java.util.jar.JarOutputStream
import java.util.zip.ZipEntry
import javax.tools.ToolProvider
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotSame
import kotlin.test.assertTrue

class DriverRegistryTest {
    private fun registry(): DriverRegistry =
        DriverRegistry(ConnectionStore(Files.createTempDirectory("brewery-db-drv")))

    /** Compiles a trivial java.sql.Driver into a jar; it accepts only urls starting with `jdbc:fake:`. */
    private fun buildDriverJar(): Path {
        val dir = Files.createTempDirectory("brewery-fake-driver")
        val src = dir.resolve("fake/FakeDriver.java")
        Files.createDirectories(src.parent)
        Files.writeString(
            src,
            """
            package fake;
            import java.sql.*;
            import java.util.*;
            import java.util.logging.Logger;
            public class FakeDriver implements Driver {
                public Connection connect(String url, Properties info) { return null; }
                public boolean acceptsURL(String url) { return url.startsWith("jdbc:fake:"); }
                public DriverPropertyInfo[] getPropertyInfo(String url, Properties info) { return new DriverPropertyInfo[0]; }
                public int getMajorVersion() { return 1; }
                public int getMinorVersion() { return 0; }
                public boolean jdbcCompliant() { return false; }
                public Logger getParentLogger() { return null; }
            }
            """.trimIndent(),
        )
        val compiler = ToolProvider.getSystemJavaCompiler()
        assertTrue(compiler != null, "tests need a JDK")
        assertEquals(0, compiler.run(null, null, null, "-d", dir.toString(), src.toString()))
        val jar = dir.resolve("fake.jar")
        JarOutputStream(Files.newOutputStream(jar)).use { out ->
            out.putNextEntry(ZipEntry("fake/FakeDriver.class"))
            out.write(Files.readAllBytes(dir.resolve("fake/FakeDriver.class")))
            out.closeEntry()
        }
        return jar
    }

    @Test
    fun `lists the four bundled drivers`() {
        val ids = registry().list().filter { it.bundled }.map { it.id }
        assertEquals(listOf("postgresql", "mysql", "mariadb", "sqlite"), ids)
    }

    @Test
    fun `bundled ids load their driver class`() {
        val registry = registry()
        assertTrue(registry.driver("postgresql").acceptsURL("jdbc:postgresql://h/db"))
        assertTrue(registry.driver("mariadb").acceptsURL("jdbc:mariadb://h/db"))
        assertTrue(registry.driver("mysql").acceptsURL("jdbc:mariadb://h/db"))
        assertTrue(registry.driver("sqlite").acceptsURL("jdbc:sqlite:x.db"))
    }

    @Test
    fun `unknown driver id is reported`() {
        assertEquals(DbErrorCodes.NOT_FOUND, assertFailsWith<DbException> { registry().driver("nope") }.code)
    }

    @Test
    fun `user jar is loaded in its own classloader and registered`() {
        val root = Files.createTempDirectory("brewery-db-drv2")
        val registry = DriverRegistry(ConnectionStore(root))
        val jar = buildDriverJar()
        val info = registry.addJar("fake", "Fake", "fake.FakeDriver", listOf(jar.toString()))
        assertEquals(DriverInfo("fake", "Fake", false, listOf(jar.toString())), info)
        assertTrue(registry.list().any { it.id == "fake" && !it.bundled })

        val driver = registry.driver("fake")
        assertTrue(driver.acceptsURL("jdbc:fake:x"))
        val loader = driver.javaClass.classLoader
        assertNotSame(DriverRegistry::class.java.classLoader, loader)
        assertTrue(loader is java.net.URLClassLoader)

        // survives a restart: a new registry over the same project reads db.json
        val reloaded = DriverRegistry(ConnectionStore(root)).driver("fake")
        assertTrue(reloaded.acceptsURL("jdbc:fake:y"))
    }

    @Test
    fun `addJar rejects missing jar, missing class and bundled ids`() {
        val registry = registry()
        assertFailsWith<DbException> { registry.addJar("x", "X", "a.B", listOf("/does/not/exist.jar")) }
        val jar = buildDriverJar()
        assertFailsWith<DbException> { registry.addJar("x", "X", "fake.Missing", listOf(jar.toString())) }
        assertFailsWith<DbException> { registry.addJar("sqlite", "X", "fake.FakeDriver", listOf(jar.toString())) }
        assertTrue(registry.list().none { it.id == "x" })
    }
}

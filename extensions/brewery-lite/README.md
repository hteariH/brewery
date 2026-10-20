# Brewery Lite

A Zed extension that brings part of [Brewery](https://github.com/hteariH/brewery), an
open-source Zed-based IDE for Java and Spring, to stock Zed:

- **Spring Boot config completion**: a language server (`brewery-spring-config`) for
  `application.yml` and `application.properties` that completes Spring Boot
  configuration keys.
- **An MCP server for Java/Spring projects and databases**: a context server (`brewery`)
  that gives Zed's agent the project model, Spring beans and endpoints, run configurations,
  and read-only access to the project's database.
- **Brewery's theme and icons**: the IntelliJ-style "Brewery" theme (dark and light) and the
  "Brewery Icons" icon theme.

Brewery Lite is for stock Zed. The Brewery IDE has all of this built in, under the same server
and theme names, so don't install Brewery Lite there.

The language server only starts in Maven and Gradle projects: folders with a `pom.xml`,
`build.gradle`, `build.gradle.kts`, `settings.gradle` or `settings.gradle.kts` at the root. In
any other folder Zed shows "Failed to run brewery-spring-config" once when you open a YAML or
properties file; that is expected, and nothing is downloaded or started there.

Completion and hover come from the configuration metadata in your project's dependency jars
(`spring-boot-autoconfigure` and friends). The language server resolves them through Gradle
or Maven when you open the first configuration file, which can take a while on a large build;
a project without Spring Boot on its classpath gets no completion. The MCP server reads the
project from its working directory, the project root; the database tools use the
`spring.datasource.url` (with username and password) in
`src/main/resources/application.{properties,yml}` and report an error when there is none.
`${NAME}` placeholders in those values resolve from the environment Zed itself runs in, not
from `.env` files or direnv.

Both servers are the Brewery daemon, a Kotlin program that runs on Java. The extension
downloads the daemon jar of its own version from the Brewery GitHub release and checks it
against the release's `SHA256SUMS` before running it.

## Requirements

- Java 21 or newer. The extension picks the first of:
  1. the `java_path` setting (used as is, see below);
  2. for the language server, `$JAVA_HOME/bin/java` of your project's shell environment, then
     `java` on that shell's `PATH`, each only when it is Java 21 or newer (on Windows a
     `JAVA_HOME` without `bin\java.exe` may still pass this check through another Java on the
     system; set `java_path` if that picks the wrong one);
  3. `java` on Zed's own `PATH`, when it is Java 21 or newer;
  4. a Java 21 runtime (Eclipse Temurin JRE from api.adoptium.net), downloaded once into the
     extension's work directory.
- `application.properties` support needs an extension that provides the "Properties" language
  (for example the Java extension); `application.yml` works in stock Zed.

## Settings

Both servers take a `java_path` setting, in Zed's `settings.json`:

```json
{
  "lsp": { "brewery-spring-config": { "settings": { "java_path": "/path/to/jdk-21/bin/java" } } },
  "context_servers": { "brewery": { "settings": { "java_path": "/path/to/jdk-21/bin/java" } } }
}
```

## MCP tools

| Tool | What it returns |
| --- | --- |
| `project_model` | The build tool (Gradle or Maven), the modules and their tasks |
| `spring_beans` | Spring beans declared in the sources: name, type, stereotype, file and line |
| `spring_endpoints` | HTTP endpoints of the Spring controllers: method, path, handler, file and line |
| `run_configurations` | The command that starts each Spring Boot application, with working directory and main class |
| `db_schema` | Tables and views of the Spring datasource (`spring.datasource.url`), or the columns of one table |
| `db_query` | The result of a read-only SQL query |

`db_query` supports PostgreSQL, MySQL/MariaDB and SQLite datasources. It guards against writes
in two layers:

1. A statement filter: only a single statement that reads (SELECT, WITH, VALUES, TABLE, SHOW,
   EXPLAIN) is accepted; DML, DDL, `SELECT ... INTO`, data-modifying `WITH` and multiple
   statements are rejected before they reach the database.
2. The database session: the statement runs on a read-only connection (a READ ONLY transaction
   on PostgreSQL and MySQL/MariaDB, a read-only file on SQLite) inside a transaction that is
   always rolled back. This catches writes the filter cannot see, such as `nextval()` or a
   function that writes.

It returns at most 200 rows and times out after 30 seconds.

Limitation: a SELECT can still call functions that act outside the transaction, and neither
layer blocks them. On PostgreSQL that includes `pg_terminate_backend`, `pg_cancel_backend`
and, for superusers, large-object and server-file functions. Point `spring.datasource` at a
least-privilege database user (or use a separate read-only user for local development) when
the agent should not be able to do that.

## The full IDE

Brewery Lite is a subset. Brewery itself adds Java navigation and refactoring, run and
debug configurations, a database tool window, Spring tooling and more:
<https://github.com/hteariH/brewery>.

## Licence

Apache License 2.0, see [LICENSE-APACHE](LICENSE-APACHE). The icons come from
JetBrains/intellij-community (Apache 2.0), see [icons/NOTICE](icons/NOTICE).

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

Both servers are the Brewery daemon, a Kotlin program that runs on Java. The extension
downloads the daemon jar of its own version from the Brewery GitHub release and checks it
against the release's `SHA256SUMS` before running it.

## Requirements

- Java 21 or newer. The extension picks the first of:
  1. the `java_path` setting (used as is, see below);
  2. for the language server, `$JAVA_HOME/bin/java` of your project's shell environment, then
     `java` on that shell's `PATH`, each only when it is Java 21 or newer;
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

`db_query` is read-only: it accepts a single statement that only reads data (anything
else is rejected before it reaches the database), runs it on a read-only connection inside
a transaction that is always rolled back, returns at most 200 rows, and times out after
30 seconds.

## The full IDE

Brewery Lite is a subset. Brewery itself adds Java navigation and refactoring, run and
debug configurations, a database tool window, Spring tooling and more:
<https://github.com/hteariH/brewery>.

## Licence

Apache License 2.0, see [LICENSE-APACHE](LICENSE-APACHE). The icons come from
JetBrains/intellij-community (Apache 2.0), see [icons/NOTICE](icons/NOTICE).

# Changelog

The text under `## <version>` becomes the body of that version's GitHub Release (see
"Releasing" in docs/brewery/FORK.md). A version without a section gets a generic install
note. Draft:

## 0.1.0

First public release of Brewery, an open-source IDE for Java and Spring built on the Zed editor.

- Java: project import for Maven and Gradle, code intelligence through jdtls, run and debug, JUnit tests, and a Build tool window.
- Spring: run widget with Spring Boot configurations, `application.yml`/`.properties` completion and inspections, and an endpoints and beans view.
- Databases: connect to PostgreSQL, MySQL and SQLite, browse schemas, run queries and edit table data.
- Git: a commit tool window, branches, history and merge conflict resolution in the IntelliJ style.
- Everything runs offline: the installer bundles a JRE, jdtls and the Brewery daemon.
- Unsigned builds for Windows (installer), Linux (tarball, deb, rpm, AUR) and macOS (experimental zip). Windows installs update themselves; Linux and macOS show a notification.

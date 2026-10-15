# Changelog

The text under `## <version>` is the top of that version's GitHub Release (install notes and checksums are added below it; see
"Releasing" in docs/brewery/FORK.md). A version without a section gets a generic intro in its place.
Draft (drop this note when 0.1.0 ships):

## 0.1.0

First public release of Brewery, an open-source IDE for Java and Spring built on the Zed editor.

- Java: project import for Maven and Gradle, code intelligence through jdtls, run and debug, JUnit tests from the gutter, and a Build tool window.
- Spring: run widget with Spring Boot configurations, `application.yml`/`.properties` completion and inspections, and an endpoints and beans view.
- Databases: connect to PostgreSQL, MySQL and SQLite, browse schemas, run queries and page through results.
- Git: a commit tool window, branches, history and merge conflict resolution in the IntelliJ style.
- Everything runs offline: the installer bundles a JRE, jdtls and the Brewery daemon.
- Unsigned builds for Windows (installer), Linux (tarball, deb, rpm, and a PKGBUILD for the AUR) and macOS (experimental zip). Windows can update itself from GitHub Releases; Linux and macOS show a notification.

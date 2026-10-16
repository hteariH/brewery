# Changelog

The text under `## <version>` is the top of that version's GitHub Release (install notes and checksums are added below it; see
"Releasing" in docs/brewery/FORK.md). A version without a section gets a generic intro in its place.

## 0.1.2

- Java: Gradle projects import again after a Brewery update. jdtls kept an old path to its Gradle init script ("The specified initialization script ... init.gradle does not exist"); the bundled jdtls now starts with a fresh workspace whenever it changes, so each project is imported once after an update.
- Git: in the Branches popup, the arrow next to a branch opens the branch's menu instead of checking the branch out; right-click opens the same menu.
- The Database, Services and Gradle/Maven tool windows can be moved to the other side, and remember where they are.

## 0.1.1

- Project Diagnostics (Alt+6) now lists problems that belong to a whole project or folder, such as jdtls's "The project cannot be built until build path errors are resolved". Before, the error count showed 1 but the Diagnostics tab was empty.

## 0.1.0

First public release of Brewery, an open-source IDE for Java and Spring built on the Zed editor.

- Java: project import for Maven and Gradle, code intelligence through jdtls, run and debug, JUnit tests from the gutter, and a Build tool window.
- Spring: run widget with Spring Boot configurations, `application.yml`/`.properties` completion and inspections, and an endpoints and beans view.
- Databases: connect to PostgreSQL, MySQL/MariaDB, SQLite or any JDBC driver, browse schemas, run queries with SQL completion and page through results.
- Git: a Commit tool window, a Branches popup, a Log with filters, file history, compare with a branch or revision, and a 3-way merge editor, in the IntelliJ style.
- Java tools work offline: packages bundle a Java runtime, jdtls, Lombok, the debugger and the Java extension. Your project's own JDK and Maven/Gradle dependencies still download the first time.
- Unsigned builds for Windows (installer), Linux (tarball, deb, rpm, and a PKGBUILD for the AUR) and macOS (experimental zip). Windows can update itself from GitHub Releases; Linux and macOS show a notification.

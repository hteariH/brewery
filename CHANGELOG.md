# Changelog

The text under `## <version>` is the top of that version's GitHub Release (install notes and checksums are added below it; see
"Releasing" in docs/brewery/FORK.md). A version without a section gets a generic intro in its place.

## 0.1.3

Bug fixes from the first review round.

- Databases:
  - a password typed into the URL or driver options goes to the OS secret store, never into `.brewery/db.json`;
  - PostgreSQL transactions stay intact after `COMMIT AND CHAIN` and while the schema tree is expanded;
  - expanding a MariaDB/MySQL schema no longer ends results that are still being read;
  - abandoned results release their locks after a minute;
  - very large results inside a transaction are capped at 100 000 rows instead of exhausting memory;
  - SQLite reports 0 rows for statements that change nothing;
  - switching projects no longer freezes the daemon.
- SQL completion replaces the rest of the word, qualifies MySQL/MariaDB tables when the URL names no database, and recovers after a daemon restart or an abandoned DDL run.
- Git:
  - the merge editor no longer writes through symlinks;
  - merge and rebase steps time out instead of hanging on a stuck hook;
  - the Log's text filter falls back to a message search for hash-like text;
  - Show Commit in Log clears a filter that hides the commit;
  - path filters are relative to the project folder and taken literally;
  - Compare picks the branch over a same-named tag and says it is relative to the merge base;
  - the Log tool window can move to another side.
- Java: Gradle runs on your JDK (`JAVA_HOME`) when the wrapper supports it, so projects on Gradle older than 8.5 import; jdtls follows the `min_memory`/`max_memory` settings.
- Updates: Preview builds only notify, never install the Stable release; requests identify as Brewery; a misleading palette entry is gone.
- Packages: Linux and macOS packages include the font licences; the README lists the distributions the packages really support.

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

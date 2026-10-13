# Brewery

Brewery is an open-source Java IDE for Windows and Linux with Spring and
database tools, built on a fast, natively compiled, GPU-rendered editor.

> Inspired by the IntelliJ IDEA interface. Not affiliated with or endorsed by
> JetBrains.

Brewery is a fork of [Zed](https://github.com/zed-industries/zed). Zed's
collaboration, account, AI and telemetry features are switched off. Brewery still contacts Zed's extension gallery: it installs Zed's default
extensions on first start, checks installed extensions for updates, and serves
the extensions you browse or install. It also downloads Zed's remote server when
you open a project over SSH.

**Status:** early development (milestone M0 — the fork itself). Not usable for
Java work yet. See the [design](docs/superpowers/specs/2026-10-10-brewery-design.md).

## Building

Follow Zed's build guides — [Windows](docs/src/development/windows.md),
[Linux](docs/src/development/linux.md) — then:

    cargo run -p zed

Brewery's Gradle/Maven tool window and build model come from a Kotlin daemon.
Build it before running the app (needs JDK 21):

    cd brewery-daemon && ./gradlew shadowJar

The Java extension is installed automatically on first start. Its language
server (jdtls) needs JDK 21 or newer on the machine, or it is downloaded.

Development builds also need the CLI helper next to the app binary, or the
app prints `could not find zed-cli` on start (harmless, but git over SSH
cannot ask for passwords):

    cargo build -p cli -p zed

In debug builds, extensions run unoptimised, so large extension downloads (for
example jdtls, ~50 MB, fetched by the Java extension) can crawl for minutes
and time out. Release builds download it in seconds. For Java work, build with
`cargo build --profile release-fast -p cli -p zed`, or retry with
"editor: restart language server".

## Spring support

The Services tool window (bottom of the left stripe) lists the Spring Boot
applications in your project, their HTTP endpoints and the beans found in the
sources. Click an item to jump to its code. Click ▶ on an application to run it
with Gradle `bootRun` or Maven `spring-boot:run`.

Maven multi-module apps need their sibling modules installed first, so run
`mvn install` at the root before starting one of them.

The scan reads the source text only, with no classpath or symbol resolution, so
it has limits:

- Records and enums are not scanned.
- Spring Data repositories that are not annotated with `@Repository` are not found.
- A mapping path written as a constant shows as the class-level prefix only.
- Controllers with the same simple name are grouped together in the tree.
- A custom Gradle `projectDir` is not supported; modules are looked up under the
  default layout.
- Maven parent poms outside the project are not followed when looking for the
  Spring Boot plugin.

In `application*.properties` and `application*.yml`/`.yaml` files, Brewery
completes Spring Boot configuration keys and boolean and hint values, shows
descriptions on hover, and warns about unknown and deprecated keys. It uses the
`spring-configuration-metadata.json` files of the project's dependencies: Gradle
projects are read through the Tooling API, and Maven projects through
`dependency:build-classpath`. The Brewery daemon needs a JDK 21 or newer.

## Databases

The Database tool window (left stripe) lists the project's data sources. Use `+`
to add one: PostgreSQL, MySQL, MariaDB and SQLite drivers are bundled, and any
other database works through a JDBC driver jar you add in the same dialog. Test
Connection checks the settings before you save them. Expand a data source to
browse schemas, tables, views, columns, indexes and foreign keys.

New Query Console opens a SQL editor for the selected data source. Ctrl+Enter
runs the selected text or the statement under the cursor, and Ctrl+F2 stops it.
Results appear in a grid under the editor, and more rows load as you scroll.
Double-click a table to open a console with `SELECT * FROM` that table.

While you type in a console, Brewery suggests SQL keywords, the data source's schemas
and tables after `FROM` or `JOIN`, and columns of the tables in the statement,
including after an alias such as `o.`. Each column shows its type. The schema is
loaded once per data source and reloaded after Refresh or after a `CREATE`, `ALTER` or
`DROP` run from a console.

Data sources are saved in `.brewery/db.json`, which you can commit. Passwords are
never written there: they go to the operating system's secret store (Windows
Credential Manager, Secret Service on Linux, macOS Keychain).

Limits in this version:

- Consoles on the same data source share one connection, so stopping a statement
  in one console can stop a statement running in another, and running a
  statement ends the unread results of the other consoles.
- Only one statement runs at a time; there is no "run all".
- MySQL Connector/J is not bundled because of its GPL licence; MySQL servers are
  reached through MariaDB Connector/J.
- On macOS the console keys are Cmd+Enter (run) and Cmd+F2 (stop).
- Without a secret store on the system the password is not saved; a warning says
  so when you save the data source.
- PostgreSQL results stop loading more rows after about a minute without
  scrolling ("Result expired — run again").
- Expanding the tree of a data source waits while a statement runs on it: each
  data source has one connection.

## Git

The Commit tool window (left stripe, Alt+0) lists changed files with file and hunk
staging, the commit message, amend, push, pull and stash. Ctrl+K (Cmd+K on macOS)
opens it; pressed again inside the tool window, it moves to the commit message.

The branch button in the title bar (or Ctrl+~, Cmd+~ on macOS) opens the Branches popup: recent,
local and remote branches, plus New Branch, Checkout Tag or Revision, Update Project
and Push. Press the right arrow on a branch for checkout, new branch from it, rebase,
merge into the current branch, rename and delete. When a merge or rebase stops on
conflicts, a notification says how many files conflict; its Show Conflicts button
opens the Commit tool window.

The Log tool window (bottom, Alt+9, Cmd+9 on macOS) shows the commit graph with
commit details. Filter it by branch, user, date, path, and text or hash. On very
large repositories without a commit-graph file, Brewery offers to write one, which
makes the Log load faster.

File history opens as a tab in the Log tool window. Open it with View History in the
Project panel's context menu, View File History in the editor or Commit tool window
context menu, or `git: file history` in the command palette.
Select a commit, then Compare with Local, Show Diff for that commit, or Get to put
that version back into the working copy. Renamed files are followed.

Compare with Branch or Revision (`brewery: compare with revision` in the command
palette) compares the open file side by side with a branch,
tag or any revision, or the whole project when no file is focused. The project
compare shows the changes since the merge base with that branch. "Compare with
current" in the Branches popup opens the same view.

When a merge or rebase from the Branches popup stops on conflicts, the Conflicts
dialog lists the files. Accept Yours or Accept Theirs resolves a file in one click;
Merge… opens the 3-pane merge editor: your version on the left, the result in the
middle and theirs on the right. Changes that don't conflict are already applied.
Take each change from either side with the arrow buttons or ignore it, edit the
result freely, jump between conflicts with F7 and Shift+F7, then Apply to save the
file and mark it resolved. Continue finishes the merge or rebase, and Abort cancels
it. During a rebase the left side is still your branch. Clicking a conflicted file
in the Commit tool window opens the merge editor, and `brewery: resolve conflicts`
reopens the dialog. Conflicts after a pull show a notification with a Resolve
button.

## Updates

On Windows, Brewery updates itself from GitHub Releases: it downloads the new version
in the background and installs it when you quit. On Linux, update with your package
manager (or download the new release); Brewery shows a notification when one is out.
Use Check for Updates in the menu to check now.

## Licence

The editor is GPL-3.0-or-later, like Zed. See `LICENSE-GPL`.

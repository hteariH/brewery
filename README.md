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

Completion for `application.yml` and `application.properties` comes later (M3b).

## Licence

The editor is GPL-3.0-or-later, like Zed. See `LICENSE-GPL`.

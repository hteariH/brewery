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

Development builds also need the CLI helper next to the app binary, or the
app prints `could not find zed-cli` on start (harmless, but git over SSH
cannot ask for passwords):

    cargo build -p cli -p zed

## Licence

The editor is GPL-3.0-or-later, like Zed. See `LICENSE-GPL`.

# Brewery

Brewery is an open-source Java IDE for Windows and Linux with Spring and
database tools, built on a fast, natively compiled, GPU-rendered editor.

> Inspired by the IntelliJ IDEA interface. Not affiliated with or endorsed by
> JetBrains.

Brewery is a fork of [Zed](https://github.com/zed-industries/zed). Zed's
collaboration, account, AI and telemetry features are switched off. Brewery still
contacts Zed's servers in two cases, both started by you: browsing or updating
extensions from Zed's extension gallery, and downloading Zed's remote server
when you open a project over SSH.

**Status:** early development (milestone M0 — the fork itself). Not usable for
Java work yet. See the [design](docs/superpowers/specs/2026-10-10-brewery-design.md).

## Building

Follow Zed's build guides — [Windows](docs/src/development/windows.md),
[Linux](docs/src/development/linux.md) — then:

    cargo run -p zed

## Licence

The editor is GPL-3.0-or-later, like Zed. See `LICENSE-GPL`.

# Contributing to Brewery

Thanks for helping make Brewery better!

## Before you start

- Bugs: open an [issue](https://github.com/hteariH/brewery/issues/new/choose) with steps to reproduce.
- Features and bigger changes: start a [discussion](https://github.com/hteariH/brewery/discussions) first, so we agree on the approach before you write code.
- Security problems: don't open a public issue, see [SECURITY.md](SECURITY.md).

## How the code is organised

Brewery is a fork of the [Zed](https://github.com/zed-industries/zed) editor plus a Kotlin daemon:

- `crates/brewery_*`: Brewery's own Rust crates (tool windows, Git, databases, daemon client, bundle lookup).
- `brewery-daemon/`: the Kotlin/JVM daemon (Gradle/Maven model, Spring, databases), Apache-2.0.
- Zed's own crates: changed as little as possible. Every edit to a Zed file is short and marked `// brewery:`, so upstream merges stay easy.
- `script/brewery/`, `packaging/`: offline bundle, packages and release scripts.

[`docs/brewery/FORK.md`](docs/brewery/FORK.md) explains the fork rules, how to build and test each part, the bundle layout, packaging and releasing.

## Pull requests

- Keep each pull request focused on one change, and link the issue it fixes ("Fixes #123").
- Add or update tests. Rust: `cargo test -p <crate>`; daemon: `cd brewery-daemon && ./gradlew test`; scripts: `python -m unittest discover script/brewery`.
- Run `cargo clippy -p <crate> --all-targets -- -D warnings` on the crates you touched.
- Use LF line endings and the existing code style.
- Note user-visible changes in [`CHANGELOG.md`](CHANGELOG.md).

## Licence

By contributing, you agree that your contributions are licensed under the licence of the part you change: GPL-3.0-or-later for the editor, Apache-2.0 for `brewery-daemon/`.

Everyone taking part is expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

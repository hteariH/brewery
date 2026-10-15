# Release checklist

Done by hand for every release; the release workflow
(`.github/workflows/brewery_release.yml`) builds and checks the packages, and creates a
draft GitHub Release.

## Before tagging

1. `crates/zed/BREWERY_VERSION` holds the new version; `CHANGELOG.md` has a section for it.
2. A dry run of the release workflow (Actions → Brewery Release → Run workflow,
   `dry_run` on) is green, and its artifacts contain every asset.
3. Main's CI is green.

## Tag and draft

4. `git tag v<version>` on main and push the tag. The workflow creates a **draft**
   release with the installer, tarball, deb, rpm, macOS zip, `SHA256SUMS`,
   `latest.json`, the winget manifests and the AUR `PKGBUILD`/`.SRCINFO`.

## Check the draft's packages on clean machines

5. Windows 11 (a fresh VM or Windows Sandbox), **offline after install**:
   - Install the setup.exe (SmartScreen: "More info" → "Run anyway"), start Brewery.
   - Open a Spring Boot sample (spring-petclinic, with its Maven/Gradle
     dependencies already downloaded once online), check Java completion, go to
     definition, run the app from the gutter, hit a breakpoint.
   - Services panel lists the app; open a Database data source (SQLite file), run a query.
   - Git: Commit window, Log, Branches popup.
6. Ubuntu 24.04 and Fedora: install the deb / rpm, the same checks on Linux.
7. Updates: with the previous version installed on Windows, set
   `BREWERY_UPDATE_MANIFEST_URL` to the draft's `latest.json` copy served over HTTPS
   (for example a gist) and check that the update downloads and installs on quit.
   (First release: skip.)

## Publish (owner actions)

8. Publish the draft release on GitHub.
9. winget: submit the manifests from the release (`wingetcreate submit` or a PR to
   `microsoft/winget-pkgs`).
10. AUR: push `PKGBUILD` and `.SRCINFO` to `ssh://aur@aur.archlinux.org/brewery-bin.git`.
11. Announce.

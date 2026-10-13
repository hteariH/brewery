//! Brewery: app updates come from a `latest.json` manifest published as a
//! GitHub Release asset instead of Zed's cloud. The SSH remote-server asset
//! keeps using Zed's cloud (it must match Zed's version), see
//! `AutoUpdater::get_release_asset`.

use std::{collections::HashMap, path::Path, sync::LazyLock};

use anyhow::{Context as _, Result};
use http_client::HttpClient;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use smol::io::AsyncReadExt as _;

use release_channel::ReleaseChannel;

use crate::ReleaseAsset;

/// The manifest of the newest non-draft, non-pre-release GitHub Release.
/// GitHub redirects this to the asset's storage host, so the request must
/// follow redirects.
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/hteariH/brewery/releases/latest/download/latest.json";

/// Overrides [`DEFAULT_MANIFEST_URL`] for testing updates (read once).
pub const MANIFEST_URL_ENV: &str = "BREWERY_UPDATE_MANIFEST_URL";

/// The Windows installer is saved under `updates\` with this name, then run with
/// `/verysilent /update=true`.
pub const WINDOWS_INSTALLER_FILE_NAME: &str = "Brewery-setup.exe";

static MANIFEST_URL: LazyLock<String> =
    LazyLock::new(|| manifest_url_from(std::env::var(MANIFEST_URL_ENV).ok()));

/// The manifest URL in effect for this process.
pub fn manifest_url() -> &'static str {
    MANIFEST_URL.as_str()
}

fn manifest_url_from(env_value: Option<String>) -> String {
    env_value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MANIFEST_URL.to_string())
}

/// What Brewery does with a newer release on an OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    /// Download and install it (Windows: the Inno installer and the update helper).
    Install,
    /// Only tell the user. Linux packages are managed by the package manager and the
    /// macOS assets are zips Zed's dmg installer cannot apply.
    Notify,
}

/// Windows installs updates; every other OS gets a notification.
pub fn update_action_for_os(os: &str) -> UpdateAction {
    match os {
        "windows" => UpdateAction::Install,
        _ => UpdateAction::Notify,
    }
}

/// A newer release this build does not install itself (see [`UpdateAction::Notify`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: Version,
    pub notes_url: String,
}

/// The GitHub release page of a Brewery version.
pub fn release_notes_url(version: &Version) -> String {
    let mut version = version.clone();
    version.build = semver::BuildMetadata::EMPTY;
    format!("{}/releases/tag/v{version}", brewery_core::REPOSITORY_URL)
}

/// What "View Release Notes" opens on a channel.
pub fn release_notes_url_for_channel(channel: ReleaseChannel, version: &Version) -> String {
    match channel {
        ReleaseChannel::Stable | ReleaseChannel::Preview => release_notes_url(version),
        ReleaseChannel::Nightly | ReleaseChannel::Dev => {
            format!("{}/commits/main/", brewery_core::REPOSITORY_URL)
        }
    }
}

/// Zed's product announcements (e.g. Delta) are not Brewery's to make.
pub const SHOW_ZED_ANNOUNCEMENTS: bool = false;

/// Release notes open the GitHub release page; Zed's notes API is not used.
pub const RELEASE_NOTES_IN_BROWSER: bool = true;

/// What a manual "Check for Updates" says on a channel that never updates:
/// `(message, detail)`, or `None` when the channel checks for updates.
pub fn updates_off_prompt(channel: ReleaseChannel) -> Option<(&'static str, &'static str)> {
    const DETAIL: &str = "Download Brewery from GitHub Releases.";
    match channel {
        ReleaseChannel::Dev => Some(("Updates are off in development builds", DETAIL)),
        ReleaseChannel::Nightly => Some(("Updates are off in nightly builds", DETAIL)),
        ReleaseChannel::Stable | ReleaseChannel::Preview => None,
    }
}

/// Linux builds point at the package manager first.
pub const LINUX_UPDATE_HINT: &str =
    "Update Brewery with your package manager or from GitHub Releases.";

/// Headline of the "update available" notification.
pub fn available_update_title(version: &Version) -> String {
    format!("Brewery {version} is available")
}

/// Body of the "update available" notification on an OS.
pub fn available_update_message(os: &str) -> &'static str {
    match os {
        "linux" => LINUX_UPDATE_HINT,
        _ => "Download it from GitHub Releases.",
    }
}

/// `latest.json`. Unknown keys are ignored.
#[derive(Deserialize, Debug, Clone)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes_url: Option<String>,
    #[serde(default)]
    pub assets: HashMap<String, ManifestAsset>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ManifestAsset {
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Returned by [`get_app_release_asset`] when the manifest offers nothing to
/// this build: no asset for this OS/arch, or a pre-release offered to a
/// release build. Callers treat it as "up to date", not as a failure.
#[derive(Debug)]
pub struct NoUpdateAvailable;

impl std::fmt::Display for NoUpdateAvailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no Brewery update available for this platform")
    }
}

impl std::error::Error for NoUpdateAvailable {}

/// Manifest key for an `std::env::consts::{OS, ARCH}` pair.
pub fn platform_key(os: &str, arch: &str) -> Option<String> {
    let os = match os {
        "windows" | "linux" | "macos" => os,
        _ => return None,
    };
    let arch = match arch {
        "x86_64" | "aarch64" => arch,
        _ => return None,
    };
    Some(format!("{os}-{arch}"))
}

/// Parses `latest.json`; the version must be semver (a leading `v` is allowed).
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest> {
    let mut manifest: Manifest = serde_json::from_slice(bytes).with_context(|| {
        format!(
            "error deserializing update manifest {:?}",
            String::from_utf8_lossy(bytes)
        )
    })?;
    let version = manifest.version.trim();
    let version = version.strip_prefix('v').unwrap_or(version);
    let version = Version::parse(version)
        .with_context(|| format!("invalid version in update manifest: {version:?}"))?;
    manifest.version = version.to_string();
    Ok(manifest)
}

/// Picks this platform's asset, or `None` when there is nothing to offer.
pub fn select_release(
    manifest: &Manifest,
    os: &str,
    arch: &str,
    current_version: &Version,
) -> Option<ReleaseAsset> {
    let version = Version::parse(&manifest.version).ok()?;
    if !version.pre.is_empty() && current_version.pre.is_empty() {
        return None;
    }
    let asset = manifest.assets.get(&platform_key(os, arch)?)?;
    Some(ReleaseAsset {
        version: manifest.version.clone(),
        url: asset.url.clone(),
        sha256: asset.sha256.clone(),
        notes_url: manifest.notes_url.clone(),
    })
}

pub async fn fetch_manifest(http_client: &dyn HttpClient, url: &str) -> Result<Manifest> {
    // GitHub answers `releases/latest/download/...` with a 302 to its asset host.
    let follow_redirects = true;
    let mut response = http_client
        .get(url, Default::default(), follow_redirects)
        .await
        .with_context(|| format!("failed to fetch update manifest {url}"))?;
    let mut body = Vec::new();
    response.body_mut().read_to_end(&mut body).await?;
    anyhow::ensure!(
        response.status().is_success(),
        "failed to fetch update manifest {url}: {}",
        response.status(),
    );
    parse_manifest(&body)
}

/// Fetches the manifest and returns this platform's release, or a
/// [`NoUpdateAvailable`] error. No telemetry is sent.
pub async fn get_app_release_asset(
    http_client: &dyn HttpClient,
    url: &str,
    os: &str,
    arch: &str,
    current_version: &Version,
) -> Result<ReleaseAsset> {
    let manifest = fetch_manifest(http_client, url).await?;
    select_release(&manifest, os, arch, current_version).ok_or_else(|| NoUpdateAvailable.into())
}

/// Checks a downloaded file against the manifest's SHA-256 (hex, any case).
/// On mismatch the file is deleted and an error returned.
pub async fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let mut file = smol::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let bytes_read = file.read(&mut buffer).await?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    drop(file);
    let actual = format!("{:x}", hasher.finalize());
    if actual.eq_ignore_ascii_case(expected.trim()) {
        return Ok(());
    }
    if let Err(error) = smol::fs::remove_file(path).await {
        log::warn!("failed to delete {}: {error}", path.display());
    }
    anyhow::bail!(
        "SHA-256 mismatch for downloaded update {}: expected {}, got {actual}",
        path.display(),
        expected.trim(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_client::{FakeHttpClient, RedirectPolicy, Response};
    use std::sync::{Arc, Mutex};

    const FIXTURE: &str = include_str!("../test-data/brewery-latest.json");

    fn v(version: &str) -> Version {
        Version::parse(version).unwrap()
    }

    #[test]
    fn test_manifest_url_defaults_to_latest_release_asset() {
        assert_eq!(manifest_url_from(None), DEFAULT_MANIFEST_URL);
        assert_eq!(manifest_url_from(Some(String::new())), DEFAULT_MANIFEST_URL);
        assert_eq!(
            manifest_url_from(Some("https://example.test/latest.json".into())),
            "https://example.test/latest.json"
        );
    }

    #[test]
    fn test_platform_keys() {
        assert_eq!(
            platform_key("windows", "x86_64").as_deref(),
            Some("windows-x86_64")
        );
        assert_eq!(
            platform_key("linux", "x86_64").as_deref(),
            Some("linux-x86_64")
        );
        assert_eq!(
            platform_key("linux", "aarch64").as_deref(),
            Some("linux-aarch64")
        );
        assert_eq!(
            platform_key("macos", "aarch64").as_deref(),
            Some("macos-aarch64")
        );
        assert_eq!(
            platform_key("macos", "x86_64").as_deref(),
            Some("macos-x86_64")
        );
        assert_eq!(platform_key("freebsd", "x86_64"), None);
        assert_eq!(platform_key("windows", "x86"), None);
    }

    #[test]
    fn test_parse_fixture_ignores_unknown_keys() {
        let manifest = parse_manifest(FIXTURE.as_bytes()).unwrap();
        assert_eq!(manifest.version, "0.1.1");
        assert_eq!(
            manifest.notes_url.as_deref(),
            Some("https://github.com/hteariH/brewery/releases/tag/v0.1.1")
        );
        assert_eq!(manifest.assets.len(), 4);
        assert_eq!(manifest.assets["linux-x86_64"].sha256, None);
    }

    #[test]
    fn test_parse_rejects_bad_manifests() {
        assert!(parse_manifest(b"not json").is_err());
        assert!(parse_manifest(br#"{"assets": {}}"#).is_err());
        assert!(parse_manifest(br#"{"version": "latest", "assets": {}}"#).is_err());
    }

    #[test]
    fn test_select_release_for_platform() {
        let manifest = parse_manifest(FIXTURE.as_bytes()).unwrap();
        let release = select_release(&manifest, "windows", "x86_64", &v("0.1.0")).unwrap();
        assert_eq!(release.version, "0.1.1");
        assert_eq!(
            release.url,
            "https://github.com/hteariH/brewery/releases/download/v0.1.1/Brewery-0.1.1-x86_64-setup.exe"
        );
        assert_eq!(
            release.sha256.as_deref(),
            Some("8f434346648f6b96df89dda901c5176b10a6d83961dd3c1ac88b59b2dc327aa4")
        );

        let release = select_release(&manifest, "linux", "x86_64", &v("0.1.0")).unwrap();
        assert_eq!(release.sha256, None);
        assert_eq!(
            release.notes_url.as_deref(),
            Some("https://github.com/hteariH/brewery/releases/tag/v0.1.1")
        );
    }

    #[test]
    fn test_updates_off_prompt() {
        assert_eq!(
            updates_off_prompt(ReleaseChannel::Dev),
            Some((
                "Updates are off in development builds",
                "Download Brewery from GitHub Releases."
            ))
        );
        assert_eq!(
            updates_off_prompt(ReleaseChannel::Nightly),
            Some((
                "Updates are off in nightly builds",
                "Download Brewery from GitHub Releases."
            ))
        );
        assert_eq!(updates_off_prompt(ReleaseChannel::Stable), None);
        assert_eq!(updates_off_prompt(ReleaseChannel::Preview), None);
    }

    #[test]
    fn test_only_windows_installs_updates() {
        assert_eq!(update_action_for_os("windows"), UpdateAction::Install);
        assert_eq!(update_action_for_os("linux"), UpdateAction::Notify);
        assert_eq!(update_action_for_os("macos"), UpdateAction::Notify);
        assert_eq!(update_action_for_os("freebsd"), UpdateAction::Notify);
    }

    #[test]
    fn test_windows_installer_file_name() {
        // The Windows install step saves the downloaded setup under this name.
        assert_eq!(WINDOWS_INSTALLER_FILE_NAME, "Brewery-setup.exe");
    }

    #[test]
    fn test_release_notes_urls() {
        assert_eq!(
            release_notes_url(&v("0.1.1")),
            "https://github.com/hteariH/brewery/releases/tag/v0.1.1"
        );
        for channel in [ReleaseChannel::Stable, ReleaseChannel::Preview] {
            assert_eq!(
                release_notes_url_for_channel(channel, &v("0.2.0-rc.1+abc")),
                "https://github.com/hteariH/brewery/releases/tag/v0.2.0-rc.1",
                "{channel:?}"
            );
        }
        for channel in [ReleaseChannel::Nightly, ReleaseChannel::Dev] {
            assert_eq!(
                release_notes_url_for_channel(channel, &v("0.1.0")),
                "https://github.com/hteariH/brewery/commits/main/",
                "{channel:?}"
            );
        }
    }

    #[test]
    fn test_available_update_texts() {
        assert_eq!(
            available_update_title(&v("0.1.1")),
            "Brewery 0.1.1 is available"
        );
        assert_eq!(
            available_update_message("linux"),
            "Update Brewery with your package manager or from GitHub Releases."
        );
        assert_eq!(
            available_update_message("macos"),
            "Download it from GitHub Releases."
        );
    }

    #[test]
    fn test_select_release_missing_platform_is_no_update() {
        let manifest = parse_manifest(FIXTURE.as_bytes()).unwrap();
        assert!(select_release(&manifest, "macos", "x86_64", &v("0.1.0")).is_none());
        assert!(select_release(&manifest, "freebsd", "x86_64", &v("0.1.0")).is_none());
    }

    #[test]
    fn test_pre_release_never_offered_to_release_build() {
        let manifest =
            parse_manifest(FIXTURE.replace("\"0.1.1\"", "\"0.2.0-rc.1\"").as_bytes()).unwrap();
        assert!(select_release(&manifest, "windows", "x86_64", &v("0.1.0")).is_none());
        let release = select_release(&manifest, "windows", "x86_64", &v("0.1.1-rc.1")).unwrap();
        assert_eq!(release.version, "0.2.0-rc.1");
    }

    #[test]
    fn test_manifest_versions_compare_as_semver() {
        let newer = |installed: &str, fetched: &str| {
            crate::AutoUpdater::check_if_fetched_version_is_newer_non_nightly(
                v(installed),
                v(fetched),
            )
        };
        assert_eq!(newer("0.1.0", "0.1.1"), Some(v("0.1.1")));
        assert_eq!(newer("0.1.1", "0.1.1"), None);
        assert_eq!(newer("0.2.0", "0.1.1"), None);
        assert_eq!(newer("0.9.0", "0.10.0"), Some(v("0.10.0")));
    }

    #[test]
    fn test_fetch_follows_redirects_and_sends_no_telemetry() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let client = FakeHttpClient::create({
            let seen = seen.clone();
            move |req| {
                seen.lock().unwrap().push((
                    req.uri().to_string(),
                    req.extensions().get::<RedirectPolicy>().cloned(),
                ));
                async move {
                    Ok(Response::builder()
                        .status(200)
                        .body(FIXTURE.into())
                        .unwrap())
                }
            }
        });

        let release = smol::block_on(get_app_release_asset(
            client.as_ref(),
            DEFAULT_MANIFEST_URL,
            "windows",
            "x86_64",
            &v("0.1.0"),
        ))
        .unwrap();
        assert_eq!(release.version, "0.1.1");

        let seen = seen.lock().unwrap();
        assert_eq!(
            *seen,
            vec![(
                DEFAULT_MANIFEST_URL.to_string(),
                Some(RedirectPolicy::FollowAll)
            )]
        );
    }

    #[test]
    fn test_fetch_missing_platform_is_no_update_error() {
        let client = FakeHttpClient::create(|_| async move {
            Ok(Response::builder()
                .status(200)
                .body(FIXTURE.into())
                .unwrap())
        });
        let error = smol::block_on(get_app_release_asset(
            client.as_ref(),
            DEFAULT_MANIFEST_URL,
            "macos",
            "x86_64",
            &v("0.1.0"),
        ))
        .unwrap_err();
        assert!(error.is::<NoUpdateAvailable>());
    }

    #[test]
    fn test_fetch_http_error_is_an_error() {
        let client = FakeHttpClient::with_404_response();
        let error = smol::block_on(get_app_release_asset(
            client.as_ref(),
            DEFAULT_MANIFEST_URL,
            "windows",
            "x86_64",
            &v("0.1.0"),
        ))
        .unwrap_err();
        assert!(!error.is::<NoUpdateAvailable>());
    }

    #[test]
    fn test_verify_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Brewery-setup.exe");
        std::fs::write(&path, "hi").unwrap();

        smol::block_on(verify_sha256(
            &path,
            "8f434346648f6b96df89dda901c5176b10a6d83961dd3c1ac88b59b2dc327aa4",
        ))
        .unwrap();
        smol::block_on(verify_sha256(
            &path,
            "8F434346648F6B96DF89DDA901C5176B10A6D83961DD3C1AC88B59B2DC327AA4",
        ))
        .unwrap();
        assert!(path.exists());

        let error = smol::block_on(verify_sha256(&path, &"0".repeat(64))).unwrap_err();
        assert!(error.to_string().contains("SHA-256"), "{error:#}");
        assert!(!path.exists(), "a file with a bad hash must be deleted");
    }
}

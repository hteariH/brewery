#![allow(
    clippy::disallowed_methods,
    reason = "build helper used only from build scripts"
)]
#![cfg(target_os = "windows")]

use std::process::Command;

fn git_sha() -> Option<String> {
    if let Ok(sha) = std::env::var("ZED_COMMIT_SHA") {
        return Some(sha);
    }

    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// brewery: the release version lives in crates/zed/BREWERY_VERSION and replaces each
// crate's own CARGO_PKG_VERSION in the exe metadata.
const BREWERY_VERSION_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../zed/BREWERY_VERSION");

fn brewery_version() -> Option<String> {
    let version = std::fs::read_to_string(BREWERY_VERSION_PATH).ok()?;
    let version = version.trim();
    (!version.is_empty()).then(|| version.to_string())
}

/// `major,minor,patch,0` for the FILEVERSION/PRODUCTVERSION fields; pre-release and
/// build suffixes ("0.1.0-rc.1") are ignored.
fn file_version(version: &str) -> String {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let mut parts = core
        .split('.')
        .map(|part| part.parse::<u16>().unwrap_or(0))
        .chain(std::iter::repeat(0));
    format!(
        "{},{},{},{}",
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

fn product_version() -> String {
    let commit_sha = git_sha();
    // brewery: Brewery's version, not the crate's.
    let pkg_version =
        brewery_version().unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    let channel = std::env::var("RELEASE_CHANNEL").unwrap_or_else(|_| "dev".into());
    let build_id = std::env::var("GITHUB_RUN_NUMBER").ok();

    let mut metadata = channel;
    if let Some(build_id) = &build_id {
        metadata.push('.');
        metadata.push_str(build_id);
    }
    if let Some(sha) = &commit_sha {
        metadata.push('.');
        metadata.push_str(sha);
    }

    format!("{pkg_version}+{metadata}")
}

const ICON_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../zed/resources/windows");
const MANIFEST_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/manifest.xml");

pub fn compile(manifest: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed={BREWERY_VERSION_PATH}"); // brewery:
    let channel = option_env!("RELEASE_CHANNEL").unwrap_or("dev");
    let (icon_filename, product_name) = match channel {
        // brewery: product names are ReleaseChannel::display_name()
        "stable" => ("app-icon.ico", "Brewery"),
        "preview" => ("app-icon-preview.ico", "Brewery Preview"),
        "nightly" => ("app-icon-nightly.ico", "Brewery Nightly"),
        _ => ("app-icon-dev.ico", "Brewery Dev"),
    };
    let icon = std::path::PathBuf::from(ICON_DIR).join(icon_filename);
    let icon_escaped = icon.to_string_lossy().replace('\\', "\\\\");

    let manifest_line = if manifest {
        let escaped = MANIFEST_PATH.replace('\\', "\\\\");
        format!("1 24 \"{escaped}\"")
    } else {
        String::new()
    };

    // brewery: Brewery's version, not the crate's.
    let pkg_version =
        brewery_version().unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    let product_version = product_version();
    let file_version = file_version(&pkg_version);

    // The resource paths can contain non-ASCII characters (e.g. when the
    // repository lives under a path with such characters). Rust Strings are
    // encoded using UTF-8, but rc.exe decodes the .rc file using the ANSI code
    // page by default, which mangles those paths and makes it fail to find the
    // referenced files, so specify UTF-8 decoding.
    let rc_content = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon_escaped}"
{manifest_line}

1 VERSIONINFO
FILEVERSION {file_version}
PRODUCTVERSION {file_version}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "FileDescription", "{product_name}\0"
            VALUE "FileVersion", "{pkg_version}\0"
            VALUE "ProductName", "{product_name}\0"
            VALUE "ProductVersion", "{product_version}\0"
            VALUE "CompanyName", "Brewery contributors\0"
            VALUE "LegalCopyright", "Copyright 2026 Brewery contributors. Portions copyright Zed Industries, Inc.\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0409, 1200
    END
END
"#
    );

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    let rc_path = out_dir.join("zed_resources.rc");
    std::fs::write(&rc_path, rc_content)?;

    if let Ok(toolkit_path) = std::env::var("ZED_RC_TOOLKIT_PATH") {
        let rc_exe = std::path::Path::new(&toolkit_path).join("rc.exe");
        unsafe {
            std::env::set_var("RC", rc_exe);
        }
    }

    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .unwrap();

    Ok(())
}

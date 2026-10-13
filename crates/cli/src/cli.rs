use std::path::PathBuf;

use anyhow::Result;
use collections::HashMap;
pub use ipc_channel::ipc;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct IpcHandshake {
    pub requests: ipc::IpcSender<CliRequest>,
    pub responses: ipc::IpcReceiver<CliResponse>,
}

/// Controls how CLI paths are opened — whether to reuse existing windows,
/// create new ones, or add to the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpenBehavior {
    /// Consult the user's `cli_default_open_behavior` setting.
    #[default]
    Default,
    /// Always create a new window. No matching against existing worktrees.
    /// Corresponds to `zed -n`.
    AlwaysNew,
    /// Create a new window unless opening a subpath of an existing project.
    PreferNewWindow,
    /// Match broadly including subdirectories, and fall back to any existing
    /// window if no worktree matched. Corresponds to `zed -a`.
    Add,
    /// Open directories as a new workspace in the current Zed window's sidebar.
    /// Reuse existing windows for files in open worktrees.
    /// Corresponds to `zed -e`.
    ExistingWindow,
    /// New window for directories, reuse existing window for files in open
    /// worktrees. The classic pre-sidebar behavior.
    /// Corresponds to `zed --classic`.
    Classic,
    /// Replace the content of an existing window with a new workspace.
    /// Corresponds to `zed -r`.
    Reuse,
}

/// The setting-level enum for configuring default behavior. This only has
/// two values because the other modes are always explicitly requested via
/// CLI flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliBehaviorSetting {
    /// Open directories as a new workspace in the current Zed window's sidebar.
    ExistingWindow,
    /// Open paths in a new window unless they are subpaths of an existing project.
    NewWindow,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CliRequest {
    Open {
        paths: Vec<String>,
        urls: Vec<String>,
        diff_paths: Vec<[String; 2]>,
        diff_all: bool,
        wsl: Option<String>,
        wait: bool,
        #[serde(default)]
        open_behavior: OpenBehavior,
        env: Option<HashMap<String, String>>,
        user_data_dir: Option<String>,
        dev_container: bool,
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    SetOpenBehavior {
        behavior: CliBehaviorSetting,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CliResponse {
    Ping,
    Stdout { message: String },
    Stderr { message: String },
    Exit { status: i32 },
    PromptOpenBehavior,
}

/// When Zed started not as an *.app but as a binary (e.g. local development),
/// there's a possibility to tell it to behave "regularly".
///
/// Note that in the main zed binary, this variable is unset after it's read for the first time,
/// therefore it should always be accessed through the `FORCE_CLI_MODE` static.
pub const FORCE_CLI_MODE_ENV_VAR_NAME: &str = "ZED_FORCE_CLI_MODE";

/// Abstracts the transport for sending CLI responses (Zed → CLI).
///
/// Production code uses `IpcSender<CliResponse>`. Tests can provide in-memory
/// implementations to avoid OS-level IPC.
pub trait CliResponseSink: Send + 'static {
    fn send(&self, response: CliResponse) -> Result<()>;
}

impl CliResponseSink for ipc::IpcSender<CliResponse> {
    fn send(&self, response: CliResponse) -> Result<()> {
        ipc::IpcSender::send(self, response).map_err(|error| anyhow::anyhow!("{error}"))
    }
}

// brewery: CLI and app lookup names live here as pure data so they are unit-testable
// and shared by the `cli` binary and the app's open listener.

/// Linux app lookup, relative to the CLI's directory. libexec is the standard,
/// lib/brewery is for Arch (and other non-libexec distros), ./brewery is for the
/// target directory in development builds.
pub const LINUX_APP_LOCATIONS: [&str; 3] = [
    "../libexec/brewery-editor",
    "../lib/brewery/brewery-editor",
    "./brewery",
];

/// Windows app lookup, relative to the CLI's directory. ../Brewery.exe is the
/// standard, lib/brewery is for MSYS2, ./brewery.exe is for development builds.
pub const WINDOWS_APP_LOCATIONS: [&str; 3] = [
    "../Brewery.exe",
    "../lib/brewery/brewery-editor.exe",
    "./brewery.exe",
];

/// File name of the datagram socket the running app listens on for CLI requests.
/// Distinct from Zed's `zed-<channel>.sock` so Brewery never talks to a running Zed.
pub fn socket_file_name(channel: &str) -> String {
    format!("brewery-{channel}.sock")
}

/// `--version` output for the Linux and Windows apps.
pub fn app_version_string(
    channel: &str,
    release_version: &str,
    commit_sha: Option<&str>,
    path: &std::path::Path,
) -> String {
    format!(
        "Brewery {}{}{} – {}",
        if channel == "stable" {
            "".to_string()
        } else {
            format!("{channel} ")
        },
        release_version,
        match commit_sha {
            Some(commit_sha) => format!(" {commit_sha} "),
            None => "".to_string(),
        },
        path.display(),
    )
}

/// `--version` output for a macOS bundle.
pub fn bundle_version_string(version: &str, path: &std::path::Path) -> String {
    format!("Brewery {version} – {}", path.display())
}

#[cfg(test)]
mod brewery_name_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn linux_lookup_order_has_brewery_names() {
        assert_eq!(LINUX_APP_LOCATIONS[0], "../libexec/brewery-editor");
        assert_eq!(LINUX_APP_LOCATIONS[2], "./brewery");
    }

    #[test]
    fn windows_lookup_order_has_brewery_names() {
        assert_eq!(WINDOWS_APP_LOCATIONS[0], "../Brewery.exe");
        assert_eq!(WINDOWS_APP_LOCATIONS[2], "./brewery.exe");
    }

    #[test]
    fn no_zed_names_left() {
        for name in LINUX_APP_LOCATIONS.iter().chain(WINDOWS_APP_LOCATIONS.iter()) {
            assert!(!name.to_lowercase().contains("zed"), "{name}");
        }
        assert!(!socket_file_name("stable").contains("zed"));
    }

    #[test]
    fn socket_name() {
        assert_eq!(socket_file_name("nightly"), "brewery-nightly.sock");
    }

    #[test]
    fn version_strings() {
        let p = Path::new("/x/brewery-editor");
        assert_eq!(
            app_version_string("stable", "0.1.0", None, p),
            format!("Brewery 0.1.0 – {}", p.display())
        );
        assert!(app_version_string("dev", "", Some("abc"), p).starts_with("Brewery dev  abc "));
        assert!(bundle_version_string("0.1.0", p).starts_with("Brewery 0.1.0 – "));
    }
}

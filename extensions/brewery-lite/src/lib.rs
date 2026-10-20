//! Brewery Lite: runs the Brewery daemon as Zed's Spring config language server and as an MCP server.
//! The daemon jar comes from the GitHub release matching this extension's version; Java 21 comes from
//! the `java_path` setting, the environment or a downloaded Temurin JRE, in that order.

mod daemon_jar;
mod java;

use std::fs;
use zed_extension_api::{
    self as zed,
    serde_json::{self, Value},
    settings::{ContextServerSettings, LspSettings},
};

const REPO: &str = "hteariH/brewery";
const LSP_ID: &str = "brewery-spring-config";
const MCP_ID: &str = "brewery";
const JRE_DIR: &str = "jre";
const JRE_PARTIAL_DIR: &str = "jre.partial";
const SUMS: &str = "SHA256SUMS";
const VERSION: &str = env!("CARGO_PKG_VERSION");

const LSP_SETTING_HINT: &str =
    "set \"java_path\" under lsp.brewery-spring-config.settings in Zed's settings.json";
const MCP_SETTING_HINT: &str =
    "set \"java_path\" under context_servers.brewery.settings in Zed's settings.json";

type Status<'a> = &'a dyn Fn(zed::LanguageServerInstallationStatus);

#[derive(Default)]
struct BreweryLite {
    jar: Option<String>,
    java: Option<String>,
}

/// The `java_path` setting, if set to a non-empty string.
fn java_path_setting(settings: Option<Value>) -> Option<String> {
    let path = settings?.get("java_path")?.as_str()?.trim().to_owned();
    (!path.is_empty()).then_some(path)
}

/// The value of `key` in `env`; Windows environment names are case-insensitive (`Path`).
fn env_var<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// The environment for probing `java -version` as the worktree shell would run it: just its PATH,
/// with `dir` in front when given, so that a bare `java` resolves inside `dir` first.
fn probe_env(env: &[(String, String)], dir: Option<&str>, windows: bool) -> Vec<(String, String)> {
    let separator = if windows { ';' } else { ':' };
    let path = env_var(env, "PATH");
    let value = match (dir, path) {
        (Some(dir), Some(path)) => format!("{dir}{separator}{path}"),
        (Some(dir), None) => dir.to_owned(),
        (None, Some(path)) => path.to_owned(),
        (None, None) => return Vec::new(),
    };
    vec![("PATH".to_owned(), value)]
}

/// `<home>/bin/java` (or `java.exe`) with the platform's separator.
fn java_in_home(home: &str, windows: bool) -> (String, String) {
    let (sep, exe) = if windows {
        ('\\', "java.exe")
    } else {
        ('/', "java")
    };
    let home = home.trim_end_matches(['/', '\\']);
    let bin = format!("{home}{sep}bin");
    let java = format!("{bin}{sep}{exe}");
    (bin, java)
}

fn context_server_settings_schema() -> String {
    serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "properties": {
            "java_path": {
                "type": "string",
                "description": "Path of the Java 21+ executable that runs the Brewery daemon. When unset, Brewery Lite uses java from PATH if it is Java 21 or newer, otherwise it downloads a Java 21 runtime."
            }
        },
        "additionalProperties": false
    })
    .to_string()
}

const INSTALLATION_INSTRUCTIONS: &str = "\
The Brewery MCP server gives the agent the project model, Spring beans and endpoints, run \
configurations, and read-only access to the project's database (`spring.datasource.url`).

It runs the Brewery daemon on Java 21 or newer. Brewery Lite downloads the daemon jar of its own \
version from the Brewery GitHub release and checks it against the release's `SHA256SUMS`. It uses \
`java` from `PATH` when that is Java 21 or newer; otherwise it downloads a Java 21 runtime \
(Eclipse Temurin) once.

To use a specific Java, set `java_path` to its `java` executable, for example \
`\"/path/to/jdk-21/bin/java\"`. No other settings are needed.";

/// `java -version` through process:exec (the only command the manifest allows); true when it
/// reports Java 21 or newer. `env` is added to Zed's own environment for the probe.
fn java_is_recent(env: &[(String, String)]) -> bool {
    let output = zed::process::Command::new("java")
        .arg("-version")
        .envs(env.iter().cloned())
        .output();
    let Ok(output) = output else {
        return false;
    };
    let text = String::from_utf8_lossy(&output.stderr).into_owned()
        + &String::from_utf8_lossy(&output.stdout);
    java::parse_major(&text).is_some_and(|major| major >= java::MIN_MAJOR)
}

fn list_dir(dir: &str) -> Vec<String> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn absolute(relative: &str) -> Result<String, String> {
    let dir = std::env::current_dir()
        .map_err(|e| format!("Brewery Lite cannot find its work directory: {e}"))?;
    Ok(dir.join(relative).to_string_lossy().into_owned())
}

impl BreweryLite {
    /// Path of the verified daemon jar in the extension's work dir, downloading it on first use.
    fn daemon_jar(&mut self, status: Status) -> Result<String, String> {
        if let Some(jar) = &self.jar
            && fs::metadata(jar).is_ok()
        {
            return Ok(jar.clone());
        }
        let name = daemon_jar::asset_name(VERSION);
        if fs::metadata(&name).is_err() {
            status(zed::LanguageServerInstallationStatus::Downloading);
            Self::download_jar(&name)?;
            for stale in daemon_jar::stale_jars(&list_dir("."), &name) {
                let _ = fs::remove_file(stale);
            }
        }
        let jar = absolute(&name)?;
        self.jar = Some(jar.clone());
        Ok(jar)
    }

    /// Downloads `name` from release v<VERSION>, verifies it against the release's SHA256SUMS
    /// and only then moves it to `name`, so an existing `name` is always a verified jar.
    fn download_jar(name: &str) -> Result<(), String> {
        let tag = format!("v{VERSION}");
        let release = zed::github_release_by_tag_name(REPO, &tag).map_err(|e| {
            format!(
                "Brewery Lite could not find release {tag} of github.com/{REPO} ({e}); \
                 check the network connection, or update Brewery Lite"
            )
        })?;
        let asset = |n: &str| {
            release
                .assets
                .iter()
                .find(|a| a.name == n)
                .map(|a| a.download_url.clone())
        };
        let missing = |n: &str| {
            format!(
                "release {tag} of github.com/{REPO} has no {n}; this Brewery Lite version cannot \
                 download its daemon, update Brewery Lite"
            )
        };
        let jar_url = asset(name).ok_or_else(|| missing(name))?;
        let sums_url = asset(SUMS).ok_or_else(|| missing(SUMS))?;

        let partial = format!("{name}.partial");
        let _ = fs::remove_file(&partial);
        let _ = fs::remove_file(SUMS);
        zed::download_file(&jar_url, &partial, zed::DownloadedFileType::Uncompressed)
            .map_err(|e| format!("Brewery Lite could not download {name}: {e}"))?;
        zed::download_file(&sums_url, SUMS, zed::DownloadedFileType::Uncompressed)
            .map_err(|e| format!("Brewery Lite could not download {SUMS} of {tag}: {e}"))?;

        let verified = (|| {
            let sums = fs::read_to_string(SUMS).map_err(|e| format!("reading {SUMS}: {e}"))?;
            let expected = daemon_jar::expected_sha256(&sums, name)
                .ok_or_else(|| format!("{SUMS} of {tag} does not list {name}"))?;
            let bytes = fs::read(&partial).map_err(|e| format!("reading {name}: {e}"))?;
            let actual = daemon_jar::sha256_hex(&bytes);
            if actual != expected {
                return Err(format!(
                    "{name} failed its checksum (expected {expected}, got {actual}); \
                     restart Zed to download it again"
                ));
            }
            fs::rename(&partial, name).map_err(|e| format!("saving {name}: {e}"))
        })();
        let _ = fs::remove_file(SUMS);
        if verified.is_err() {
            let _ = fs::remove_file(&partial);
        }
        verified.map_err(|e| format!("Brewery Lite: {e}"))
    }

    /// The downloaded Temurin JRE, fetching it on first use.
    fn downloaded_java(&mut self, status: Status, hint: &str) -> Result<String, String> {
        let (os, arch) = zed::current_platform();
        let mut found = java::java_in_jre(&list_dir(JRE_DIR), os);
        if found.is_none() {
            status(zed::LanguageServerInstallationStatus::Downloading);
            let url = java::adoptium_url(os, arch).map_err(|e| format!("{e}; {hint}"))?;
            let kind = if os == zed::Os::Windows {
                zed::DownloadedFileType::Zip
            } else {
                zed::DownloadedFileType::GzipTar
            };
            // Extract into an empty directory and swap it in, so `jre/` never holds two JREs
            // or the remains of an interrupted download.
            let _ = fs::remove_dir_all(JRE_PARTIAL_DIR);
            zed::download_file(&url, JRE_PARTIAL_DIR, kind).map_err(|e| {
                format!(
                    "Brewery Lite found no Java {} and could not download one ({e}); \
                     install Java {} or {hint}",
                    java::MIN_MAJOR,
                    java::MIN_MAJOR
                )
            })?;
            let _ = fs::remove_dir_all(JRE_DIR);
            fs::rename(JRE_PARTIAL_DIR, JRE_DIR)
                .map_err(|e| format!("Brewery Lite could not save the Java runtime: {e}"))?;
            found = java::java_in_jre(&list_dir(JRE_DIR), os);
        }
        let rel = found.ok_or_else(|| {
            format!(
                "the downloaded Java {} runtime has no bin/java; {hint}",
                java::MIN_MAJOR
            )
        })?;
        let path = absolute(&format!("{JRE_DIR}/{rel}"))?;
        if os != zed::Os::Windows {
            zed::make_file_executable(&path).map_err(|e| {
                format!("Brewery Lite could not make {path} executable ({e}); {hint}")
            })?;
        }
        Ok(path)
    }

    /// Java 21+ to run the daemon with: the setting, then (for the language server) the worktree
    /// shell's JAVA_HOME and PATH, then Zed's own PATH, then a downloaded JRE.
    fn java(
        &mut self,
        setting: Option<String>,
        worktree: Option<&zed::Worktree>,
        status: Status,
        hint: &str,
    ) -> Result<String, String> {
        if let Some(path) = setting {
            return Ok(path);
        }
        if let Some(java) = &self.java {
            return Ok(java.clone());
        }
        let java = match worktree
            .and_then(Self::worktree_java)
            .or_else(|| java_is_recent(&[]).then(|| "java".to_owned()))
        {
            Some(java) => java,
            None => self.downloaded_java(status, hint)?,
        };
        self.java = Some(java.clone());
        Ok(java)
    }

    /// Java 21+ from the worktree shell: `$JAVA_HOME/bin/java`, else `java` on its PATH. Each is
    /// probed by running `java -version` with that directory first on PATH.
    fn worktree_java(worktree: &zed::Worktree) -> Option<String> {
        let windows = zed::current_platform().0 == zed::Os::Windows;
        let env = worktree.shell_env();
        if let Some(home) = env_var(&env, "JAVA_HOME").filter(|h| !h.trim().is_empty()) {
            let (bin, java) = java_in_home(home, windows);
            if java_is_recent(&probe_env(&env, Some(&bin), windows)) {
                return Some(java);
            }
        }
        let java = worktree.which("java")?;
        java_is_recent(&probe_env(&env, None, windows)).then_some(java)
    }
}

impl zed::Extension for BreweryLite {
    fn new() -> Self {
        Self::default()
    }

    fn language_server_command(
        &mut self,
        id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        let status = |s: zed::LanguageServerInstallationStatus| {
            zed::set_language_server_installation_status(id, &s)
        };
        let setting = LspSettings::for_worktree(LSP_ID, worktree)
            .ok()
            .and_then(|s| java_path_setting(s.settings));
        let result = self
            .java(setting, Some(worktree), &status, LSP_SETTING_HINT)
            .and_then(|java| Ok((java, self.daemon_jar(&status)?)));
        match result {
            Ok((java, jar)) => {
                status(zed::LanguageServerInstallationStatus::None);
                Ok(zed::Command {
                    command: java,
                    args: vec!["-jar".into(), jar, "--spring-config-lsp".into()],
                    env: Vec::new(),
                })
            }
            Err(e) => {
                status(zed::LanguageServerInstallationStatus::Failed(e.clone()));
                Err(e)
            }
        }
    }

    fn context_server_command(
        &mut self,
        _id: &zed::ContextServerId,
        project: &zed::Project,
    ) -> zed::Result<zed::Command> {
        let setting = ContextServerSettings::for_project(MCP_ID, project)
            .ok()
            .and_then(|s| java_path_setting(s.settings));
        let quiet = |_: zed::LanguageServerInstallationStatus| {};
        let java = self.java(setting, None, &quiet, MCP_SETTING_HINT)?;
        let jar = self.daemon_jar(&quiet)?;
        Ok(zed::Command {
            command: java,
            args: vec!["-jar".into(), jar, "--mcp".into()],
            env: Vec::new(),
        })
    }

    fn context_server_configuration(
        &mut self,
        _id: &zed::ContextServerId,
        _project: &zed::Project,
    ) -> zed::Result<Option<zed::ContextServerConfiguration>> {
        Ok(Some(zed::ContextServerConfiguration {
            installation_instructions: INSTALLATION_INSTRUCTIONS.into(),
            settings_schema: context_server_settings_schema(),
            default_settings: "{}".into(),
        }))
    }
}

zed::register_extension!(BreweryLite);

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn reads_java_path_setting() {
        let s = |v: Value| java_path_setting(Some(v));
        assert_eq!(
            s(serde_json::json!({"java_path": "/jdk/bin/java"})).as_deref(),
            Some("/jdk/bin/java")
        );
        assert_eq!(s(serde_json::json!({"java_path": "  "})), None);
        assert_eq!(s(serde_json::json!({"java_path": 21})), None);
        assert_eq!(s(serde_json::json!({})), None);
        assert_eq!(java_path_setting(None), None);
    }

    #[test]
    fn probe_env_puts_dir_first_on_path() {
        let windows = env(&[("Path", r"C:\Windows"), ("X", "1")]);
        assert_eq!(
            probe_env(&windows, Some(r"C:\jdk\bin"), true),
            env(&[("PATH", r"C:\jdk\bin;C:\Windows")])
        );
        assert_eq!(
            probe_env(&windows, None, true),
            env(&[("PATH", r"C:\Windows")])
        );
        let unix = env(&[("PATH", "/usr/bin")]);
        assert_eq!(
            probe_env(&unix, Some("/jdk/bin"), false),
            env(&[("PATH", "/jdk/bin:/usr/bin")])
        );
        assert_eq!(
            probe_env(&[], Some("/jdk/bin"), false),
            env(&[("PATH", "/jdk/bin")])
        );
        assert_eq!(probe_env(&[], None, false), Vec::new());
        assert_eq!(
            env_var(&env(&[("java_home", "/jdk")]), "JAVA_HOME"),
            Some("/jdk")
        );
    }

    #[test]
    fn java_under_java_home() {
        assert_eq!(
            java_in_home("/opt/jdk-21/", false),
            ("/opt/jdk-21/bin".into(), "/opt/jdk-21/bin/java".into())
        );
        assert_eq!(
            java_in_home("C:\\jdk-21", true),
            ("C:\\jdk-21\\bin".into(), "C:\\jdk-21\\bin\\java.exe".into())
        );
    }

    #[test]
    fn settings_schema_accepts_java_path() {
        let schema: Value = serde_json::from_str(&context_server_settings_schema()).unwrap();
        assert_eq!(
            schema["properties"]["java_path"]["type"],
            Value::String("string".into())
        );
        assert!(schema.get("required").is_none());
    }
}

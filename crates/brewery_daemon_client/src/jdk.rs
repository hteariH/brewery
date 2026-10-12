//! Finding a JDK (21+) to run the daemon with, and the daemon jar itself.

use anyhow::{Context as _, Result, bail};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JdkCandidate {
    pub java: PathBuf,
    pub major: u32,
}

/// Parses a `JAVA_VERSION="21.0.11"` line of a JDK's `release` file.
pub fn parse_release_version(contents: &str) -> Option<u32> {
    let line = contents
        .lines()
        .find_map(|line| line.trim().strip_prefix("JAVA_VERSION="))?;
    parse_version_string(line.trim().trim_matches('"'))
}

/// `21.0.11` -> 21; the legacy `1.8.0_402` scheme -> 8.
fn parse_version_string(version: &str) -> Option<u32> {
    let mut parts = version.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Extracts the major version from `java -version` output (`openjdk version "21.0.11" ...`).
fn parse_java_version_output(output: &str) -> Option<u32> {
    let marker = "version \"";
    let start = output.find(marker)? + marker.len();
    let rest = &output[start..];
    let end = rest.find('"')?;
    parse_version_string(&rest[..end])
}

/// First candidate, in the given (search) order, that is new enough.
pub fn pick_jdk(candidates: Vec<JdkCandidate>, min_major: u32) -> Option<JdkCandidate> {
    candidates
        .into_iter()
        .find(|candidate| candidate.major >= min_major)
}

fn java_exe_name() -> &'static str {
    if cfg!(windows) { "java.exe" } else { "java" }
}

/// Version of a JDK home: the cheap `release` file first, then running `java -version`.
fn candidate_from_home(home: &Path) -> Option<JdkCandidate> {
    let java = home.join("bin").join(java_exe_name());
    if !java.is_file() {
        return None;
    }
    let major = std::fs::read_to_string(home.join("release"))
        .ok()
        .and_then(|contents| parse_release_version(&contents))
        .or_else(|| major_from_running(&java))?;
    Some(JdkCandidate { java, major })
}

// Runs a short-lived `java -version`; `find_java` is only called from background tasks.
#[allow(clippy::disallowed_methods)]
fn major_from_running(java: &Path) -> Option<u32> {
    let output = util::command::new_std_command(java)
        .arg("-version")
        .output()
        .ok()?;
    // `java -version` prints to stderr; accept stdout too for odd launchers.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    parse_java_version_output(&text)
}

fn candidate_from_java_exe(java: &Path) -> Option<JdkCandidate> {
    if !java.is_file() {
        return None;
    }
    // bin/java sits next to a `release` file in the JDK home.
    if let Some(home) = java.parent().and_then(Path::parent)
        && let Some(candidate) = candidate_from_home(home)
        && candidate.java == java
    {
        return Some(candidate);
    }
    Some(JdkCandidate {
        java: java.to_path_buf(),
        major: major_from_running(java)?,
    })
}

fn children(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    // Lexically larger names first, so temurin-21 is tried before temurin-17.
    paths.reverse();
    paths
}

/// Walks the documented search order: `BREWERY_DAEMON_JAVA`, `JAVA_HOME`, `java` on
/// PATH, `~/.jdks/*`, then the platform's usual install directories. Lazy, so slower
/// sources are only read when earlier ones did not qualify.
pub fn find_java(min_major: u32) -> Result<PathBuf> {
    find_java_in(min_major, |name| std::env::var_os(name))
}

/// Like `find_java`, but reads `BREWERY_DAEMON_JAVA`, `JAVA_HOME` and `PATH` from `env`
/// (typically a worktree's login-shell environment, which sees SDKMAN/asdf setups that
/// the process environment of a desktop launch does not).
pub fn find_java_with_env(min_major: u32, env: &HashMap<String, String>) -> Result<PathBuf> {
    find_java_in(min_major, |name| {
        env.get(name)
            .or_else(|| {
                // Windows environment names are case-insensitive (`Path`).
                cfg!(windows)
                    .then(|| env.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)))
                    .flatten()
                    .map(|(_, value)| value)
            })
            .map(OsString::from)
    })
}

fn find_java_in(min_major: u32, var: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf> {
    let mut rejected: Vec<JdkCandidate> = Vec::new();
    let mut consider = |candidate: Option<JdkCandidate>| -> Option<PathBuf> {
        let candidate = candidate?;
        match pick_jdk(vec![candidate.clone()], min_major) {
            Some(picked) => Some(picked.java),
            None => {
                rejected.push(candidate);
                None
            }
        }
    };

    if let Some(value) = var("BREWERY_DAEMON_JAVA").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(value);
        let candidate = if path.is_dir() {
            candidate_from_home(&path)
        } else {
            candidate_from_java_exe(&path)
        };
        if let Some(java) = consider(candidate) {
            return Ok(java);
        }
    }
    if let Some(value) = var("JAVA_HOME").filter(|v| !v.is_empty())
        && let Some(java) = consider(candidate_from_home(Path::new(&value)))
    {
        return Ok(java);
    }
    if let Some(path_var) = var("PATH") {
        // Only the first `java` on PATH is what a shell would run.
        let first = std::env::split_paths(&path_var)
            .map(|dir| dir.join(java_exe_name()))
            .find(|java| java.is_file());
        if let Some(java) = first
            && let Some(java) = consider(candidate_from_java_exe(&java))
        {
            return Ok(java);
        }
    }

    let mut roots = vec![util::paths::home_dir().join(".jdks")];
    if cfg!(windows) {
        roots.push(PathBuf::from(r"C:\Program Files\Eclipse Adoptium"));
        roots.push(PathBuf::from(r"C:\Program Files\Java"));
    } else {
        roots.push(PathBuf::from("/usr/lib/jvm"));
    }
    for root in roots {
        for home in children(&root) {
            if let Some(java) = consider(candidate_from_home(&home)) {
                return Ok(java);
            }
        }
    }

    if rejected.is_empty() {
        bail!(
            "no Java installation found; Brewery needs JDK {min_major} or newer \
            (set JAVA_HOME or BREWERY_DAEMON_JAVA)"
        );
    }
    let found: Vec<String> = rejected
        .iter()
        .map(|c| format!("{} (Java {})", c.java.display(), c.major))
        .collect();
    bail!(
        "no JDK {min_major} or newer found; only: {}",
        found.join(", ")
    )
}

/// Daemon jar: `BREWERY_DAEMON_JAR`, then next to the executable, then (development)
/// the Gradle build output found by walking up from the executable.
pub fn find_daemon_jar() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("BREWERY_DAEMON_JAR").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "BREWERY_DAEMON_JAR points at {}, which is not a file",
            path.display()
        );
    }
    let exe = std::env::current_exe().context("cannot determine the executable path")?;
    let exe_dir = exe.parent().context("executable has no parent directory")?;
    find_daemon_jar_from(exe_dir).context(
        "brewery-daemon.jar not found (set BREWERY_DAEMON_JAR, or build it with \
        `./gradlew shadowJar` in brewery-daemon/)",
    )
}

fn find_daemon_jar_from(exe_dir: &Path) -> Option<PathBuf> {
    let bundled = exe_dir.join("brewery-daemon").join("brewery-daemon.jar");
    if bundled.is_file() {
        return Some(bundled);
    }
    exe_dir.ancestors().find_map(|dir| {
        let dev = dir
            .join("brewery-daemon")
            .join("build")
            .join("libs")
            .join("brewery-daemon-all.jar");
        dev.is_file().then_some(dev)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_file_versions() {
        assert_eq!(
            parse_release_version("JAVA_VERSION=\"21.0.11\"\n"),
            Some(21)
        );
        assert_eq!(
            parse_release_version("IMPLEMENTOR=\"x\"\nJAVA_VERSION=\"1.8.0_402\"\n"),
            Some(8)
        );
        assert_eq!(parse_release_version("JAVA_VERSION=\"17\""), Some(17));
        assert_eq!(parse_release_version("garbage"), None);
        assert_eq!(parse_release_version("JAVA_VERSION=\"abc\""), None);
    }

    #[test]
    fn parses_java_version_output() {
        let output = "openjdk version \"21.0.11\" 2026-04-21 LTS\nOpenJDK Runtime Environment";
        assert_eq!(parse_java_version_output(output), Some(21));
        assert_eq!(
            parse_java_version_output("java version \"1.8.0_402\""),
            Some(8)
        );
        assert_eq!(parse_java_version_output("nothing"), None);
    }

    #[test]
    fn picks_first_candidate_that_is_new_enough() {
        let candidate = |path: &str, major| JdkCandidate {
            java: PathBuf::from(path),
            major,
        };
        let picked = pick_jdk(
            vec![candidate("a", 17), candidate("b", 21), candidate("c", 25)],
            21,
        );
        assert_eq!(picked, Some(candidate("b", 21)));
        assert_eq!(pick_jdk(vec![candidate("a", 17)], 21), None);
        assert_eq!(pick_jdk(Vec::new(), 21), None);
    }

    /// A fake JDK home: `release` makes `candidate_from_home` skip running `java`.
    fn fake_jdk(root: &Path, name: &str, version: &str) -> PathBuf {
        let home = root.join(name);
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(home.join("bin").join(java_exe_name()), b"").unwrap();
        std::fs::write(
            home.join("release"),
            format!(
                "JAVA_VERSION=\"{version}\"
"
            ),
        )
        .unwrap();
        home
    }

    #[test]
    fn explicit_environment_supplies_java_home() {
        let root = std::env::temp_dir().join(format!("brewery-env-jdk-{}", std::process::id()));
        let home = fake_jdk(&root, "jdk-21", "21.0.1");
        let env = HashMap::from([("JAVA_HOME".to_string(), home.to_string_lossy().into_owned())]);
        assert_eq!(
            find_java_with_env(21, &env).unwrap(),
            home.join("bin").join(java_exe_name())
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn explicit_environment_supplies_path() {
        let root = std::env::temp_dir().join(format!("brewery-env-path-{}", std::process::id()));
        let home = fake_jdk(&root, "jdk-22", "22.0.0");
        let path = std::env::join_paths([home.join("bin")]).unwrap();
        let env = HashMap::from([("PATH".to_string(), path.to_string_lossy().into_owned())]);
        assert_eq!(
            find_java_with_env(21, &env).unwrap(),
            home.join("bin").join(java_exe_name())
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn jar_lookup_prefers_bundled_then_walks_up_to_dev_build() {
        let root = std::env::temp_dir().join(format!("brewery-jar-test-{}", std::process::id()));
        let exe_dir = root.join("target").join("debug");
        std::fs::create_dir_all(&exe_dir).unwrap();
        assert_eq!(find_daemon_jar_from(&exe_dir), None);

        let dev = root.join("brewery-daemon").join("build").join("libs");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("brewery-daemon-all.jar"), b"").unwrap();
        assert_eq!(
            find_daemon_jar_from(&exe_dir),
            Some(dev.join("brewery-daemon-all.jar"))
        );

        let bundled = exe_dir.join("brewery-daemon");
        std::fs::create_dir_all(&bundled).unwrap();
        std::fs::write(bundled.join("brewery-daemon.jar"), b"").unwrap();
        assert_eq!(
            find_daemon_jar_from(&exe_dir),
            Some(bundled.join("brewery-daemon.jar"))
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

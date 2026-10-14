//! Brewery's half of the update helper: the file layout it swaps, the strings it shows, and
//! stopping the bundled Java processes (daemon, jdtls, Gradle daemons started from the bundled
//! JRE) that would otherwise keep `jre\` and the jars locked.

use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
        Storage::FileSystem::GetLongPathNameW,
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, QueryFullProcessImageNameW,
                TerminateProcess, WaitForSingleObject,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

use crate::updater::Job;

pub(crate) const APP_EXE: &str = "Brewery.exe";
pub(crate) const DIALOG_CLASS: &str = "Brewery-Auto-Updater-Dialog-Class";
pub(crate) const DIALOG_TITLE: &str = "Brewery";
pub(crate) const UPDATING_TEXT: &str = "Updating Brewery...";
pub(crate) const ERROR_TITLE: &str = "Error: Brewery update failed.";

/// Directories of the offline bundle (see `brewery_bundle`), swapped as a whole.
pub(crate) const BUNDLED_DIRS: [&str; 4] = ["jre", "brewery-daemon", "jdtls", "extensions"];

/// How long the app and its children get to exit on their own before bundled ones are killed.
pub(crate) const PROCESS_GRACE: Duration = Duration::from_secs(5);
/// How long to wait for a killed process to be gone.
pub(crate) const PROCESS_KILL_WAIT: Duration = Duration::from_secs(3);

/// The update in place. The installer, run with `/update=true`, has written the new files
/// under `install\` (and `tools\`, `licenses\` directly); the app has quit. Every job is
/// undone in reverse order if a later one fails, so nothing here deletes before the end.
pub(crate) fn jobs() -> Vec<Job> {
    fn p(value: &str) -> &Path {
        Path::new(value)
    }
    vec![
        // An `old\` left by an update whose cleanup failed would block the directory moves.
        remove_stale_dir(p("old")),
        // Move old files
        // Not deleting because installing new files can fail
        Job::mkdir(p("old")),
        Job::move_file(p("Brewery.exe"), p("old\\Brewery.exe")),
        Job::mkdir(p("old\\bin")),
        Job::move_file(p("bin\\brewery.exe"), p("old\\bin\\brewery.exe")),
        Job::move_file(p("bin\\brewery"), p("old\\bin\\brewery")),
        Job::mkdir(p("old\\x64")),
        Job::mkdir(p("old\\arm64")),
        Job::move_if_exists(p("x64\\OpenConsole.exe"), p("old\\x64\\OpenConsole.exe")),
        Job::move_if_exists(
            p("arm64\\OpenConsole.exe"),
            p("old\\arm64\\OpenConsole.exe"),
        ),
        Job::move_file(p("conpty.dll"), p("old\\conpty.dll")),
        // The bundle directories: optional in the old install (it may predate the bundle).
        Job::move_if_exists(p("jre"), p("old\\jre")),
        Job::move_if_exists(p("brewery-daemon"), p("old\\brewery-daemon")),
        Job::move_if_exists(p("jdtls"), p("old\\jdtls")),
        Job::move_if_exists(p("extensions"), p("old\\extensions")),
        // Copy new files
        Job::move_file(p("install\\Brewery.exe"), p("Brewery.exe")),
        Job::move_file(p("install\\bin\\brewery.exe"), p("bin\\brewery.exe")),
        Job::move_file(p("install\\bin\\brewery"), p("bin\\brewery")),
        Job::mkdir_if_exists(p("x64"), p("install\\x64")),
        Job::mkdir_if_exists(p("arm64"), p("install\\arm64")),
        Job::move_if_exists(
            p("install\\x64\\OpenConsole.exe"),
            p("x64\\OpenConsole.exe"),
        ),
        Job::move_if_exists(
            p("install\\arm64\\OpenConsole.exe"),
            p("arm64\\OpenConsole.exe"),
        ),
        Job::move_file(p("install\\conpty.dll"), p("conpty.dll")),
        // Required in the new install: a release always ships the whole bundle.
        Job::move_file(p("install\\jre"), p("jre")),
        Job::move_file(p("install\\brewery-daemon"), p("brewery-daemon")),
        Job::move_file(p("install\\jdtls"), p("jdtls")),
        Job::move_file(p("install\\extensions"), p("extensions")),
        // Cleanup installer and updates folder
        Job::rmdir_nofail(p("updates")),
        Job::rmdir_nofail(p("install")),
        // Cleanup old installation
        Job::rmdir_nofail(p("old")),
    ]
}

/// After a rolled-back update, drops the installer's "ready" flag so the app does not run the
/// helper again on every quit; the next update check downloads a fresh installer.
pub(crate) fn forget_failed_update(app_dir: &Path) {
    let flag = app_dir.join("updates").join("versions.txt");
    if let Err(error) = std::fs::remove_file(&flag)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("Failed to remove {}: {error}", flag.display());
    }
}

/// Removes `name` if present; nothing to undo, since it only held a previous version.
fn remove_stale_dir(name: &'static Path) -> Job {
    Job {
        apply: Box::new(move |app_dir| {
            let dir = app_dir.join(name);
            if dir.exists() {
                log::info!("Removing stale directory: {}", dir.display());
                std::fs::remove_dir_all(&dir)
                    .context(format!("Failed to remove directory {}", dir.display()))?;
            }
            Ok(())
        }),
        rollback: Box::new(|_| Ok(())),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessKind {
    /// The app, its CLI or console helpers: waited for, never killed.
    App,
    /// A process started from a bundled directory (`jre\bin\java.exe`, ...): killed after the grace period.
    Bundled,
}

/// Brewery's own executables (lowercase, relative to the install dir, optionally under `old\`
/// or `install\`): waited for, never killed.
const APP_FILES: [&str; 5] = [
    "brewery.exe",
    r"bin\brewery.exe",
    r"x64\openconsole.exe",
    r"arm64\openconsole.exe",
    r"tools\auto_update_helper.exe",
];

fn normalize(path: &Path) -> String {
    let path = path.to_string_lossy().replace('/', "\\");
    let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(&path).to_string()
    };
    path.trim_end_matches('\\').to_lowercase()
}

/// Whether a process with this image path belongs to the installation in `app_dir`, and how
/// the helper treats it. Only Brewery's own top-level entries count, because the install dir
/// may be shared (a drive root, `C:\Apps`): `Bundled` needs a bundled dir as the first
/// component (or right after `old\` / `install\`), `App` is one of `APP_FILES`.
/// Everything else, including the user's own JDKs, is `None` and never touched.
pub(crate) fn classify(image: &Path, app_dir: &Path) -> Option<ProcessKind> {
    let image = normalize(image);
    let app_dir = normalize(app_dir);
    let relative = image.strip_prefix(&app_dir)?.strip_prefix('\\')?;
    let relative = ["old\\", "install\\"]
        .iter()
        .find_map(|prefix| relative.strip_prefix(prefix))
        .unwrap_or(relative);
    let (first, rest) = relative.split_once('\\').unwrap_or(("", relative));
    if BUNDLED_DIRS.contains(&first) && !rest.is_empty() {
        Some(ProcessKind::Bundled)
    } else if APP_FILES.contains(&relative) {
        Some(ProcessKind::App)
    } else {
        None
    }
}

/// `classify` for a running process. The image path keeps the spelling the process was started
/// with, which can contain 8.3 short names (`C:\Users\RUNNER~1\...`) while `app_dir` is
/// canonical (long), so a short-looking image is expanded with `GetLongPathNameW` first.
fn classify_image(image: &Path, app_dir: &Path) -> Option<ProcessKind> {
    if !image.to_string_lossy().contains('~') {
        return classify(image, app_dir);
    }
    long_path(image)
        .and_then(|long| classify(&long, app_dir))
        .or_else(|| classify(image, app_dir))
}

fn long_path(path: &Path) -> Option<PathBuf> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buffer = vec![0u16; 32 * 1024];
    let len = unsafe { GetLongPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buffer)) } as usize;
    (len > 0 && len < buffer.len()).then(|| PathBuf::from(OsString::from_wide(&buffer[..len])))
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) }.ok();
    }
}

struct AppProcess {
    pid: u32,
    image: String,
    kind: ProcessKind,
    handle: OwnedHandle,
}

fn image_path(process: &OwnedHandle) -> Option<OsString> {
    let mut buffer = vec![0u16; 32 * 1024];
    let mut len = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR::from_raw(buffer.as_mut_ptr()),
            &mut len,
        )
    }
    .ok()?;
    Some(OsString::from_wide(&buffer[..len as usize]))
}

/// The other processes running from `app_dir`. Each is opened once and its image path is read
/// through that handle, so a reused pid can never make us kill something else.
fn app_processes(app_dir: &Path) -> Vec<AppProcess> {
    let mut found = Vec::new();
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        log::warn!("Could not list processes");
        return found;
    };
    let snapshot = OwnedHandle(snapshot);
    let own_pid = std::process::id();
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_ok();
    while more {
        let pid = entry.th32ProcessID;
        if pid != 0 && pid != own_pid {
            let full = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE;
            let query_only = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE;
            let opened = unsafe { OpenProcess(full, false, pid) }
                .or_else(|_| unsafe { OpenProcess(query_only, false, pid) });
            if let Ok(handle) = opened {
                let handle = OwnedHandle(handle);
                if let Some(image) = image_path(&handle)
                    && let Some(kind) = classify_image(Path::new(&image), app_dir)
                {
                    found.push(AppProcess {
                        pid,
                        image: image.to_string_lossy().into_owned(),
                        kind,
                        handle,
                    });
                }
            }
        }
        more = unsafe { Process32NextW(snapshot.0, &mut entry) }.is_ok();
    }
    found
}

/// Waits up to `grace` for every process running from `app_dir` to exit (the app quits first,
/// then the daemon sees EOF on stdin and jdtls notices its parent is gone). Whatever still runs
/// from a bundled directory after that (typically a Gradle daemon started with the bundled JRE,
/// which outlives the app by design) is terminated, since it locks `jre\` and the jars. The app
/// itself and its CLI are never killed; the job retries and the rollback deal with them.
pub(crate) fn stop_bundled_processes(app_dir: &Path, grace: Duration, kill_wait: Duration) {
    // Not a Brewery install (or the helper was started from somewhere odd): kill nothing.
    if !app_dir.join(APP_EXE).is_file() {
        log::warn!(
            "{} has no {APP_EXE}, not stopping any processes",
            app_dir.display()
        );
        return;
    }
    // Resolve junctions and short names once; `classify_image` expands short image paths.
    let canonical = std::fs::canonicalize(app_dir).ok();
    let app_dir = canonical.as_deref().unwrap_or(app_dir);
    let deadline = Instant::now() + grace;
    let remaining = loop {
        let processes = app_processes(app_dir);
        if processes.is_empty() {
            return;
        }
        if Instant::now() >= deadline {
            break processes;
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    for process in remaining {
        match process.kind {
            ProcessKind::App => {
                log::warn!(
                    "Still running after {grace:?}: {} (pid {})",
                    process.image,
                    process.pid
                );
            }
            ProcessKind::Bundled => {
                log::info!("Stopping {} (pid {})", process.image, process.pid);
                if let Err(error) = unsafe { TerminateProcess(process.handle.0, 1) } {
                    log::warn!("Failed to stop pid {}: {error}", process.pid);
                    continue;
                }
                let wait =
                    unsafe { WaitForSingleObject(process.handle.0, kill_wait.as_millis() as u32) };
                if wait != WAIT_OBJECT_0 {
                    log::warn!("pid {} did not exit within {kill_wait:?}", process.pid);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        process::{Child, Stdio},
        time::Duration,
    };

    use super::*;
    use crate::updater::perform_update_with;

    const OLD_FILES: [&str; 11] = [
        "Brewery.exe",
        "bin\\brewery.exe",
        "bin\\brewery",
        "conpty.dll",
        "x64\\OpenConsole.exe",
        "jre\\release",
        "jre\\bin\\java.exe",
        "brewery-daemon\\brewery-daemon.jar",
        "jdtls\\plugins\\org.eclipse.equinox.launcher.jar",
        "extensions\\java\\extension.toml",
        "tools\\auto_update_helper.exe",
    ];

    /// What M6d's installer writes in update mode (`/update=true`).
    const INSTALLED_FILES: [&str; 10] = [
        "install\\Brewery.exe",
        "install\\bin\\brewery.exe",
        "install\\bin\\brewery",
        "install\\conpty.dll",
        "install\\x64\\OpenConsole.exe",
        "install\\jre\\release",
        "install\\jre\\bin\\java.exe",
        "install\\brewery-daemon\\brewery-daemon.jar",
        "install\\jdtls\\plugins\\org.eclipse.equinox.launcher.jar",
        "install\\extensions\\java\\extension.toml",
    ];

    fn write(root: &Path, files: &[&str], content: &str) {
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
    }

    fn read(root: &Path, file: &str) -> Option<String> {
        std::fs::read_to_string(root.join(file)).ok()
    }

    fn app_files() -> Vec<&'static str> {
        OLD_FILES
            .iter()
            .copied()
            .filter(|file| *file != "tools\\auto_update_helper.exe")
            .collect()
    }

    #[test]
    fn test_perform_update_swaps_brewery_tree() {
        let app_dir = tempfile::tempdir().unwrap();
        let app_dir = app_dir.path();
        write(app_dir, &OLD_FILES, "old");
        write(app_dir, &INSTALLED_FILES, "new");
        write(app_dir, &["updates\\versions.txt"], "0.1.1");

        perform_update_with(&jobs(), app_dir, None, false, &[]).unwrap();

        for file in app_files() {
            assert_eq!(read(app_dir, file).as_deref(), Some("new"), "{file}");
        }
        // tools\ is written in place by the installer, the helper leaves it alone.
        assert_eq!(
            read(app_dir, "tools\\auto_update_helper.exe").as_deref(),
            Some("old")
        );
        for dir in ["install", "updates", "old"] {
            assert!(!app_dir.join(dir).exists(), "{dir} should be removed");
        }
    }

    #[test]
    fn test_perform_update_clears_stale_old_dir() {
        let app_dir = tempfile::tempdir().unwrap();
        let app_dir = app_dir.path();
        write(app_dir, &OLD_FILES, "old");
        write(app_dir, &INSTALLED_FILES, "new");
        // Left behind by an earlier update whose cleanup failed.
        write(app_dir, &["old\\jre\\release", "old\\Brewery.exe"], "stale");

        perform_update_with(&jobs(), app_dir, None, false, &[]).unwrap();

        assert_eq!(read(app_dir, "jre\\release").as_deref(), Some("new"));
        assert!(!app_dir.join("old").exists());
    }

    #[test]
    fn test_perform_update_rolls_back_brewery_tree() {
        let app_dir = tempfile::tempdir().unwrap();
        let app_dir = app_dir.path();
        write(app_dir, &OLD_FILES, "old");
        // The installer output lacks extensions\: the last move fails and everything is undone.
        let partial: Vec<&str> = INSTALLED_FILES
            .iter()
            .copied()
            .filter(|file| !file.starts_with("install\\extensions"))
            .collect();
        write(app_dir, &partial, "new");
        write(app_dir, &["updates\\versions.txt"], "0.1.1");

        let error = perform_update_with(&jobs(), app_dir, None, false, &[]).unwrap_err();
        // The flag is gone, so the next quit does not retry (and fail) the same update.
        assert!(!app_dir.join("updates\\versions.txt").exists());
        assert_eq!(error.to_string(), "Autoupdate failed, rollback successful");

        for file in OLD_FILES {
            assert_eq!(read(app_dir, file).as_deref(), Some("old"), "{file}");
        }
        for file in partial {
            assert_eq!(read(app_dir, file).as_deref(), Some("new"), "{file}");
        }
        assert!(!app_dir.join("old").exists());
    }

    #[test]
    fn test_bundle_without_optional_old_dirs_still_updates() {
        // An install from before the bundle existed has no jre\ etc.; the new one brings them.
        let app_dir = tempfile::tempdir().unwrap();
        let app_dir = app_dir.path();
        write(
            app_dir,
            &[
                "Brewery.exe",
                "bin\\brewery.exe",
                "bin\\brewery",
                "conpty.dll",
            ],
            "old",
        );
        write(app_dir, &INSTALLED_FILES, "new");

        perform_update_with(&jobs(), app_dir, None, false, &[]).unwrap();

        for file in app_files() {
            assert_eq!(read(app_dir, file).as_deref(), Some("new"), "{file}");
        }
    }

    #[test]
    fn test_strings_say_brewery() {
        assert_eq!(APP_EXE, "Brewery.exe");
        for text in [DIALOG_CLASS, DIALOG_TITLE, UPDATING_TEXT, ERROR_TITLE] {
            assert!(text.contains("Brewery"), "{text}");
            assert!(!text.contains("Zed"), "{text}");
        }
        assert_eq!(UPDATING_TEXT, "Updating Brewery...");
        assert_eq!(ERROR_TITLE, "Error: Brewery update failed.");
    }

    #[test]
    fn test_classify_processes() {
        let app = Path::new(r"C:\Users\me\AppData\Local\Programs\Brewery");
        let cases: [(&str, Option<ProcessKind>); 17] = [
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\jre\bin\java.exe",
                Some(ProcessKind::Bundled),
            ),
            (
                r"c:\users\ME\appdata\local\programs\brewery\JRE\bin\javaw.exe",
                Some(ProcessKind::Bundled),
            ),
            (
                r"\\?\C:\Users\me\AppData\Local\Programs\Brewery\jre\bin\java.exe",
                Some(ProcessKind::Bundled),
            ),
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\jdtls\bin\proxy.exe",
                Some(ProcessKind::Bundled),
            ),
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\extensions\java\proxy.exe",
                Some(ProcessKind::Bundled),
            ),
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\old\jre\bin\java.exe",
                Some(ProcessKind::Bundled),
            ),
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\Brewery.exe",
                Some(ProcessKind::App),
            ),
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\bin\brewery.exe",
                Some(ProcessKind::App),
            ),
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\install\jdtls\bin\proxy.exe",
                Some(ProcessKind::Bundled),
            ),
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\tools\auto_update_helper.exe",
                Some(ProcessKind::App),
            ),
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\x64\OpenConsole.exe",
                Some(ProcessKind::App),
            ),
            // A file named like a bundled dir is not inside it.
            (r"C:\Users\me\AppData\Local\Programs\Brewery\jre", None),
            // A bundled dir name deeper down is somebody else's (the install dir may be shared).
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\something\jre\bin\java.exe",
                None,
            ),
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\old\x\jre\bin\java.exe",
                None,
            ),
            // Unknown files under the install dir are not waited for either.
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery\other.exe",
                None,
            ),
            // The user's own JDK, and a sibling directory sharing the prefix.
            (r"C:\Users\me\.jdks\temurin-21\bin\java.exe", None),
            (
                r"C:\Users\me\AppData\Local\Programs\Brewery Preview\jre\bin\java.exe",
                None,
            ),
        ];
        for (image, expected) in cases {
            assert_eq!(classify(Path::new(image), app), expected, "{image}");
        }
        // A trailing separator on the app dir changes nothing.
        assert_eq!(
            classify(
                Path::new(r"C:\Brewery\jre\bin\java.exe"),
                Path::new(r"C:\Brewery\")
            ),
            Some(ProcessKind::Bundled)
        );
    }

    #[test]
    fn test_classify_shared_install_dirs() {
        // Inno lets the user pick a drive root or a shared folder as the install dir.
        let root = Path::new(r"D:\");
        let cases: [(&str, Option<ProcessKind>); 5] = [
            (r"D:\jre\bin\java.exe", Some(ProcessKind::Bundled)),
            (r"D:\Brewery.exe", Some(ProcessKind::App)),
            (r"D:\jdk1.8.0\jre\bin\java.exe", None),
            (r"D:\Tools\something\jdtls\java.exe", None),
            (r"D:\other.exe", None),
        ];
        for (image, expected) in cases {
            assert_eq!(classify(Path::new(image), root), expected, "{image}");
        }
        let apps = Path::new(r"C:\Apps");
        assert_eq!(
            classify(Path::new(r"C:\Apps\jdk8\jre\bin\java.exe"), apps),
            None
        );
        assert_eq!(
            classify(Path::new(r"C:\Apps\jre\bin\java.exe"), apps),
            Some(ProcessKind::Bundled)
        );
        // Long-path and UNC forms match their plain spelling.
        assert_eq!(
            classify(
                Path::new(r"\\?\UNC\server\share\Brewery\jre\bin\java.exe"),
                Path::new(r"\\server\share\Brewery")
            ),
            Some(ProcessKind::Bundled)
        );
    }

    struct Running(Child);

    impl Drop for Running {
        fn drop(&mut self) {
            self.0.kill().ok();
            self.0.wait().ok();
        }
    }

    /// Starts a copy of ping.exe at `exe`: a long-running process with that image path.
    #[allow(
        clippy::disallowed_methods,
        reason = "test helper, not the main binary"
    )]
    fn spawn_copy_at(exe: PathBuf) -> Running {
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        std::fs::copy(Path::new(&system).join(r"System32\PING.EXE"), &exe).unwrap();
        Running(
            std::process::Command::new(&exe)
                .args(["-n", "60", "127.0.0.1"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    #[test]
    fn test_stop_bundled_processes_kills_only_bundled_java() {
        let root = tempfile::tempdir().unwrap();
        let app_dir = root.path().join("Brewery");
        let mut bundled = spawn_copy_at(app_dir.join(r"jre\bin\java.exe"));
        let mut app = spawn_copy_at(app_dir.join("Brewery.exe"));
        let mut unrelated = spawn_copy_at(root.path().join(r"jdk\bin\java.exe"));

        stop_bundled_processes(&app_dir, Duration::from_millis(300), Duration::from_secs(5));

        assert!(
            bundled.0.try_wait().unwrap().is_some(),
            "bundled java should be stopped"
        );
        assert!(
            app.0.try_wait().unwrap().is_none(),
            "the app is never killed"
        );
        assert!(
            unrelated.0.try_wait().unwrap().is_none(),
            "java outside the app dir is never killed"
        );
    }

    /// The 8.3 form of an existing path, or None when the volume has short names disabled.
    fn short_path(path: &Path) -> Option<PathBuf> {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut buffer = vec![0u16; 32 * 1024];
        let len = unsafe {
            windows::Win32::Storage::FileSystem::GetShortPathNameW(
                windows::core::PCWSTR(wide.as_ptr()),
                Some(&mut buffer),
            )
        } as usize;
        (len > 0 && len < buffer.len())
            .then(|| PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len])))
            .filter(|short| short != path)
    }

    /// Like the GitHub runner, whose %TEMP% is C:\Users\RUNNER~1\...: the process is started
    /// through one spelling of the install dir and the helper is given the other.
    fn assert_stops_across_spellings(start_short: bool) {
        let root = tempfile::tempdir().unwrap();
        let long_dir = root.path().join("Brewery Install Directory");
        std::fs::create_dir_all(long_dir.join(r"jre\bin")).unwrap();
        let Some(short_dir) = short_path(&long_dir) else {
            eprintln!("8.3 names are disabled on this volume, skipping");
            return;
        };
        let (start_dir, helper_dir) = if start_short {
            (&short_dir, &long_dir)
        } else {
            (&long_dir, &short_dir)
        };
        let mut bundled = spawn_copy_at(start_dir.join(r"jre\bin\java.exe"));
        let _app = spawn_copy_at(start_dir.join("Brewery.exe"));

        stop_bundled_processes(
            helper_dir,
            Duration::from_millis(300),
            Duration::from_secs(5),
        );

        assert!(
            bundled.0.try_wait().unwrap().is_some(),
            "bundled java started via {} should be stopped by a helper given {}",
            start_dir.display(),
            helper_dir.display()
        );
    }

    #[test]
    fn test_stop_bundled_processes_started_via_short_path() {
        assert_stops_across_spellings(true);
    }

    #[test]
    fn test_stop_bundled_processes_started_via_long_path() {
        assert_stops_across_spellings(false);
    }

    #[test]
    fn test_stop_bundled_processes_kills_nothing_without_brewery_exe() {
        // A directory that is not a Brewery install (no Brewery.exe): nothing is killed.
        let root = tempfile::tempdir().unwrap();
        let app_dir = root.path().join("NotBrewery");
        let mut java = spawn_copy_at(app_dir.join(r"jre\bin\java.exe"));

        stop_bundled_processes(&app_dir, Duration::from_millis(300), Duration::from_secs(5));

        assert!(java.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn test_stop_bundled_processes_returns_when_nothing_runs() {
        let root = tempfile::tempdir().unwrap();
        let start = std::time::Instant::now();
        stop_bundled_processes(root.path(), Duration::from_secs(10), Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}

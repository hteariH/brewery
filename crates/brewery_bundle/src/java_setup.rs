//! Startup wiring for the bundled Java tools: seeds the bundled Java extension into Zed's
//! installed-extensions dir and layers `lsp.jdtls.settings` defaults that point the
//! extension at the bundled jdtls launcher, Lombok, java-debug and java-lsp-proxy.
//! Without a bundle (dev builds) nothing changes.

use std::path::{Path, PathBuf};

use gpui::{App, BorrowAppContext as _};
use serde_json::{Map, Value};
use settings::{SettingsStore, merge_from::MergeFrom as _};

/// The language server whose settings the overlay fills.
pub const JDTLS_SERVER: &str = "jdtls";

/// Extensions that ship in `<root>/extensions/` and are seeded on first start.
pub const SEEDED_EXTENSIONS: &[&str] = &["java"];

/// `lsp.jdtls.settings` defaults for the bundle at `root`; empty without a bundle. Each key
/// is set only when its piece is present, so a partial bundle gives a partial overlay.
pub fn java_settings_overlay_for_root(root: Option<&Path>) -> Map<String, Value> {
    let mut overlay = Map::new();
    let Some(root) = root else {
        return overlay;
    };
    let file = |path: PathBuf| path.is_file().then_some(path);
    let jre = root.join("jre");
    let jre = jre.join("release").is_file().then_some(jre);
    let launcher = file(root.join("jdtls").join(if cfg!(windows) {
        "brewery-jdtls.cmd"
    } else {
        "brewery-jdtls"
    }));
    let proxy = file(root.join(if cfg!(windows) {
        "java-lsp-proxy.exe"
    } else {
        "java-lsp-proxy"
    }));

    let mut set_path = |key: &str, path: Option<PathBuf>| {
        let Some(path) = path else { return false };
        match path.to_str() {
            Some(path) => {
                overlay.insert(key.to_string(), Value::String(path.to_string()));
                true
            }
            None => {
                log::warn!("brewery: bundled {key} path is not UTF-8: {path:?}");
                false
            }
        }
    };
    let mut any = false;
    // Both go through `cmd /C` on Windows (the launcher, and Lombok as its `--jvm-arg`).
    any |= set_path("jdtls_launcher", launcher.as_deref().map(cmd_safe_path));
    any |= set_path("lombok_jar", file(root.join("lombok.jar")).as_deref().map(cmd_safe_path));
    any |= set_path("java_debug_jar", file(root.join("java-debug.jar")));
    any |= set_path("lsp_proxy_path", proxy);
    // The launcher prefers the bundled JRE by itself. `java_home` would also reach jdtls as
    // JAVA_HOME, making the stripped JRE the default project runtime, so it is set only
    // when the extension has to run its managed jdtls on the bundled JRE.
    let java_home = if launcher.is_none() { jre.clone() } else { None };
    any |= set_path("java_home", java_home);
    if jre.is_some() {
        overlay.insert("jdk_auto_download".to_string(), Value::Bool(false));
        any = true;
    }
    if any {
        overlay.insert("check_updates".to_string(), Value::String("once".to_string()));
    }
    overlay
}

/// [`java_settings_overlay_for_root`] for the running executable's bundle.
pub fn java_settings_overlay() -> Map<String, Value> {
    java_settings_overlay_for_root(crate::bundle_root().as_deref())
}

/// Merges `overlay` into the default `lsp.jdtls.settings`. User, project and extension
/// settings are higher layers, so they still win key by key.
pub fn apply_jdtls_defaults(overlay: Map<String, Value>, cx: &mut App) {
    if overlay.is_empty() {
        return;
    }
    log::info!(
        "brewery: bundled Java tools: {}",
        overlay.keys().cloned().collect::<Vec<_>>().join(", ")
    );
    let overlay = Value::Object(overlay);
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |defaults| {
            let jdtls = defaults.project.lsp.0.entry(JDTLS_SERVER.into()).or_default();
            match &mut jdtls.settings {
                Some(settings) => settings.merge_from(&overlay),
                None => jdtls.settings = Some(overlay),
            }
        });
    });
}

/// Copies `<bundled>` to `<installed_dir>/<id>` unless that already exists. Returns
/// whether it copied. The copy goes to a temp dir first and is renamed into place, so a
/// crash never leaves a half-seeded extension behind.
pub fn seed_extension(bundled: &Path, installed_dir: &Path, id: &str) -> std::io::Result<bool> {
    let target = installed_dir.join(id);
    if target.symlink_metadata().is_ok() {
        return Ok(false);
    }
    std::fs::create_dir_all(installed_dir)?;
    let staging = installed_dir.join(format!(".{id}.brewery-seed-{}", std::process::id()));
    std::fs::remove_dir_all(&staging).ok();
    let result = copy_dir(bundled, &staging).and_then(|()| std::fs::rename(&staging, &target));
    if let Err(error) = result {
        std::fs::remove_dir_all(&staging).ok();
        // Another Brewery instance may have seeded it in the meantime.
        if target.is_dir() {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(true)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let destination = to.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), &destination)?;
        }
    }
    Ok(())
}

/// Seeds every bundled extension in [`SEEDED_EXTENSIONS`] into `extensions_dir/installed`.
/// Must run before `extension_host` loads its index. Failures are logged, not fatal.
pub fn seed_bundled_extensions(extensions_dir: &Path) {
    let installed_dir = extensions_dir.join("installed");
    for id in SEEDED_EXTENSIONS {
        let Some(bundled) = crate::bundled_extension(id) else {
            continue;
        };
        match seed_extension(&bundled, &installed_dir, id) {
            Ok(true) => log::info!("brewery: seeded the bundled {id} extension"),
            Ok(false) => {}
            Err(error) => log::error!("brewery: could not seed the bundled {id} extension: {error}"),
        }
    }
}

/// A path that survives java-lsp-proxy's `cmd /C "<launcher>" args` on Windows: cmd strips
/// the outer quotes when the launcher path has spaces and a later argument is quoted, so
/// such paths are given as their 8.3 short form. Unchanged elsewhere, for paths without
/// spaces, and (with a warning) when the volume has no short names.
pub fn cmd_safe_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    if path.as_os_str().to_string_lossy().contains(' ') {
        match short_path(path) {
            Some(short) if !short.as_os_str().to_string_lossy().contains(' ') => return short,
            _ => log::warn!(
                "brewery: no 8.3 short name for {path:?}; java-lsp-proxy may fail to start jdtls"
            ),
        }
    }
    path.to_path_buf()
}

#[cfg(windows)]
fn short_path(path: &Path) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
    use windows::Win32::Storage::FileSystem::GetShortPathNameW;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives both calls; the buffer length is passed.
    let needed = unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), None) };
    if needed == 0 {
        return None;
    }
    let mut buffer = vec![0u16; needed as usize];
    let written = unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buffer)) };
    if written == 0 || written as usize >= buffer.len() {
        return None;
    }
    buffer.truncate(written as usize);
    Some(PathBuf::from(OsString::from_wide(&buffer)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("brewery-java-setup-{name}-{}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    fn launcher_name() -> &'static str {
        if cfg!(windows) {
            "brewery-jdtls.cmd"
        } else {
            "brewery-jdtls"
        }
    }

    fn proxy_name() -> &'static str {
        if cfg!(windows) {
            "java-lsp-proxy.exe"
        } else {
            "java-lsp-proxy"
        }
    }

    fn populate(root: &Path) {
        touch(&root.join("jre").join("release"));
        touch(&root.join("jdtls").join(launcher_name()));
        touch(&root.join("lombok.jar"));
        touch(&root.join("java-debug.jar"));
        touch(&root.join(proxy_name()));
    }

    fn string(path: PathBuf) -> Value {
        Value::String(path.to_str().unwrap().to_string())
    }

    #[test]
    fn overlay_is_empty_without_a_bundle() {
        assert!(java_settings_overlay_for_root(None).is_empty());
        let temp = TempDir::new("empty");
        assert!(java_settings_overlay_for_root(Some(&temp.0)).is_empty());
        // A dev build's exe dir has none of the pieces either.
        std::fs::create_dir_all(temp.0.join("jdtls")).unwrap();
        std::fs::create_dir_all(temp.0.join("jre").join("bin")).unwrap();
        assert!(java_settings_overlay_for_root(Some(&temp.0)).is_empty());
    }

    #[test]
    fn full_bundle_points_at_absolute_paths() {
        let temp = TempDir::new("full");
        populate(&temp.0);
        let overlay = java_settings_overlay_for_root(Some(&temp.0));
        let root = &temp.0;
        assert!(root.is_absolute());
        assert_eq!(
            overlay["jdtls_launcher"],
            string(root.join("jdtls").join(launcher_name()))
        );
        assert_eq!(overlay["lombok_jar"], string(root.join("lombok.jar")));
        assert_eq!(overlay["java_debug_jar"], string(root.join("java-debug.jar")));
        assert_eq!(overlay["lsp_proxy_path"], string(root.join(proxy_name())));
        assert_eq!(overlay["jdk_auto_download"], Value::Bool(false));
        assert_eq!(overlay["check_updates"], Value::String("once".into()));
        // The launcher runs on the bundled JRE by itself; `java_home` would also become
        // jdtls's JAVA_HOME and so the default project runtime, so it is left to the user.
        assert!(!overlay.contains_key("java_home"), "{overlay:?}");
        assert_eq!(overlay.len(), 6, "{overlay:?}");
    }

    #[test]
    fn bundle_under_a_path_with_spaces_gives_cmd_safe_launcher_paths() {
        let temp = TempDir::new("spaced");
        let root = temp.0.join("Brewery Install Dir");
        populate(&root);
        let overlay = java_settings_overlay_for_root(Some(&root));
        let as_str = |key: &str| overlay[key].as_str().unwrap().to_string();
        if cfg!(windows) && !short_names_disabled(&temp.0) {
            assert!(!as_str("jdtls_launcher").contains(' '), "{overlay:?}");
            assert!(!as_str("lombok_jar").contains(' '), "{overlay:?}");
            assert!(Path::new(&as_str("jdtls_launcher")).is_file());
        }
        // Zed spawns the proxy directly and java-debug goes through LSP: long paths are fine.
        assert_eq!(overlay["lsp_proxy_path"], string(root.join(proxy_name())));
        assert_eq!(overlay["java_debug_jar"], string(root.join("java-debug.jar")));
    }

    #[test]
    fn partial_bundle_gives_a_partial_overlay() {
        let temp = TempDir::new("partial");
        touch(&temp.0.join("lombok.jar"));
        touch(&temp.0.join(proxy_name()));
        // A launcher directory without the launcher script does not count.
        std::fs::create_dir_all(temp.0.join("jdtls")).unwrap();
        let overlay = java_settings_overlay_for_root(Some(&temp.0));
        assert_eq!(overlay["lombok_jar"], string(temp.0.join("lombok.jar")));
        assert_eq!(overlay["lsp_proxy_path"], string(temp.0.join(proxy_name())));
        assert_eq!(overlay["check_updates"], Value::String("once".into()));
        for absent in ["jdtls_launcher", "java_debug_jar", "java_home", "jdk_auto_download"] {
            assert!(!overlay.contains_key(absent), "{absent}: {overlay:?}");
        }
    }

    #[test]
    fn jre_without_launcher_runs_managed_jdtls_on_it() {
        let temp = TempDir::new("jre-only");
        touch(&temp.0.join("jre").join("release"));
        let overlay = java_settings_overlay_for_root(Some(&temp.0));
        assert_eq!(overlay["java_home"], string(temp.0.join("jre")));
        assert_eq!(overlay["jdk_auto_download"], Value::Bool(false));
        assert!(!overlay.contains_key("jdtls_launcher"));
    }

    fn jdtls_settings(cx: &App) -> Value {
        cx.global::<SettingsStore>().merged_settings().project.lsp.0[JDTLS_SERVER]
            .settings
            .clone()
            .unwrap()
    }

    #[gpui::test]
    fn user_settings_override_the_overlay(cx: &mut TestAppContext) {
        let temp = TempDir::new("user-wins");
        populate(&temp.0);
        let overlay = java_settings_overlay_for_root(Some(&temp.0));
        cx.update(|cx| {
            let store = SettingsStore::new(cx, &settings::default_settings());
            cx.set_global(store);
            apply_jdtls_defaults(overlay, cx);

            let settings = jdtls_settings(cx);
            // The overlay beats default.json's `jdk_auto_download: true`.
            assert_eq!(settings["jdk_auto_download"], Value::Bool(false));
            assert_eq!(settings["lombok_jar"], string(temp.0.join("lombok.jar")));

            cx.update_global::<SettingsStore, _>(|store, cx| {
                store
                    .set_user_settings(
                        r#"{"lsp": {"jdtls": {"settings": {
                            "lombok_jar": "/mine/lombok.jar",
                            "jdk_auto_download": true,
                            "max_memory": "4G"
                        }}}}"#,
                        cx,
                    )
                    .unwrap();
            });
            let settings = jdtls_settings(cx);
            assert_eq!(settings["lombok_jar"], Value::String("/mine/lombok.jar".into()));
            assert_eq!(settings["jdk_auto_download"], Value::Bool(true));
            assert_eq!(settings["max_memory"], Value::String("4G".into()));
            // Keys the user did not set still come from the overlay.
            assert_eq!(settings["java_debug_jar"], string(temp.0.join("java-debug.jar")));
            assert_eq!(settings["check_updates"], Value::String("once".into()));
        });
    }

    #[gpui::test]
    fn empty_overlay_keeps_the_defaults(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = SettingsStore::new(cx, &settings::default_settings());
            cx.set_global(store);
            let before = jdtls_settings(cx);
            apply_jdtls_defaults(Map::new(), cx);
            assert_eq!(jdtls_settings(cx), before);
            assert_eq!(before["jdk_auto_download"], Value::Bool(true));
        });
    }

    fn bundled_extension(root: &Path) -> PathBuf {
        let ext = root.join("bundle").join("extensions").join("java");
        touch(&ext.join("extension.toml"));
        touch(&ext.join("extension.wasm"));
        touch(&ext.join("grammars").join("java.wasm"));
        touch(&ext.join("languages").join("java").join("config.toml"));
        ext
    }

    #[test]
    fn seeding_copies_when_absent() {
        let temp = TempDir::new("seed");
        let bundled = bundled_extension(&temp.0);
        let installed = temp.0.join("extensions").join("installed");
        assert!(seed_extension(&bundled, &installed, "java").unwrap());
        let java = installed.join("java");
        for file in [
            "extension.toml",
            "extension.wasm",
            "grammars/java.wasm",
            "languages/java/config.toml",
        ] {
            assert!(java.join(file).is_file(), "{file}");
        }
        // No temp dirs are left next to it.
        let entries: Vec<_> = std::fs::read_dir(&installed).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn seeding_is_idempotent_and_never_overwrites() {
        let temp = TempDir::new("seed-twice");
        let bundled = bundled_extension(&temp.0);
        let installed = temp.0.join("extensions").join("installed");
        assert!(seed_extension(&bundled, &installed, "java").unwrap());
        // The gallery may update it later; the seed must not undo that.
        std::fs::write(installed.join("java").join("extension.toml"), b"updated").unwrap();
        assert!(!seed_extension(&bundled, &installed, "java").unwrap());
        assert_eq!(
            std::fs::read(installed.join("java").join("extension.toml")).unwrap(),
            b"updated"
        );
    }

    #[test]
    fn seeding_skips_an_existing_install() {
        let temp = TempDir::new("seed-existing");
        let bundled = bundled_extension(&temp.0);
        let installed = temp.0.join("extensions").join("installed");
        touch(&installed.join("java").join("extension.toml"));
        assert!(!seed_extension(&bundled, &installed, "java").unwrap());
        assert!(!installed.join("java").join("extension.wasm").exists());
    }

    #[test]
    fn seed_bundled_extensions_without_a_bundle_does_nothing() {
        let temp = TempDir::new("seed-none");
        // The test exe's dir has no `extensions/java`.
        seed_bundled_extensions(&temp.0);
        assert!(!temp.0.join("installed").join("java").exists());
    }

    #[test]
    fn cmd_safe_path_keeps_paths_without_spaces() {
        let temp = TempDir::new("nospace");
        let file = temp.0.join("brewery-jdtls.cmd");
        touch(&file);
        assert_eq!(cmd_safe_path(&file), file);
    }

    #[test]
    fn cmd_safe_path_removes_spaces_on_windows() {
        let temp = TempDir::new("space");
        let file = temp.0.join("Program Files Like").join("brewery-jdtls.cmd");
        touch(&file);
        let safe = cmd_safe_path(&file);
        if cfg!(windows) && !short_names_disabled(&temp.0) {
            assert!(!safe.to_str().unwrap().contains(' '), "{safe:?}");
            assert_ne!(safe, file);
            // Same file.
            assert!(safe.is_file());
            assert_eq!(std::fs::read(&safe).unwrap(), b"x");
        } else {
            assert_eq!(safe, file);
        }
    }

    /// 8.3 names can be turned off per volume (`fsutil 8dot3name`); then the fallback
    /// keeps the long path, which the test above accepts. Asks cmd's `%~s` independently
    /// of the code under test.
    #[cfg(windows)]
    #[allow(clippy::disallowed_methods, reason = "a test helper; cmd returns at once")]
    fn short_names_disabled(dir: &Path) -> bool {
        use std::os::windows::process::CommandExt as _;
        let probe = dir.join("long name probe directory");
        std::fs::create_dir_all(&probe).unwrap();
        let output = std::process::Command::new("cmd")
            .raw_arg(format!("/C for %I in (\"{}\") do @echo %~sI", probe.display()))
            .output()
            .unwrap();
        std::fs::remove_dir_all(&probe).ok();
        String::from_utf8_lossy(&output.stdout).trim().contains(' ')
    }

    #[cfg(not(windows))]
    fn short_names_disabled(_dir: &Path) -> bool {
        true
    }
}

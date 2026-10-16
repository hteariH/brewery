//! Startup wiring for the bundled Java tools: seeds the bundled Java extension into Zed's
//! installed-extensions dir and layers defaults that point the extension at the bundled
//! jdtls launcher, Lombok, java-debug and java-lsp-proxy (`lsp.jdtls.settings`) and keep
//! the extension at the bundled version (`auto_update_extensions`). Without a bundle (dev
//! builds) nothing changes.

use std::path::{Path, PathBuf};

use gpui::{App, BorrowAppContext as _};
use serde_json::{Map, Value};
use settings::{SettingsStore, merge_from::MergeFrom as _};

/// The language server whose settings the overlay fills.
pub const JDTLS_SERVER: &str = "jdtls";

/// The extension the jdtls overlay is built for.
pub const JAVA_EXTENSION: &str = "java";

/// Extensions that ship in `<root>/extensions/` and are seeded on first start.
pub const SEEDED_EXTENSIONS: &[&str] = &[JAVA_EXTENSION];

/// Where the bundle keeps the extension's helpers: inside `jdtls/`, one of the directories
/// the Windows update helper swaps, so they are updated together with jdtls.
fn tools_dir(root: &Path) -> PathBuf {
    root.join("jdtls").join("tools")
}

/// `lsp.jdtls.settings` defaults for the bundle at `root`; empty without a bundle. Each key
/// is set only when its piece is present, so a partial bundle gives a partial overlay.
pub fn java_settings_overlay_for_root(root: Option<&Path>) -> Map<String, Value> {
    overlay_with(root, &cmd_safe_path)
}

/// `cmd_safe` gives the form of a path that may cross `cmd /C`, or `None` when there is
/// none (a spaced path on a volume without 8.3 names).
fn overlay_with(
    root: Option<&Path>,
    cmd_safe: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Map<String, Value> {
    let mut overlay = Map::new();
    let Some(root) = root else {
        return overlay;
    };
    let file = |path: PathBuf| path.is_file().then_some(path);
    let tools = tools_dir(root);
    let jre = root.join("jre");
    let jre = jre.join("release").is_file().then_some(jre);
    let launcher = file(root.join("jdtls").join(if cfg!(windows) {
        "brewery-jdtls.cmd"
    } else {
        "brewery-jdtls"
    }));
    let proxy = file(tools.join(if cfg!(windows) {
        "java-lsp-proxy.exe"
    } else {
        "java-lsp-proxy"
    }));
    let lombok = file(tools.join("lombok.jar"));

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
    // Both go through `cmd /C` on Windows: the launcher, and Lombok as its `--jvm-arg`.
    let mut lombok_off = false;
    if let Some(launcher) = &launcher {
        let safe = cmd_safe(launcher);
        // A spaced launcher path still works as long as no later argument is quoted, so
        // without a short name jdtls starts without the (quoted) Lombok agent.
        lombok_off = safe.is_none() && lombok.is_some();
        any |= set_path("jdtls_launcher", Some(safe.unwrap_or_else(|| launcher.clone())));
    }
    if !lombok_off && let Some(lombok) = &lombok {
        any |= set_path("lombok_jar", Some(cmd_safe(lombok).unwrap_or_else(|| lombok.clone())));
    }
    any |= set_path("java_debug_jar", file(tools.join("java-debug.jar")));
    any |= set_path("lsp_proxy_path", proxy);
    // The launcher prefers the bundled JRE by itself. `java_home` would also reach jdtls as
    // JAVA_HOME, making the stripped JRE the default project runtime, so it is set only
    // when the extension has to run its managed jdtls on the bundled JRE.
    let java_home = if launcher.is_none() { jre.clone() } else { None };
    any |= set_path("java_home", java_home);
    if lombok_off {
        log::warn!("brewery: Lombok is off: the install path has spaces and no 8.3 short name");
        overlay.insert("lombok_support".to_string(), Value::Bool(false));
    }
    if jre.is_some() {
        overlay.insert("jdk_auto_download".to_string(), Value::Bool(false));
        any = true;
    }
    if any {
        overlay.insert("check_updates".to_string(), Value::String("once".to_string()));
    }
    overlay
}

/// `java.import.gradle.java.home` with the bundled launcher: jdtls expands `${...}` in it
/// from its system properties, and the launcher sets `brewery.gradle.java.home` to the
/// user's JAVA_HOME when the project's Gradle wrapper can run on it, else to "" (then
/// jdtls picks a JDK itself). Without it Gradle runs on the bundled JRE 21, which Gradle
/// before 8.5 cannot run on.
pub const GRADLE_JAVA_HOME: &str = "${brewery.gradle.java.home}";

/// `lsp.jdtls.initialization_options` defaults for a jdtls overlay; empty unless it uses
/// the bundled launcher. Initialization options, not settings: jdtls reads them before
/// its first Gradle import, while settings arrive later, and a settings update that
/// does not name a key keeps it.
pub fn java_initialization_options_for(overlay: &Map<String, Value>) -> Map<String, Value> {
    if !overlay.contains_key("jdtls_launcher") {
        return Map::new();
    }
    let mut options = Map::new();
    options.insert(
        "settings".to_string(),
        serde_json::json!({"java": {"import": {"gradle": {"java": {"home": GRADLE_JAVA_HOME}}}}}),
    );
    options
}

/// Defaults a bundle contributes on top of `assets/settings/default.json`.
#[derive(Debug, Default, PartialEq)]
pub struct BundleDefaults {
    /// Merged into the default `lsp.jdtls.settings`.
    pub jdtls: Map<String, Value>,
    /// Merged into the default `lsp.jdtls.initialization_options`.
    pub jdtls_initialization_options: Map<String, Value>,
    /// Bundled extensions that default to `auto_update_extensions: false`, so the
    /// extension stays at the version the bundled helpers were pinned and tested with.
    pub pinned_extensions: Vec<String>,
}

/// The `version` in an extension's `extension.toml`.
pub fn extension_version(extension_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(extension_dir.join("extension.toml")).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table.get("version")?.as_str().map(str::to_string)
}

/// [`BundleDefaults`] for the bundle at `root`, given Zed's `installed/` extensions dir.
/// The jdtls overlay is only used while the installed Java extension is the bundled
/// version: the proxy and the setting names belong to that release.
pub fn bundle_defaults_for(root: Option<&Path>, installed_dir: &Path) -> BundleDefaults {
    let Some(root) = root else {
        return BundleDefaults::default();
    };
    let bundled_dir = |id: &str| {
        let dir = root.join("extensions").join(id);
        dir.is_dir().then_some(dir)
    };
    let pinned_extensions = SEEDED_EXTENSIONS
        .iter()
        .filter(|id| bundled_dir(id).is_some())
        .map(|id| id.to_string())
        .collect();
    let bundled = bundled_dir(JAVA_EXTENSION).and_then(|dir| extension_version(&dir));
    let installed = extension_version(&installed_dir.join(JAVA_EXTENSION));
    let jdtls = match (&bundled, &installed) {
        (Some(bundled), Some(installed)) if bundled == installed => {
            java_settings_overlay_for_root(Some(root))
        }
        (Some(bundled), installed) => {
            log::warn!(
                "brewery: the installed Java extension ({}) is not the bundled {bundled}; \
                 not pointing it at the bundled Java tools",
                installed.as_deref().unwrap_or("none")
            );
            Map::new()
        }
        (None, _) => Map::new(),
    };
    BundleDefaults {
        jdtls_initialization_options: java_initialization_options_for(&jdtls),
        jdtls,
        pinned_extensions,
    }
}

/// Layers `defaults` onto the default settings. User, project and extension settings are
/// higher layers, so they still win key by key.
pub fn apply_bundle_defaults(defaults: BundleDefaults, cx: &mut App) {
    let BundleDefaults {
        jdtls,
        jdtls_initialization_options,
        pinned_extensions,
    } = defaults;
    if jdtls.is_empty() && jdtls_initialization_options.is_empty() && pinned_extensions.is_empty() {
        return;
    }
    if !jdtls.is_empty() {
        log::info!(
            "brewery: bundled Java tools: {}",
            jdtls.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    let jdtls = (!jdtls.is_empty()).then(|| Value::Object(jdtls));
    let options = (!jdtls_initialization_options.is_empty())
        .then(|| Value::Object(jdtls_initialization_options));
    let merge = |target: &mut Option<Value>, value: &Value| match target {
        Some(target) => target.merge_from(value),
        None => *target = Some(value.clone()),
    };
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |defaults| {
            for id in &pinned_extensions {
                defaults
                    .extension
                    .auto_update_extensions
                    .entry(id.as_str().into())
                    .or_insert(false);
            }
            if jdtls.is_some() || options.is_some() {
                let server = defaults.project.lsp.0.entry(JDTLS_SERVER.into()).or_default();
                if let Some(jdtls) = &jdtls {
                    merge(&mut server.settings, jdtls);
                }
                if let Some(options) = &options {
                    merge(&mut server.initialization_options, options);
                }
            }
        });
    });
}

/// The startup hook: seeds the bundled extensions into `extensions_dir/installed`, then
/// layers the bundle's defaults. Must run after `settings::init` and before
/// `extension_host` loads its index.
pub fn setup_bundled_java(extensions_dir: &Path, cx: &mut App) {
    seed_bundled_extensions(extensions_dir);
    let defaults = bundle_defaults_for(
        crate::bundle_root().as_deref(),
        &extensions_dir.join("installed"),
    );
    apply_bundle_defaults(defaults, cx);
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
    // Staging dirs a crashed seed left behind (Brewery runs as a single instance).
    let prefix = format!(".{id}.brewery-seed-");
    for entry in std::fs::read_dir(installed_dir)?.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            std::fs::remove_dir_all(entry.path()).ok();
        }
    }
    let staging = installed_dir.join(format!("{prefix}{}", std::process::id()));
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
/// such paths are given as their 8.3 short form. Paths without spaces (and every path on
/// other systems) come back unchanged; `None` when a spaced path has no short name
/// (8.3 names disabled on the volume).
pub fn cmd_safe_path(path: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    if path.as_os_str().to_string_lossy().contains(' ') {
        return match short_path(path) {
            Some(short) if !short.as_os_str().to_string_lossy().contains(' ') => Some(short),
            _ => {
                log::warn!("brewery: no 8.3 short name for {path:?}");
                None
            }
        };
    }
    Some(path.to_path_buf())
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

    fn tools(root: &Path) -> PathBuf {
        root.join("jdtls").join("tools")
    }

    fn write_extension_toml(dir: &Path, version: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("extension.toml"),
            format!("id = \"java\"\nname = \"Java\"\nversion = \"{version}\"\n\n[grammars.java]\nrepository = \"x\"\n"),
        )
        .unwrap();
    }

    fn populate(root: &Path) {
        touch(&root.join("jre").join("release"));
        touch(&root.join("jdtls").join(launcher_name()));
        touch(&tools(root).join("lombok.jar"));
        touch(&tools(root).join("java-debug.jar"));
        touch(&tools(root).join(proxy_name()));
        write_extension_toml(&root.join("extensions").join("java"), "6.8.28");
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
        assert_eq!(overlay["lombok_jar"], string(tools(root).join("lombok.jar")));
        assert_eq!(overlay["java_debug_jar"], string(tools(root).join("java-debug.jar")));
        assert_eq!(overlay["lsp_proxy_path"], string(tools(root).join(proxy_name())));
        assert_eq!(overlay["jdk_auto_download"], Value::Bool(false));
        assert_eq!(overlay["check_updates"], Value::String("once".into()));
        // The launcher runs on the bundled JRE by itself; `java_home` would also become
        // jdtls's JAVA_HOME and so the default project runtime, so it is left to the user.
        assert!(!overlay.contains_key("java_home"), "{overlay:?}");
        assert_eq!(overlay.len(), 6, "{overlay:?}");
    }

    #[test]
    fn launcher_gives_gradle_the_users_jdk_from_the_start() {
        let temp = TempDir::new("gradle-java-home");
        populate(&temp.0);
        let overlay = java_settings_overlay_for_root(Some(&temp.0));
        let options = java_initialization_options_for(&overlay);
        assert_eq!(
            Value::Object(options),
            serde_json::json!({"settings": {"java": {"import": {"gradle": {"java": {
                "home": "${brewery.gradle.java.home}"
            }}}}}})
        );
        // Without the launcher nothing defines the property.
        let mut no_launcher = overlay;
        no_launcher.remove("jdtls_launcher");
        assert!(java_initialization_options_for(&no_launcher).is_empty());
    }

    #[test]
    fn helpers_at_the_bundle_root_are_not_used() {
        // The update helper only swaps jre/, brewery-daemon/, jdtls/ and extensions/.
        let temp = TempDir::new("old-layout");
        touch(&temp.0.join("lombok.jar"));
        touch(&temp.0.join("java-debug.jar"));
        touch(&temp.0.join(proxy_name()));
        assert!(java_settings_overlay_for_root(Some(&temp.0)).is_empty());
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
        assert_eq!(overlay["lsp_proxy_path"], string(tools(&root).join(proxy_name())));
        assert_eq!(overlay["java_debug_jar"], string(tools(&root).join("java-debug.jar")));
    }

    #[test]
    fn without_a_short_name_lombok_is_turned_off_instead_of_breaking_jdtls() {
        let temp = TempDir::new("no-8dot3");
        populate(&temp.0);
        let no_short_names = |_: &Path| None;
        let overlay = overlay_with(Some(&temp.0), &no_short_names);
        assert_eq!(
            overlay["jdtls_launcher"],
            string(temp.0.join("jdtls").join(launcher_name()))
        );
        assert_eq!(overlay["lombok_support"], Value::Bool(false));
        assert!(!overlay.contains_key("lombok_jar"), "{overlay:?}");
        assert_eq!(overlay["lsp_proxy_path"], string(tools(&temp.0).join(proxy_name())));
    }

    #[test]
    fn partial_bundle_gives_a_partial_overlay() {
        let temp = TempDir::new("partial");
        touch(&tools(&temp.0).join("lombok.jar"));
        touch(&tools(&temp.0).join(proxy_name()));
        // A jdtls directory without the launcher script does not count.
        let overlay = java_settings_overlay_for_root(Some(&temp.0));
        assert_eq!(overlay["lombok_jar"], string(tools(&temp.0).join("lombok.jar")));
        assert_eq!(overlay["lsp_proxy_path"], string(tools(&temp.0).join(proxy_name())));
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

    #[test]
    fn reads_the_extension_version() {
        let temp = TempDir::new("version");
        write_extension_toml(&temp.0, "6.8.28");
        assert_eq!(extension_version(&temp.0).as_deref(), Some("6.8.28"));
        assert_eq!(extension_version(&temp.0.join("missing")), None);
    }

    #[test]
    fn overlay_needs_the_bundled_extension_version_installed() {
        let temp = TempDir::new("versions");
        let root = temp.0.join("bundle");
        populate(&root);
        let installed = temp.0.join("installed");

        // Not installed yet (seeding failed): nothing points at the bundled helpers.
        let defaults = bundle_defaults_for(Some(&root), &installed);
        assert!(defaults.jdtls.is_empty());
        assert_eq!(defaults.pinned_extensions, vec!["java".to_string()]);

        write_extension_toml(&installed.join("java"), "6.8.28");
        let defaults = bundle_defaults_for(Some(&root), &installed);
        assert_eq!(defaults.jdtls, java_settings_overlay_for_root(Some(&root)));
        assert!(!defaults.jdtls.is_empty());
        assert_eq!(
            defaults.jdtls_initialization_options,
            java_initialization_options_for(&defaults.jdtls)
        );
        assert!(!defaults.jdtls_initialization_options.is_empty());

        // The user updated the extension: its own proxy and downloads, not ours.
        write_extension_toml(&installed.join("java"), "6.9.0");
        let defaults = bundle_defaults_for(Some(&root), &installed);
        assert!(defaults.jdtls.is_empty());
        assert!(defaults.jdtls_initialization_options.is_empty());
        assert_eq!(defaults.pinned_extensions, vec!["java".to_string()]);
    }

    #[test]
    fn no_bundle_no_defaults() {
        let temp = TempDir::new("no-bundle-defaults");
        assert_eq!(bundle_defaults_for(None, &temp.0), BundleDefaults::default());
        assert_eq!(bundle_defaults_for(Some(&temp.0), &temp.0), BundleDefaults::default());
    }

    fn jdtls_settings(cx: &App) -> Value {
        cx.global::<SettingsStore>().merged_settings().project.lsp.0[JDTLS_SERVER]
            .settings
            .clone()
            .unwrap()
    }

    fn auto_update_java(cx: &App) -> Option<bool> {
        cx.global::<SettingsStore>()
            .merged_settings()
            .extension
            .auto_update_extensions
            .get("java")
            .copied()
    }

    fn set_user_settings(cx: &mut App, json: &str) {
        cx.update_global::<SettingsStore, _>(|store, cx| {
            store.set_user_settings(json, cx).unwrap();
        });
    }

    fn full_defaults(root: &Path) -> BundleDefaults {
        let jdtls = java_settings_overlay_for_root(Some(root));
        BundleDefaults {
            jdtls_initialization_options: java_initialization_options_for(&jdtls),
            jdtls,
            pinned_extensions: vec!["java".to_string()],
        }
    }

    fn jdtls_initialization_options(cx: &App) -> Option<Value> {
        cx.global::<SettingsStore>().merged_settings().project.lsp.0[JDTLS_SERVER]
            .initialization_options
            .clone()
    }

    #[gpui::test]
    fn initialization_options_merge_with_the_users(cx: &mut TestAppContext) {
        let temp = TempDir::new("init-options");
        populate(&temp.0);
        cx.update(|cx| {
            let store = SettingsStore::new(cx, &settings::default_settings());
            cx.set_global(store);
            apply_bundle_defaults(full_defaults(&temp.0), cx);
            let home = "/settings/java/import/gradle/java/home";
            let options = jdtls_initialization_options(cx).unwrap();
            assert_eq!(options.pointer(home), Some(&Value::String(GRADLE_JAVA_HOME.into())));

            set_user_settings(
                cx,
                r#"{"lsp": {"jdtls": {"initialization_options": {
                    "settings": {"java": {"format": {"enabled": false}}}
                }}}}"#,
            );
            let options = jdtls_initialization_options(cx).unwrap();
            assert_eq!(options.pointer(home), Some(&Value::String(GRADLE_JAVA_HOME.into())));
            assert_eq!(options.pointer("/settings/java/format/enabled"), Some(&Value::Bool(false)));

            set_user_settings(
                cx,
                r#"{"lsp": {"jdtls": {"initialization_options": {
                    "settings": {"java": {"import": {"gradle": {"java": {"home": "/my/jdk"}}}}}
                }}}}"#,
            );
            let options = jdtls_initialization_options(cx).unwrap();
            assert_eq!(options.pointer(home), Some(&Value::String("/my/jdk".into())));
        });
    }

    #[gpui::test]
    fn user_settings_override_the_overlay(cx: &mut TestAppContext) {
        let temp = TempDir::new("user-wins");
        populate(&temp.0);
        cx.update(|cx| {
            let store = SettingsStore::new(cx, &settings::default_settings());
            cx.set_global(store);
            apply_bundle_defaults(full_defaults(&temp.0), cx);

            let settings = jdtls_settings(cx);
            // The overlay beats default.json's `jdk_auto_download: true`.
            assert_eq!(settings["jdk_auto_download"], Value::Bool(false));
            assert_eq!(settings["lombok_jar"], string(tools(&temp.0).join("lombok.jar")));

            set_user_settings(
                cx,
                r#"{"lsp": {"jdtls": {"settings": {
                    "lombok_jar": "/mine/lombok.jar",
                    "jdk_auto_download": true,
                    "max_memory": "4G"
                }}}}"#,
            );
            let settings = jdtls_settings(cx);
            assert_eq!(settings["lombok_jar"], Value::String("/mine/lombok.jar".into()));
            assert_eq!(settings["jdk_auto_download"], Value::Bool(true));
            assert_eq!(settings["max_memory"], Value::String("4G".into()));
            // Keys the user did not set still come from the overlay.
            assert_eq!(
                settings["java_debug_jar"],
                string(tools(&temp.0).join("java-debug.jar"))
            );
            assert_eq!(settings["check_updates"], Value::String("once".into()));
        });
    }

    #[gpui::test]
    fn bundled_extension_does_not_auto_update_unless_the_user_says_so(cx: &mut TestAppContext) {
        let temp = TempDir::new("pinned");
        populate(&temp.0);
        cx.update(|cx| {
            let store = SettingsStore::new(cx, &settings::default_settings());
            cx.set_global(store);
            assert_eq!(auto_update_java(cx), None);
            apply_bundle_defaults(full_defaults(&temp.0), cx);
            assert_eq!(auto_update_java(cx), Some(false));
            set_user_settings(cx, r#"{"auto_update_extensions": {"java": true}}"#);
            assert_eq!(auto_update_java(cx), Some(true));
        });
    }

    #[gpui::test]
    fn empty_defaults_keep_the_defaults(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = SettingsStore::new(cx, &settings::default_settings());
            cx.set_global(store);
            let before = jdtls_settings(cx);
            let options_before = jdtls_initialization_options(cx);
            apply_bundle_defaults(BundleDefaults::default(), cx);
            assert_eq!(jdtls_settings(cx), before);
            assert_eq!(jdtls_initialization_options(cx), options_before);
            assert_eq!(before["jdk_auto_download"], Value::Bool(true));
            assert_eq!(auto_update_java(cx), None);
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
    fn seeding_removes_staging_dirs_a_crash_left_behind() {
        let temp = TempDir::new("seed-stale");
        let bundled = bundled_extension(&temp.0);
        let installed = temp.0.join("extensions").join("installed");
        touch(&installed.join(".java.brewery-seed-999999").join("extension.wasm"));
        touch(&installed.join("other").join("extension.toml"));
        assert!(seed_extension(&bundled, &installed, "java").unwrap());
        let mut names: Vec<_> = std::fs::read_dir(&installed)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["java", "other"]);
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
        assert_eq!(cmd_safe_path(&file), Some(file));
    }

    #[test]
    fn cmd_safe_path_removes_spaces_on_windows() {
        let temp = TempDir::new("space");
        let file = temp.0.join("Program Files Like").join("brewery-jdtls.cmd");
        touch(&file);
        let safe = cmd_safe_path(&file);
        if cfg!(windows) && !short_names_disabled(&temp.0) {
            let safe = safe.unwrap();
            assert!(!safe.to_str().unwrap().contains(' '), "{safe:?}");
            assert_ne!(safe, file);
            // Same file.
            assert!(safe.is_file());
            assert_eq!(std::fs::read(&safe).unwrap(), b"x");
        } else if cfg!(windows) {
            assert_eq!(safe, None);
        } else {
            assert_eq!(safe, Some(file));
        }
    }

    /// 8.3 names can be turned off per volume (`fsutil 8dot3name`); then there is no
    /// cmd-safe form, which the tests above accept. Asks cmd's `%~s` independently of the
    /// code under test.
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

//! Where a packaged Brewery keeps its bundled pieces: the JRE, the daemon jar, jdtls
//! and seeded extensions. Pure path logic; every function has a `*_from_exe` variant
//! that takes the executable path so tests can use temp dirs.

use std::path::{Path, PathBuf};

pub fn bundle_root() -> Option<PathBuf> {
    bundle_root_from_exe(&std::env::current_exe().ok()?)
}

pub fn bundled_jre_home() -> Option<PathBuf> {
    bundled_jre_home_from_exe(&std::env::current_exe().ok()?)
}

pub fn bundled_daemon_jar() -> Option<PathBuf> {
    bundled_daemon_jar_from_exe(&std::env::current_exe().ok()?)
}

pub fn bundled_jdtls_home() -> Option<PathBuf> {
    bundled_jdtls_home_from_exe(&std::env::current_exe().ok()?)
}

pub fn bundled_extension(id: &str) -> Option<PathBuf> {
    bundled_extension_from_exe(&std::env::current_exe().ok()?, id)
}

/// The directory holding the bundled pieces: the exe dir (Windows), its parent when the
/// exe dir is `libexec` (Linux `/opt/brewery/libexec/brewery-editor`), or
/// `Contents/Resources` when the exe dir is `Contents/MacOS` (macOS `.app`).
pub fn bundle_root_from_exe(exe: &Path) -> Option<PathBuf> {
    let exe_dir = exe.parent()?;
    let name = exe_dir.file_name().and_then(|name| name.to_str());
    if name == Some("libexec") {
        return exe_dir.parent().map(Path::to_path_buf);
    }
    if name == Some("MacOS")
        && let Some(contents) = exe_dir.parent()
        && contents.file_name().and_then(|name| name.to_str()) == Some("Contents")
    {
        return Some(contents.join("Resources"));
    }
    Some(exe_dir.to_path_buf())
}

/// `<root>/jre`, when it looks like a runtime image (has a `release` file).
pub fn bundled_jre_home_from_exe(exe: &Path) -> Option<PathBuf> {
    let jre = bundle_root_from_exe(exe)?.join("jre");
    jre.join("release").is_file().then_some(jre)
}

/// `<root>/brewery-daemon/brewery-daemon.jar`.
pub fn bundled_daemon_jar_from_exe(exe: &Path) -> Option<PathBuf> {
    let jar = bundle_root_from_exe(exe)?
        .join("brewery-daemon")
        .join("brewery-daemon.jar");
    jar.is_file().then_some(jar)
}

/// `<root>/jdtls`.
pub fn bundled_jdtls_home_from_exe(exe: &Path) -> Option<PathBuf> {
    let jdtls = bundle_root_from_exe(exe)?.join("jdtls");
    jdtls.is_dir().then_some(jdtls)
}

/// `<root>/extensions/<id>`; ids that are not a single path component are rejected.
pub fn bundled_extension_from_exe(exe: &Path, id: &str) -> Option<PathBuf> {
    let mut components = Path::new(id).components();
    let single = matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    );
    if !single {
        return None;
    }
    let extension = bundle_root_from_exe(exe)?.join("extensions").join(id);
    extension.is_dir().then_some(extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("brewery-bundle-{name}-{}", std::process::id()));
            std::fs::remove_dir_all(&root).ok();
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    /// Fills `root` with every bundled piece.
    fn populate(root: &Path) {
        touch(&root.join("jre").join("release"));
        touch(&root.join("brewery-daemon").join("brewery-daemon.jar"));
        std::fs::create_dir_all(root.join("jdtls")).unwrap();
        std::fs::create_dir_all(root.join("extensions").join("java")).unwrap();
    }

    fn assert_all_found(exe: &Path, root: &Path) {
        assert_eq!(bundle_root_from_exe(exe), Some(root.to_path_buf()));
        assert_eq!(bundled_jre_home_from_exe(exe), Some(root.join("jre")));
        assert_eq!(
            bundled_daemon_jar_from_exe(exe),
            Some(root.join("brewery-daemon").join("brewery-daemon.jar"))
        );
        assert_eq!(bundled_jdtls_home_from_exe(exe), Some(root.join("jdtls")));
        assert_eq!(
            bundled_extension_from_exe(exe, "java"),
            Some(root.join("extensions").join("java"))
        );
    }

    #[test]
    fn windows_flat_layout() {
        let temp = TempRoot::new("windows");
        populate(&temp.0);
        assert_all_found(&temp.0.join("Brewery.exe"), &temp.0);
    }

    #[test]
    fn linux_libexec_layout() {
        let temp = TempRoot::new("linux");
        populate(&temp.0);
        let exe = temp.0.join("libexec").join("brewery-editor");
        assert_all_found(&exe, &temp.0);
    }

    #[test]
    fn macos_app_layout() {
        let temp = TempRoot::new("macos");
        let contents = temp.0.join("Brewery.app").join("Contents");
        let resources = contents.join("Resources");
        populate(&resources);
        let exe = contents.join("MacOS").join("zed");
        assert_all_found(&exe, &resources);
    }

    #[test]
    fn macos_dir_outside_an_app_is_a_plain_exe_dir() {
        let temp = TempRoot::new("macos-plain");
        let exe = temp.0.join("MacOS").join("zed");
        assert_eq!(bundle_root_from_exe(&exe), Some(temp.0.join("MacOS")));
    }

    #[test]
    fn missing_pieces_are_none() {
        let temp = TempRoot::new("missing");
        let exe = temp.0.join("Brewery.exe");
        assert_eq!(bundled_jre_home_from_exe(&exe), None);
        assert_eq!(bundled_daemon_jar_from_exe(&exe), None);
        assert_eq!(bundled_jdtls_home_from_exe(&exe), None);
        assert_eq!(bundled_extension_from_exe(&exe, "java"), None);

        // A jre directory without a `release` file is not a runtime image.
        std::fs::create_dir_all(temp.0.join("jre").join("bin")).unwrap();
        assert_eq!(bundled_jre_home_from_exe(&exe), None);
    }

    #[test]
    fn extension_ids_must_be_a_single_component() {
        let temp = TempRoot::new("ext-ids");
        populate(&temp.0);
        let exe = temp.0.join("Brewery.exe");
        for id in ["", "..", "java/..", "../jre", "a/b", "."] {
            assert_eq!(bundled_extension_from_exe(&exe, id), None, "{id:?}");
        }
        assert_eq!(bundled_extension_from_exe(&exe, "other"), None);
    }
}

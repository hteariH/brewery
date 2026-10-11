//! Running POSIX-shell tasks on Windows through Git for Windows.
//!
//! Language extensions (notably Java) ship tasks with
//! `shell: { program: "/bin/sh", args: ["-c"] }` and scripts that do POSIX
//! path arithmetic such as `${f#$p/}` against `$PWD`. Windows has no
//! `/bin/sh`, so Brewery swaps in Git for Windows' `sh.exe`/`bash.exe` and
//! rewrites the Windows paths the script sees into MSYS form (`/c/a/b`), the
//! form Git's shell itself reports in `$PWD`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use collections::HashMap;

use crate::shell::Shell;

/// Shell programs that tasks name when they expect a POSIX shell.
const POSIX_SHELL_PROGRAMS: [&str; 4] = ["/bin/sh", "/bin/bash", "sh", "bash"];

/// Converts an absolute Windows drive path (`C:\a b\x`, `D:/x`) to MSYS form
/// (`/c/a b/x`). Returns `None` for anything that is not a drive path.
pub fn to_msys_path(path: &str) -> Option<String> {
    // Canonicalized paths may carry the verbatim prefix; MSYS has no
    // equivalent, and the plain drive path names the same file.
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let mut chars = path.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') {
        return None;
    }
    let rest = chars.as_str();
    if !(rest.is_empty() || rest.starts_with(['\\', '/'])) {
        // `C:relative` is relative to the drive's current directory.
        return None;
    }
    Some(format!(
        "/{}{}",
        drive.to_ascii_lowercase(),
        rest.replace('\\', "/")
    ))
}

/// Finds Git for Windows' shell for a task's POSIX `program` among the given
/// candidate directories.
pub fn find_git_posix_shell(program: &str, candidates: &[PathBuf]) -> Option<PathBuf> {
    if !POSIX_SHELL_PROGRAMS.contains(&program) {
        return None;
    }
    let executable = if program.ends_with("bash") {
        "bash.exe"
    } else {
        "sh.exe"
    };
    candidates
        .iter()
        .map(|dir| dir.join(executable))
        .find(|path| path.is_file())
}

/// Directories that may hold Git for Windows' `sh.exe`/`bash.exe`, given the
/// values of `%ProgramFiles%`, `%LocalAppData%` and `%PATH%`.
pub fn git_shell_candidates(
    program_files: Option<&Path>,
    local_app_data: Option<&Path>,
    path_var: Option<&OsStr>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(program_files) = program_files {
        let git = program_files.join("Git");
        candidates.push(git.join("bin"));
        candidates.push(git.join("usr").join("bin"));
    }
    if let Some(local_app_data) = local_app_data {
        // Per-user installs of Git for Windows.
        candidates.push(local_app_data.join("Programs").join("Git").join("bin"));
    }
    // Custom install locations: `git.exe` on PATH lives in `<git>\cmd`, the
    // shells in `<git>\bin`.
    if let Some(git_dir) = path_var.and_then(|path_var| {
        std::env::split_paths(path_var).find(|dir| dir.join("git.exe").is_file())
    }) {
        candidates.push(git_dir.join("..").join("bin"));
    }
    candidates
}

/// Rewrites `ZED_*` path variables in `env` to the form a MSYS shell expects
/// and returns the `(original, rewritten)` pairs, longest original first, so
/// the same values can be rewritten where they were substituted into the
/// task's command text.
pub fn msys_env(env: &mut HashMap<String, String>) -> Vec<(String, String)> {
    let mut replacements = Vec::new();
    for (name, value) in env.iter_mut() {
        if !name.starts_with("ZED_") {
            continue;
        }
        let rewritten = match to_msys_path(value) {
            Some(rewritten) => rewritten,
            // Relative paths (`ZED_RELATIVE_FILE`) stay relative, but the
            // script compares them against `/`-separated paths.
            None if name.starts_with("ZED_RELATIVE_") => value.replace('\\', "/"),
            None => continue,
        };
        if rewritten != *value {
            replacements.push((std::mem::replace(value, rewritten.clone()), rewritten));
        }
    }
    // Longer values first: `ZED_FILE` contains `ZED_WORKTREE_ROOT`, and
    // rewriting the root first would leave a half-converted file path.
    replacements.sort_by(|(a, _), (b, _)| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    replacements
}

/// Replaces every original value with its rewritten form.
pub fn rewrite_text(text: &str, replacements: &[(String, String)]) -> String {
    replacements
        .iter()
        .fold(text.to_owned(), |text, (original, rewritten)| {
            text.replace(original.as_str(), rewritten)
        })
}

/// If `shell` names `/bin/sh` or `/bin/bash` and a Git shell is found among
/// `candidates`, points `shell` at it and rewrites the task's paths to MSYS
/// form. Returns whether the task was adapted.
///
/// The process working directory stays a Windows path (`cwd`); the shell only
/// learns its MSYS form through `PWD`.
pub fn adapt_posix_task(
    shell: &mut Shell,
    command: &mut Option<String>,
    args: &mut [String],
    env: &mut HashMap<String, String>,
    cwd: Option<&Path>,
    candidates: &[PathBuf],
) -> bool {
    let program = match shell {
        Shell::Program(program) | Shell::WithArguments { program, .. } => program,
        Shell::System => return false,
    };
    // A bare `sh`/`bash` resolves through PATH to whatever shell the user
    // installed; only the Unix absolute paths are certain not to exist.
    if !matches!(program.as_str(), "/bin/sh" | "/bin/bash") {
        return false;
    }
    let Some(git_shell) = find_git_posix_shell(program, candidates) else {
        return false;
    };
    *program = git_shell.to_string_lossy().into_owned();

    // Task variables are substituted into the command text before spawning,
    // so the same Windows paths appear there as well as in the environment.
    let replacements = msys_env(env);
    if let Some(command) = command {
        *command = rewrite_text(command, &replacements);
    }
    for arg in args {
        *arg = rewrite_text(arg, &replacements);
    }
    // Left alone, Git's shell derives `$PWD` from the working directory but
    // reports its own mount points (`%TEMP%` becomes `/tmp`), and scripts
    // that strip `$PWD/` from `$ZED_FILE` then see two spellings of one
    // directory. A `PWD` naming the working directory is kept as is.
    if let Some(cwd) = cwd.and_then(|cwd| to_msys_path(&cwd.to_string_lossy())) {
        env.insert("PWD".to_owned(), cwd);
    }
    true
}

/// [`adapt_posix_task`] with Git for Windows' real install locations; warns
/// once when a task needs a POSIX shell and none is installed.
#[cfg(windows)]
pub fn adapt_posix_task_for_windows(
    shell: &mut Shell,
    command: &mut Option<String>,
    args: &mut [String],
    env: &mut HashMap<String, String>,
    cwd: Option<&Path>,
) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);

    let candidates = git_shell_candidates(
        std::env::var_os("ProgramFiles").as_deref().map(Path::new),
        std::env::var_os("LocalAppData").as_deref().map(Path::new),
        std::env::var_os("PATH").as_deref(),
    );
    let adapted = adapt_posix_task(shell, command, args, env, cwd, &candidates);
    let needs_posix_shell = matches!(
        shell,
        Shell::Program(program) | Shell::WithArguments { program, .. }
            if program == "/bin/sh" || program == "/bin/bash"
    );
    if !adapted && needs_posix_shell && !WARNED.swap(true, Ordering::Relaxed) {
        log::warn!(
            "Task needs a POSIX shell but none was found. \
             Install Git for Windows to run Java gutter tasks."
        );
    }
    adapted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, b"").expect("write");
    }

    #[test]
    fn converts_drive_paths() {
        assert_eq!(
            to_msys_path(r"C:\Users\a b\x").as_deref(),
            Some("/c/Users/a b/x")
        );
        assert_eq!(to_msys_path("D:/x").as_deref(), Some("/d/x"));
        assert_eq!(to_msys_path(r"e:\").as_deref(), Some("/e/"));
        assert_eq!(to_msys_path("C:").as_deref(), Some("/c"));
        assert_eq!(
            to_msys_path(r"\\?\C:\Users\x").as_deref(),
            Some("/c/Users/x")
        );
    }

    #[test]
    fn leaves_non_drive_paths_alone() {
        assert_eq!(to_msys_path("/c/Users"), None);
        assert_eq!(to_msys_path(r"src\Main.java"), None);
        assert_eq!(to_msys_path("C:relative"), None);
        assert_eq!(to_msys_path(r"\\server\share"), None);
        assert_eq!(to_msys_path("Main"), None);
        assert_eq!(to_msys_path(""), None);
    }

    #[test]
    fn finds_git_shell_in_first_matching_candidate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing");
        let only_bash = dir.path().join("only_bash");
        let full = dir.path().join("full");
        touch(&only_bash.join("bash.exe"));
        touch(&full.join("sh.exe"));
        touch(&full.join("bash.exe"));
        let candidates = vec![missing, only_bash.clone(), full.clone()];

        for program in ["/bin/sh", "sh"] {
            assert_eq!(
                find_git_posix_shell(program, &candidates),
                Some(full.join("sh.exe"))
            );
        }
        for program in ["/bin/bash", "bash"] {
            assert_eq!(
                find_git_posix_shell(program, &candidates),
                Some(only_bash.join("bash.exe"))
            );
        }
        assert_eq!(find_git_posix_shell("pwsh", &candidates), None);
        assert_eq!(find_git_posix_shell("/bin/zsh", &candidates), None);
        assert_eq!(find_git_posix_shell("/bin/sh", &[]), None);
    }

    #[test]
    fn candidate_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git_cmd = dir.path().join("Git").join("cmd");
        touch(&git_cmd.join("git.exe"));
        let no_git = dir.path().join("other");
        std::fs::create_dir_all(&no_git).expect("mkdir");
        let path_var = std::env::join_paths([no_git, git_cmd.clone()]).expect("join");

        let candidates = git_shell_candidates(
            Some(Path::new(r"C:\Program Files")),
            Some(Path::new(r"C:\Users\a\AppData\Local")),
            Some(&path_var),
        );
        assert_eq!(
            candidates,
            vec![
                Path::new(r"C:\Program Files").join("Git").join("bin"),
                Path::new(r"C:\Program Files")
                    .join("Git")
                    .join("usr")
                    .join("bin"),
                Path::new(r"C:\Users\a\AppData\Local")
                    .join("Programs")
                    .join("Git")
                    .join("bin"),
                git_cmd.join("..").join("bin"),
            ]
        );
        assert!(git_shell_candidates(None, None, None).is_empty());
    }

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn converts_zed_path_variables_only() {
        let mut env = env_of(&[
            ("ZED_FILE", r"C:\proj\src\main\java\Main.java"),
            ("ZED_DIRNAME", r"C:\proj\src\main\java"),
            ("ZED_WORKTREE_ROOT", r"C:\proj"),
            ("ZED_RELATIVE_FILE", r"src\main\java\Main.java"),
            ("ZED_RELATIVE_DIR", r"src\main\java"),
            ("ZED_STEM", "Main"),
            ("ZED_CUSTOM_java_class_name", "Main"),
            ("JAVA_HOME", r"C:\jdk"),
            ("PATH", r"C:\Windows;C:\jdk\bin"),
        ]);
        let replacements = msys_env(&mut env);

        assert_eq!(env["ZED_FILE"], "/c/proj/src/main/java/Main.java");
        assert_eq!(env["ZED_DIRNAME"], "/c/proj/src/main/java");
        assert_eq!(env["ZED_WORKTREE_ROOT"], "/c/proj");
        assert_eq!(env["ZED_RELATIVE_FILE"], "src/main/java/Main.java");
        assert_eq!(env["ZED_RELATIVE_DIR"], "src/main/java");
        assert_eq!(env["ZED_STEM"], "Main");
        assert_eq!(env["ZED_CUSTOM_java_class_name"], "Main");
        // The shell and the tools it runs (java, mvn) still need Windows paths.
        assert_eq!(env["JAVA_HOME"], r"C:\jdk");
        assert_eq!(env["PATH"], r"C:\Windows;C:\jdk\bin");

        assert_eq!(replacements.len(), 5);
        assert_eq!(
            replacements[0],
            (
                r"C:\proj\src\main\java\Main.java".to_owned(),
                "/c/proj/src/main/java/Main.java".to_owned()
            )
        );
        assert!(
            replacements
                .windows(2)
                .all(|pair| pair[0].0.len() >= pair[1].0.len())
        );
    }

    #[test]
    fn rewrites_longest_values_first() {
        let mut env = env_of(&[
            ("ZED_FILE", r"C:\proj\src\Main.java"),
            ("ZED_WORKTREE_ROOT", r"C:\proj"),
        ]);
        let replacements = msys_env(&mut env);
        assert_eq!(
            rewrite_text(
                r#"f="C:\proj\src\Main.java"; root="C:\proj"; p="$PWD""#,
                &replacements
            ),
            r#"f="/c/proj/src/Main.java"; root="/c/proj"; p="$PWD""#
        );
    }

    #[test]
    fn adapts_bin_sh_task() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("Git").join("bin");
        touch(&bin.join("sh.exe"));
        let candidates = vec![bin.clone()];

        let mut shell = Shell::WithArguments {
            program: "/bin/sh".to_owned(),
            args: vec!["-c".to_owned()],
            title_override: None,
        };
        let mut command = Some(r#"f="C:\proj\A.java"; echo "${f#$PWD/}""#.to_owned());
        let mut args = vec![r"C:\proj".to_owned()];
        let mut env = env_of(&[
            ("ZED_FILE", r"C:\proj\A.java"),
            ("ZED_WORKTREE_ROOT", r"C:\proj"),
        ]);

        assert!(adapt_posix_task(
            &mut shell,
            &mut command,
            &mut args,
            &mut env,
            Some(Path::new(r"C:\proj")),
            &candidates
        ));
        assert_eq!(
            shell,
            Shell::WithArguments {
                program: bin.join("sh.exe").to_string_lossy().into_owned(),
                args: vec!["-c".to_owned()],
                title_override: None,
            }
        );
        assert_eq!(
            command.as_deref(),
            Some(r#"f="/c/proj/A.java"; echo "${f#$PWD/}""#)
        );
        assert_eq!(args, vec!["/c/proj".to_owned()]);
        assert_eq!(env["ZED_FILE"], "/c/proj/A.java");
        // Git's shell would otherwise report its own mount points in `$PWD`
        // (`%TEMP%` is `/tmp`), which `${f#$PWD/}` cannot strip from `/c/...`.
        assert_eq!(env["PWD"], "/c/proj");
    }

    #[test]
    fn leaves_other_tasks_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("Git").join("bin");
        touch(&bin.join("sh.exe"));
        touch(&bin.join("bash.exe"));
        let original_env = env_of(&[("ZED_FILE", r"C:\proj\A.java")]);

        for (shell, candidates) in [
            (Shell::System, vec![bin.clone()]),
            (Shell::Program("pwsh".to_owned()), vec![bin.clone()]),
            // A bare `sh` is resolved through PATH, which may be a shell the
            // user chose (MSYS2, Cygwin); only the Unix absolute paths are
            // certain to fail on Windows.
            (Shell::Program("sh".to_owned()), vec![bin]),
            // No Git for Windows: keep Zed's behaviour.
            (Shell::Program("/bin/sh".to_owned()), Vec::new()),
        ] {
            let mut adapted = shell.clone();
            let mut command = Some(r"type C:\proj\A.java".to_owned());
            let mut env = original_env.clone();
            assert!(!adapt_posix_task(
                &mut adapted,
                &mut command,
                &mut [],
                &mut env,
                None,
                &candidates
            ));
            assert_eq!(adapted, shell);
            assert_eq!(command.as_deref(), Some(r"type C:\proj\A.java"));
            assert_eq!(env, original_env);
        }
    }

    #[test]
    fn adapts_bin_bash_program() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("Git").join("bin");
        touch(&bin.join("bash.exe"));
        let mut shell = Shell::Program("/bin/bash".to_owned());
        assert!(adapt_posix_task(
            &mut shell,
            &mut None,
            &mut [],
            &mut HashMap::default(),
            None,
            std::slice::from_ref(&bin)
        ));
        assert_eq!(
            shell,
            Shell::Program(bin.join("bash.exe").to_string_lossy().into_owned())
        );
    }
}

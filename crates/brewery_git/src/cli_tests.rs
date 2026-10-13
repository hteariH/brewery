use super::*;
use fs::RealFs;
use gpui::TestAppContext;
use project::Project;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use worktree::WorktreeModelHandle;

/// A real git repository in a temp dir, isolated from the user's global and system config.
struct TestRepo {
    _dir: TempDir,
    work: PathBuf,
    env: HashMap<String, String>,
}

impl TestRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let work = dir.path().join("repo");
        std::fs::create_dir(&work).expect("repo dir");
        let global = dir.path().join("gitconfig");
        std::fs::write(&global, "").expect("global config");
        let env = HashMap::from([
            (
                "GIT_CONFIG_GLOBAL".to_string(),
                global.to_string_lossy().into_owned(),
            ),
            ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
        ]);
        let repo = Self {
            _dir: dir,
            work,
            env,
        };
        repo.git(&["init", "--quiet"]);
        repo.git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
        for (key, value) in [
            ("core.autocrlf", "false"),
            ("core.safecrlf", "false"),
            ("user.name", "Brewery Test"),
            ("user.email", "test@brewery.dev"),
            ("commit.gpgsign", "false"),
            ("merge.ff", "true"),
        ] {
            repo.git(&["config", key, value]);
        }
        repo
    }

    /// Runs git and returns trimmed stdout; panics on failure.
    fn git(&self, args: &[&str]) -> String {
        let output = smol::block_on(
            util::command::new_command("git")
                .current_dir(&self.work)
                .envs(&self.env)
                .env("GIT_EDITOR", "true")
                .args(args)
                .output(),
        )
        .expect("spawn git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn write(&self, path: &str, content: &str) {
        std::fs::write(self.work.join(path), content).expect("write file");
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.work.join(path)).expect("read file")
    }

    fn commit(&self, path: &str, content: &str, message: &str) -> String {
        self.write(path, content);
        self.git(&["add", "--", path]);
        self.git(&["commit", "--quiet", "-m", message]);
        self.head()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn git_dir(&self) -> PathBuf {
        PathBuf::from(self.git(&["rev-parse", "--absolute-git-dir"]))
    }

    fn runner(&self) -> GitRunner {
        GitRunner {
            git_binary: PathBuf::from("git"),
            work_dir: self.work.clone(),
            git_dir: self.git_dir(),
            env: self.env.clone(),
            is_trusted: false,
        }
    }

    /// main: a.txt = "base"; feature: a.txt = "feature"; main gets b.txt.
    fn diverged_without_conflict(&self) {
        self.commit("a.txt", "base\n", "base");
        self.git(&["checkout", "--quiet", "-b", "feature"]);
        self.commit("a.txt", "feature\n", "feature change");
        self.git(&["checkout", "--quiet", "main"]);
        self.commit("b.txt", "main\n", "main change");
    }

    /// main and feature both change a.txt and c.txt differently.
    fn diverged_with_conflict(&self) {
        self.commit("a.txt", "base\n", "base");
        self.write("c.txt", "base\n");
        self.git(&["add", "c.txt"]);
        self.git(&["commit", "--quiet", "-m", "base c"]);
        self.git(&["checkout", "--quiet", "-b", "feature"]);
        self.write("a.txt", "feature\n");
        self.write("c.txt", "feature\n");
        self.git(&["commit", "--quiet", "-am", "feature change"]);
        self.git(&["checkout", "--quiet", "main"]);
        self.write("a.txt", "main\n");
        self.write("c.txt", "main\n");
        self.git(&["commit", "--quiet", "-am", "main change"]);
    }
}

fn block<T>(future: impl std::future::Future<Output = T>) -> T {
    smol::block_on(future)
}

fn paths(names: &[&str]) -> Vec<RepoPath> {
    names
        .iter()
        .map(|name| RepoPath::new(name).expect("repo path"))
        .collect()
}

#[test]
fn run_returns_status_and_output_without_failing_on_nonzero_status() {
    let repo = TestRepo::new();
    repo.commit("a.txt", "a\n", "first");
    let runner = repo.runner();

    let ok = block(runner.run(&["rev-parse", "--abbrev-ref", "HEAD"])).expect("run");
    assert_eq!(ok.status, 0);
    assert_eq!(ok.stdout.trim(), "main");

    let failed = block(runner.run(&["rev-parse", "--verify", "no-such-ref"])).expect("run");
    assert_ne!(failed.status, 0);
    assert!(!failed.stderr.is_empty());
}

#[test]
fn merge_fast_forward() {
    let repo = TestRepo::new();
    repo.commit("a.txt", "base\n", "base");
    repo.git(&["checkout", "--quiet", "-b", "feature"]);
    let feature_head = repo.commit("a.txt", "feature\n", "feature change");
    repo.git(&["checkout", "--quiet", "main"]);

    let outcome = block(repo.runner().merge("feature")).expect("merge");
    assert_eq!(outcome, MergeOutcome::Merged);
    assert_eq!(repo.head(), feature_head);
    assert_eq!(repo.read("a.txt"), "feature\n");
}

#[test]
fn merge_creates_merge_commit_without_editor() {
    let repo = TestRepo::new();
    repo.diverged_without_conflict();

    let outcome = block(repo.runner().merge("feature")).expect("merge");
    assert_eq!(outcome, MergeOutcome::Merged);
    let parents = repo.git(&["rev-list", "--parents", "-n", "1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3, "a merge commit");
    assert_eq!(repo.read("a.txt"), "feature\n");
    assert_eq!(repo.read("b.txt"), "main\n");
}

#[test]
fn merge_up_to_date() {
    let repo = TestRepo::new();
    repo.commit("a.txt", "base\n", "base");
    repo.git(&["branch", "feature"]);
    repo.commit("a.txt", "main\n", "main change");
    let head = repo.head();

    let outcome = block(repo.runner().merge("feature")).expect("merge");
    assert_eq!(outcome, MergeOutcome::UpToDate);
    assert_eq!(repo.head(), head);
}

#[test]
fn merge_with_conflicts_leaves_merge_head_and_lists_paths() {
    let repo = TestRepo::new();
    repo.diverged_with_conflict();
    let runner = repo.runner();

    let outcome = block(runner.merge("feature")).expect("merge");
    assert_eq!(outcome, MergeOutcome::Conflicts(paths(&["a.txt", "c.txt"])));
    assert!(repo.git_dir().join("MERGE_HEAD").exists());
    assert_eq!(
        block(runner.in_progress()).expect("in progress"),
        Some(Operation::Merge)
    );
}

#[test]
fn merge_unknown_branch_fails_with_git_message() {
    let repo = TestRepo::new();
    repo.commit("a.txt", "base\n", "base");

    let outcome = block(repo.runner().merge("no-such-branch")).expect("merge");
    match outcome {
        MergeOutcome::Failed(message) => assert!(!message.is_empty()),
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn abort_merge_restores_head() {
    let repo = TestRepo::new();
    repo.diverged_with_conflict();
    let head = repo.head();
    let runner = repo.runner();
    block(runner.merge("feature")).expect("merge");

    block(runner.abort_merge()).expect("abort");
    assert_eq!(repo.head(), head);
    assert!(!repo.git_dir().join("MERGE_HEAD").exists());
    assert_eq!(block(runner.in_progress()).expect("in progress"), None);
    assert_eq!(repo.read("a.txt"), "main\n");
}

#[test]
fn abort_merge_without_merge_is_an_error() {
    let repo = TestRepo::new();
    repo.commit("a.txt", "base\n", "base");
    assert!(block(repo.runner().abort_merge()).is_err());
}

#[test]
fn continue_merge_refuses_unresolved_then_commits_without_editor() {
    let repo = TestRepo::new();
    repo.diverged_with_conflict();
    let runner = repo.runner();
    block(runner.merge("feature")).expect("merge");

    let unresolved = block(runner.continue_merge()).expect("continue");
    assert!(
        matches!(unresolved, MergeOutcome::Conflicts(_)),
        "unresolved files keep the merge going: {unresolved:?}"
    );

    repo.write("a.txt", "resolved\n");
    repo.write("c.txt", "resolved\n");
    repo.git(&["add", "a.txt", "c.txt"]);
    let outcome = block(runner.continue_merge()).expect("continue");
    assert_eq!(outcome, MergeOutcome::Merged);
    assert!(!repo.git_dir().join("MERGE_HEAD").exists());
    let parents = repo.git(&["rev-list", "--parents", "-n", "1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3);
}

#[test]
fn rebase_without_conflicts() {
    let repo = TestRepo::new();
    repo.diverged_without_conflict();
    repo.git(&["checkout", "--quiet", "feature"]);
    let main_head = repo.git(&["rev-parse", "main"]);

    let outcome = block(repo.runner().rebase("main")).expect("rebase");
    assert_eq!(outcome, MergeOutcome::Merged);
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), main_head);
    assert_eq!(repo.read("a.txt"), "feature\n");
    assert_eq!(repo.read("b.txt"), "main\n");
}

#[test]
fn rebase_up_to_date() {
    let repo = TestRepo::new();
    repo.commit("a.txt", "base\n", "base");
    repo.git(&["branch", "old"]);
    repo.commit("a.txt", "main\n", "main change");
    let head = repo.head();

    let outcome = block(repo.runner().rebase("old")).expect("rebase");
    assert_eq!(outcome, MergeOutcome::UpToDate);
    assert_eq!(repo.head(), head);
}

#[test]
fn rebase_with_conflicts_then_abort() {
    let repo = TestRepo::new();
    repo.diverged_with_conflict();
    repo.git(&["checkout", "--quiet", "feature"]);
    let feature_head = repo.head();
    let runner = repo.runner();

    let outcome = block(runner.rebase("main")).expect("rebase");
    assert_eq!(outcome, MergeOutcome::Conflicts(paths(&["a.txt", "c.txt"])));
    assert_eq!(
        block(runner.in_progress()).expect("in progress"),
        Some(Operation::Rebase)
    );

    block(runner.abort_rebase()).expect("abort");
    assert_eq!(block(runner.in_progress()).expect("in progress"), None);
    assert_eq!(repo.head(), feature_head);
    assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "feature");
}

#[test]
fn rebase_with_conflicts_then_continue() {
    let repo = TestRepo::new();
    repo.diverged_with_conflict();
    repo.git(&["checkout", "--quiet", "feature"]);
    let main_head = repo.git(&["rev-parse", "main"]);
    let runner = repo.runner();
    block(runner.rebase("main")).expect("rebase");

    repo.write("a.txt", "resolved\n");
    repo.write("c.txt", "resolved\n");
    repo.git(&["add", "a.txt", "c.txt"]);
    let outcome = block(runner.continue_rebase()).expect("continue");
    assert_eq!(outcome, MergeOutcome::Merged);
    assert_eq!(block(runner.in_progress()).expect("in progress"), None);
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), main_head);
    assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "feature");
    assert_eq!(repo.read("a.txt"), "resolved\n");
}

#[test]
fn checkout_side_takes_ours_or_theirs_and_marks_resolved() {
    let repo = TestRepo::new();
    repo.diverged_with_conflict();
    let runner = repo.runner();
    block(runner.merge("feature")).expect("merge");

    block(runner.checkout_side(paths(&["a.txt"]), Side::Ours)).expect("ours");
    block(runner.checkout_side(paths(&["c.txt"]), Side::Theirs)).expect("theirs");
    assert_eq!(repo.read("a.txt"), "main\n");
    assert_eq!(repo.read("c.txt"), "feature\n");
    assert_eq!(repo.git(&["diff", "--name-only", "--diff-filter=U"]), "");

    assert_eq!(
        block(runner.continue_merge()).expect("continue"),
        MergeOutcome::Merged
    );
}

#[test]
fn checkout_side_resolves_modify_delete_conflicts() {
    let repo = TestRepo::new();
    for name in ["d1.txt", "d2.txt", "d3.txt", "d4.txt"] {
        repo.write(
            name, "base
",
        );
    }
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "base"]);
    repo.git(&["checkout", "--quiet", "-b", "feature"]);
    // feature modifies d1, d2 and deletes d3, d4.
    repo.write(
        "d1.txt", "feature
",
    );
    repo.write(
        "d2.txt", "feature
",
    );
    repo.git(&["rm", "--quiet", "d3.txt", "d4.txt"]);
    repo.git(&["commit", "--quiet", "-am", "feature"]);
    repo.git(&["checkout", "--quiet", "main"]);
    // main deletes d1, d2 and modifies d3, d4.
    repo.git(&["rm", "--quiet", "d1.txt", "d2.txt"]);
    repo.write(
        "d3.txt", "main
",
    );
    repo.write(
        "d4.txt", "main
",
    );
    repo.git(&["commit", "--quiet", "-am", "main"]);
    let runner = repo.runner();
    assert_eq!(
        block(runner.merge("feature")).expect("merge"),
        MergeOutcome::Conflicts(paths(&["d1.txt", "d2.txt", "d3.txt", "d4.txt"]))
    );

    // Ours deleted d1 and kept d4; theirs kept d2 and deleted d3.
    block(runner.checkout_side(paths(&["d1.txt", "d4.txt"]), Side::Ours)).expect("ours");
    block(runner.checkout_side(paths(&["d2.txt", "d3.txt"]), Side::Theirs)).expect("theirs");
    assert!(!repo.work.join("d1.txt").exists());
    assert_eq!(
        repo.read("d2.txt"),
        "feature
"
    );
    assert!(!repo.work.join("d3.txt").exists());
    assert_eq!(
        repo.read("d4.txt"),
        "main
"
    );
    assert_eq!(repo.git(&["diff", "--name-only", "--diff-filter=U"]), "");

    assert_eq!(
        block(runner.continue_merge()).expect("continue"),
        MergeOutcome::Merged
    );
    assert_eq!(
        repo.git(&["ls-files"]),
        "d2.txt
d4.txt"
    );
}

#[test]
#[allow(clippy::disallowed_methods)]
fn run_until_kills_git_at_the_deadline() {
    let repo = TestRepo::new();
    repo.git(&["config", "alias.slow", "!sleep 5"]);
    let runner = repo.runner();
    let started = std::time::Instant::now();
    let result = block(runner.run_until(&["slow"], async {
        smol::Timer::after(std::time::Duration::from_millis(300)).await;
    }));
    let error = result.expect_err("timed out");
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(4));

    let fast =
        block(runner.run_until(&["--version"], futures::future::pending::<()>())).expect("run");
    assert_eq!(fast.status, 0);
}

#[test]
fn recent_branches_from_reflog() {
    let repo = TestRepo::new();
    repo.commit("a.txt", "base\n", "base");
    for branch in ["one", "two", "gone", "three"] {
        repo.git(&["branch", branch]);
    }
    for branch in ["one", "two", "one", "gone", "three", "main", "three"] {
        repo.git(&["checkout", "--quiet", branch]);
    }
    // Detached HEAD entries are not branches.
    let head = repo.head();
    repo.git(&["checkout", "--quiet", &head]);
    repo.git(&["checkout", "--quiet", "three"]);
    repo.git(&["branch", "-D", "gone"]);
    let runner = repo.runner();

    assert_eq!(
        block(runner.recent_branches(10)).expect("recent"),
        vec!["three", "main", "one", "two"]
    );
    assert_eq!(
        block(runner.recent_branches(2)).expect("recent"),
        vec!["three", "main"]
    );
}

#[test]
fn recent_branches_in_empty_repository() {
    let repo = TestRepo::new();
    assert!(
        block(repo.runner().recent_branches(5))
            .expect("recent")
            .is_empty()
    );
}

#[test]
fn show_at_existing_and_missing_path() {
    let repo = TestRepo::new();
    std::fs::create_dir(repo.work.join("dir")).expect("dir");
    let first = repo.commit("dir/a.txt", "one\r\ntwo\n", "first");
    repo.commit("dir/a.txt", "changed\n", "second");
    let runner = repo.runner();

    let path = RepoPath::new("dir/a.txt").expect("path");
    assert_eq!(
        block(runner.show_at(&first, &path)).expect("show"),
        Some("one\r\ntwo\n".to_string())
    );
    assert_eq!(
        block(runner.show_at("HEAD", &path)).expect("show"),
        Some("changed\n".to_string())
    );
    let missing = RepoPath::new("dir/missing.txt").expect("path");
    assert_eq!(block(runner.show_at("HEAD", &missing)).expect("show"), None);
    assert!(
        block(runner.show_at("no-such-revision", &path)).is_err(),
        "an unknown revision is an error, not a missing file"
    );
    assert!(block(runner.show_at("HEAD", &RepoPath::new("dir").expect("path"))).is_err());
}

#[test]
fn show_bytes_at_is_lossless_and_show_at_refuses_binary() {
    let repo = TestRepo::new();
    let bytes = vec![0u8, 159, 146, 150, 255, 13, 10];
    std::fs::write(repo.work.join("blob.bin"), &bytes).expect("write");
    repo.git(&["add", "blob.bin"]);
    repo.git(&["commit", "--quiet", "-m", "binary"]);
    let runner = repo.runner();
    let path = RepoPath::new("blob.bin").expect("path");

    assert_eq!(
        block(runner.show_bytes_at("HEAD", &path)).expect("show"),
        Some(bytes)
    );
    assert!(block(runner.show_at("HEAD", &path)).is_err());
    let missing = RepoPath::new("missing.bin").expect("path");
    assert_eq!(
        block(runner.show_bytes_at("HEAD", &missing)).expect("show"),
        None
    );
}

#[test]
fn rev_list_range_with_limit() {
    let repo = TestRepo::new();
    let base = repo.commit("a.txt", "0\n", "c0");
    let c1 = repo.commit("a.txt", "1\n", "c1");
    let c2 = repo.commit("a.txt", "2\n", "c2");
    let c3 = repo.commit("a.txt", "3\n", "c3");
    let runner = repo.runner();
    let oid = |sha: &str| sha.parse::<Oid>().expect("oid");

    let range = format!("{base}..HEAD");
    assert_eq!(
        block(runner.rev_list(&range, 10)).expect("rev-list"),
        vec![oid(&c3), oid(&c2), oid(&c1)]
    );
    assert_eq!(
        block(runner.rev_list(&range, 2)).expect("rev-list"),
        vec![oid(&c3), oid(&c2)]
    );
    assert!(block(runner.rev_list("no-such..HEAD", 10)).is_err());
}

#[test]
fn in_progress_detects_cherry_pick() {
    let repo = TestRepo::new();
    repo.diverged_with_conflict();
    let runner = repo.runner();
    assert_eq!(block(runner.in_progress()).expect("in progress"), None);

    let pick = repo.runner();
    let output = block(pick.run(&["cherry-pick", "feature"])).expect("run");
    assert_ne!(output.status, 0);
    assert_eq!(
        block(runner.in_progress()).expect("in progress"),
        Some(Operation::CherryPick)
    );
}

#[test]
fn user_strings_that_look_like_options_are_refused() {
    let repo = TestRepo::new();
    repo.commit("a.txt", "base\n", "base");
    let runner = repo.runner();
    let head = repo.head();

    assert!(block(runner.merge("--abort")).is_err());
    assert!(block(runner.merge("-")).is_err());
    assert!(block(runner.merge("")).is_err());
    assert!(block(runner.rebase("--root")).is_err());
    assert!(block(runner.show_at("--output=x", &RepoPath::new("a.txt").expect("path"))).is_err());
    assert!(block(runner.rev_list("--all", 5)).is_err());
    assert_eq!(repo.head(), head);
    assert!(!repo.work.join("x").exists());
}

// Through the project's Repository entity and its job queue.

fn init_gpui(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings_store = settings::SettingsStore::test(cx);
        cx.set_global(settings_store);
        release_channel::init(semver::Version::new(0, 0, 0), cx);
    });
}

async fn open_repository(
    work: &Path,
    cx: &mut TestAppContext,
) -> (Entity<Project>, Entity<Repository>) {
    let project = Project::test(RealFs::new(None, cx.executor()), [work], cx).await;
    let tree = project.read_with(cx, |project, cx| {
        project.worktrees(cx).next().expect("worktree")
    });
    tree.flush_fs_events(cx).await;
    project
        .update(cx, |project, cx| project.git_scans_complete(cx))
        .await;
    cx.executor().run_until_parked();
    let repository = project.read_with(cx, |project, cx| {
        project
            .repositories(cx)
            .values()
            .next()
            .expect("repository")
            .clone()
    });
    (project, repository)
}

#[gpui::test]
async fn run_through_repository_job_queue(cx: &mut TestAppContext) {
    init_gpui(cx);
    cx.executor().allow_parking();
    let repo = TestRepo::new();
    repo.diverged_with_conflict();
    set_test_env(repo.env.clone());
    let (_project, repository) = open_repository(&repo.work, cx).await;

    let output = cx
        .update(|cx| {
            GitCli::run(
                &repository,
                vec!["rev-parse".into(), "--abbrev-ref".into(), "HEAD".into()],
                Some(std::time::Duration::from_secs(60)),
                cx,
            )
        })
        .await
        .expect("run");
    assert_eq!(output.status, 0);
    assert_eq!(output.stdout.trim(), "main");

    let outcome = cx
        .update(|cx| GitCli::merge(&repository, "feature", cx))
        .await
        .expect("merge");
    assert_eq!(outcome, MergeOutcome::Conflicts(paths(&["a.txt", "c.txt"])));
    assert_eq!(
        cx.update(|cx| GitCli::in_progress(&repository, cx))
            .await
            .expect("in progress"),
        Some(Operation::Merge)
    );

    cx.update(|cx| GitCli::abort_merge(&repository, cx))
        .await
        .expect("abort");
    assert_eq!(
        cx.update(|cx| GitCli::in_progress(&repository, cx))
            .await
            .expect("in progress"),
        None
    );
    assert!(
        cx.update(|cx| GitCli::merge(&repository, "-x", cx))
            .await
            .is_err()
    );
}

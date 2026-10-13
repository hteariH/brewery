//! Runs `git` for the operations Zed's git layer lacks: merge, rebase, abort/continue,
//! reflog, show at revision.
//!
//! Every command runs inside `Repository::send_job`, so it is serialised with Zed's own git
//! jobs on that repository. The `git` binary and the environment are found the way Zed's
//! `LocalRepositoryState` finds them, and the command line is built like
//! `RealGitRepository`'s (`GitBinary::build_command`).

use anyhow::{Context as _, Result, anyhow, bail};
use futures::future::{self, Either};
pub use git::Oid;
pub use git::repository::RepoPath;
use gpui::{App, AsyncApp, BackgroundExecutor, Entity, SharedString, Task};
use project::git_store::{LocalRepositoryState, Repository, RepositoryState};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::time::Duration;

/// The result of one `git` process. A non-zero `status` is not an error; the caller decides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl GitOutput {
    pub fn success(&self) -> bool {
        self.status == 0
    }

    /// git's own message: stderr, or stdout when stderr is empty, trimmed.
    pub fn message(&self) -> String {
        let stderr = self.stderr.trim();
        if stderr.is_empty() {
            self.stdout.trim().to_string()
        } else {
            stderr.to_string()
        }
    }
}

/// How a merge, rebase or their `--continue` ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// HEAD moved: fast-forward, merge commit or rebased commits.
    Merged,
    /// Nothing to do; HEAD is unchanged.
    UpToDate,
    /// The operation stopped with these unmerged paths (sorted, repository-relative).
    Conflicts(Vec<RepoPath>),
    /// git refused or failed; its message.
    Failed(String),
}

/// A side of a conflict, in git's terms. During a rebase git swaps them: `Ours` is the branch
/// being rebased onto (upstream) and `Theirs` is the commit being replayed (your change), so
/// "Accept Yours" in a rebase maps to `Theirs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
}

/// A multi-step operation git has left in progress in the repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
}

/// One index stage of a conflicted file, as read for the merge editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StageBlob {
    /// The stage does not exist: no common ancestor (add/add), or that side deleted the file.
    Missing,
    /// Larger than the limit the caller gave; not read.
    TooLarge,
    Bytes(Vec<u8>),
}

/// The index stages of a conflicted file, in git's terms: `:1:` base, `:2:` ours, `:3:` theirs.
/// See [`Side`] and [`ConflictSides`] for which is the user's own side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictStages {
    pub base: StageBlob,
    pub ours: StageBlob,
    pub theirs: StageBlob,
}

/// Who is on which side of the conflicts the repository is stopped on, named for the user.
///
/// - Merge (and cherry-pick, or no operation, e.g. a stash pop): yours is the current branch
///   (stage 2, git's "ours"); theirs is what is merged in, stage 3 (`MERGE_HEAD`, named from
///   `MERGE_MSG` or a ref pointing at it; `CHERRY_PICK_HEAD`).
/// - Rebase: git checks out the upstream and replays your commits onto it, so stage 2 is the
///   branch being rebased *onto* and stage 3 is your commit. Yours is the branch being rebased
///   (`rebase-merge/head-name`), theirs the onto commit (`rebase-merge/onto`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictSides {
    pub operation: Option<Operation>,
    /// A branch name, or a short sha for a detached HEAD.
    pub yours: String,
    /// A branch or tag name, or a short sha; empty when unknown.
    pub theirs: String,
}

impl ConflictSides {
    /// The git side ("ours"/"theirs", stage 2/3) that holds the user's own changes.
    pub fn yours_stage(&self) -> Side {
        match self.operation {
            Some(Operation::Rebase) => Side::Theirs,
            _ => Side::Ours,
        }
    }

    /// The git side that holds the other changes.
    pub fn theirs_stage(&self) -> Side {
        match self.yours_stage() {
            Side::Ours => Side::Theirs,
            Side::Theirs => Side::Ours,
        }
    }
}

/// `git` operations on a project repository. Remote (collab) repositories are not supported.
pub struct GitCli;

impl GitCli {
    /// Runs `git <args>` in the repository's work directory. With a `timeout`, git is killed
    /// and the result is an error once it runs out (a hung process would block the
    /// repository's job queue).
    ///
    /// This is a raw escape hatch: callers must validate user strings themselves (refuse
    /// values starting with `-`, end revisions with `--`), as the typed helpers do.
    pub fn run(
        repo: &Entity<Repository>,
        args: Vec<String>,
        timeout: Option<Duration>,
        cx: &mut App,
    ) -> Task<Result<GitOutput>> {
        Self::job(
            repo,
            "brewery git",
            None,
            cx,
            async move |runner, executor| {
                let args = args.iter().map(String::as_str).collect::<Vec<_>>();
                match timeout {
                    Some(timeout) => runner.run_until(&args, executor.timer(timeout)).await,
                    None => runner.run(&args).await,
                }
            },
        )
    }

    /// `git merge --no-edit <branch>` into the current branch.
    pub fn merge(
        repo: &Entity<Repository>,
        branch: &str,
        cx: &mut App,
    ) -> Task<Result<MergeOutcome>> {
        let branch = branch.to_string();
        Self::job(
            repo,
            "brewery git merge",
            Some("git merge".into()),
            cx,
            async move |runner, _| runner.merge(&branch).await,
        )
    }

    /// `git rebase <onto>` of the current branch.
    pub fn rebase(
        repo: &Entity<Repository>,
        onto: &str,
        cx: &mut App,
    ) -> Task<Result<MergeOutcome>> {
        let onto = onto.to_string();
        Self::job(
            repo,
            "brewery git rebase",
            Some("git rebase".into()),
            cx,
            async move |runner, _| runner.rebase(&onto).await,
        )
    }

    pub fn abort_merge(repo: &Entity<Repository>, cx: &mut App) -> Task<Result<()>> {
        Self::job(
            repo,
            "brewery git merge --abort",
            None,
            cx,
            async move |runner, _| runner.abort_merge().await,
        )
    }

    /// Commits the merge once every conflict is resolved and staged.
    pub fn continue_merge(repo: &Entity<Repository>, cx: &mut App) -> Task<Result<MergeOutcome>> {
        Self::job(
            repo,
            "brewery git merge --continue",
            None,
            cx,
            async move |runner, _| runner.continue_merge().await,
        )
    }

    pub fn abort_rebase(repo: &Entity<Repository>, cx: &mut App) -> Task<Result<()>> {
        Self::job(
            repo,
            "brewery git rebase --abort",
            None,
            cx,
            async move |runner, _| runner.abort_rebase().await,
        )
    }

    pub fn continue_rebase(repo: &Entity<Repository>, cx: &mut App) -> Task<Result<MergeOutcome>> {
        Self::job(
            repo,
            "brewery git rebase --continue",
            Some("git rebase".into()),
            cx,
            async move |runner, _| runner.continue_rebase().await,
        )
    }

    pub fn abort_cherry_pick(repo: &Entity<Repository>, cx: &mut App) -> Task<Result<()>> {
        Self::job(
            repo,
            "brewery git cherry-pick --abort",
            None,
            cx,
            async move |runner, _| runner.abort_cherry_pick().await,
        )
    }

    /// Commits the picked change once every conflict is resolved and staged.
    pub fn continue_cherry_pick(
        repo: &Entity<Repository>,
        cx: &mut App,
    ) -> Task<Result<MergeOutcome>> {
        Self::job(
            repo,
            "brewery git cherry-pick --continue",
            None,
            cx,
            async move |runner, _| runner.continue_cherry_pick().await,
        )
    }

    /// The conflicted (unmerged) files, sorted: `git diff --name-only --diff-filter=U`. Unlike
    /// the repository snapshot, this is current right after a git command returns.
    pub fn unmerged_paths(repo: &Entity<Repository>, cx: &mut App) -> Task<Result<Vec<RepoPath>>> {
        Self::job(
            repo,
            "brewery git unmerged paths",
            None,
            cx,
            async move |runner, _| runner.unmerged_paths().await,
        )
    }

    /// Takes one side of conflicted files and marks them resolved: `checkout --ours|--theirs`
    /// then `add`, or `rm` where that side deleted the file. See [`Side`] for rebases.
    pub fn checkout_side(
        repo: &Entity<Repository>,
        paths: Vec<RepoPath>,
        side: Side,
        cx: &mut App,
    ) -> Task<Result<()>> {
        Self::job(
            repo,
            "brewery git checkout side",
            None,
            cx,
            async move |runner, _| runner.checkout_side(paths, side).await,
        )
    }

    /// Local branches from `git reflog` checkouts, most recent first, without duplicates.
    /// The current branch is included (normally first): ask for one more than you show when
    /// it is excluded.
    pub fn recent_branches(
        repo: &Entity<Repository>,
        limit: usize,
        cx: &mut App,
    ) -> Task<Result<Vec<String>>> {
        Self::job(
            repo,
            "brewery git reflog",
            None,
            cx,
            async move |runner, _| runner.recent_branches(limit).await,
        )
    }

    /// The text of a file at `rev`; `None` when the file does not exist there. An unknown
    /// revision, a directory or non-UTF-8 content is an error; use `show_bytes_at` for
    /// binary files.
    pub fn show_at(
        repo: &Entity<Repository>,
        rev: &str,
        path: RepoPath,
        cx: &mut App,
    ) -> Task<Result<Option<String>>> {
        let rev = rev.to_string();
        Self::job(
            repo,
            "brewery git show",
            None,
            cx,
            async move |runner, _| runner.show_at(&rev, &path).await,
        )
    }

    /// The exact bytes of a file at `rev`; `None` when the file does not exist there.
    pub fn show_bytes_at(
        repo: &Entity<Repository>,
        rev: &str,
        path: RepoPath,
        cx: &mut App,
    ) -> Task<Result<Option<Vec<u8>>>> {
        let rev = rev.to_string();
        Self::job(
            repo,
            "brewery git show",
            None,
            cx,
            async move |runner, _| runner.show_bytes_at(&rev, &path).await,
        )
    }

    /// The bytes of `old_path` at `rev` as git would check them out at `path` (eol, `ident`,
    /// LFS and other smudge filters applied, with `path`'s attributes):
    /// `git cat-file --filters --path=<path> <rev>:<old_path>`. `None` when the file does not
    /// exist at `rev`.
    pub fn checked_out_bytes_at(
        repo: &Entity<Repository>,
        rev: &str,
        old_path: RepoPath,
        path: RepoPath,
        cx: &mut App,
    ) -> Task<Result<Option<Vec<u8>>>> {
        let rev = rev.to_string();
        Self::job(
            repo,
            "brewery git cat-file",
            None,
            cx,
            async move |runner, _| runner.checked_out_bytes_at(&rev, &old_path, &path).await,
        )
    }

    /// Replaces the working tree copy of `path` with its version at `rev`, leaving the index
    /// alone: `git restore --source=<rev> --worktree -- <path>`.
    pub fn restore_worktree(
        repo: &Entity<Repository>,
        rev: &str,
        path: RepoPath,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let rev = rev.to_string();
        Self::job(
            repo,
            "brewery git restore",
            Some("git restore".into()),
            cx,
            async move |runner, _| {
                check_revision(&rev, "revision")?;
                let source = format!("--source={rev}");
                let output = runner
                    .run(&["restore", &source, "--worktree", "--", path.as_unix_str()])
                    .await?;
                if !output.success() {
                    bail!("{}", output.message());
                }
                Ok(())
            },
        )
    }

    /// The commit `rev` names (`git rev-parse --verify --quiet <rev>^{commit}`); `None` when
    /// it names no commit. `-`-prefixed input is an error.
    pub fn resolve_commit(
        repo: &Entity<Repository>,
        rev: &str,
        cx: &mut App,
    ) -> Task<Result<Option<Oid>>> {
        let rev = rev.to_string();
        Self::job(
            repo,
            "brewery git rev-parse",
            None,
            cx,
            async move |runner, _| runner.resolve_commit(&rev).await,
        )
    }

    /// The size in bytes of a file at `rev`, without reading it; `None` when the file does not
    /// exist there. An unknown revision or a directory is an error.
    pub fn blob_size(
        repo: &Entity<Repository>,
        rev: &str,
        path: RepoPath,
        cx: &mut App,
    ) -> Task<Result<Option<u64>>> {
        let rev = rev.to_string();
        Self::job(
            repo,
            "brewery git cat-file",
            None,
            cx,
            async move |runner, _| runner.blob_size(&rev, &path).await,
        )
    }

    /// `git rev-list --max-count=<limit> <range>`, newest first.
    pub fn rev_list(
        repo: &Entity<Repository>,
        range: &str,
        limit: usize,
        cx: &mut App,
    ) -> Task<Result<Vec<Oid>>> {
        let range = range.to_string();
        Self::job(
            repo,
            "brewery git rev-list",
            None,
            cx,
            async move |runner, _| runner.rev_list(&range, limit).await,
        )
    }

    /// The index stages of a conflicted file; a stage larger than `max_size` bytes is not read.
    pub fn conflict_stages(
        repo: &Entity<Repository>,
        path: RepoPath,
        max_size: u64,
        cx: &mut App,
    ) -> Task<Result<ConflictStages>> {
        Self::job(
            repo,
            "brewery git conflict stages",
            None,
            cx,
            async move |runner, _| runner.conflict_stages(&path, max_size).await,
        )
    }

    /// The operation in progress and the names of the user's side and the other side.
    pub fn conflict_sides(repo: &Entity<Repository>, cx: &mut App) -> Task<Result<ConflictSides>> {
        Self::job(
            repo,
            "brewery git conflict sides",
            None,
            cx,
            async move |runner, _| runner.conflict_sides().await,
        )
    }

    /// Marks files resolved with their working tree content: `git add -- <paths>`.
    pub fn mark_resolved(
        repo: &Entity<Repository>,
        paths: Vec<RepoPath>,
        cx: &mut App,
    ) -> Task<Result<()>> {
        Self::job(repo, "brewery git add", None, cx, async move |runner, _| {
            runner.mark_resolved(paths).await
        })
    }

    pub fn in_progress(repo: &Entity<Repository>, cx: &mut App) -> Task<Result<Option<Operation>>> {
        Self::job(
            repo,
            "brewery git in progress",
            None,
            cx,
            async move |runner, _| runner.in_progress().await,
        )
    }

    fn job<R, Fut>(
        repo: &Entity<Repository>,
        description: &'static str,
        status: Option<SharedString>,
        cx: &mut App,
        f: impl FnOnce(GitRunner, BackgroundExecutor) -> Fut + Send + 'static,
    ) -> Task<Result<R>>
    where
        R: Send + 'static,
        Fut: Future<Output = Result<R>> + Send + 'static,
    {
        let work_dir = repo.read(cx).work_directory_abs_path.to_path_buf();
        let result = repo.update(cx, |repo, _| {
            repo.send_job(description, status, move |state, cx| async move {
                match state {
                    RepositoryState::Local(local) => {
                        let runner = GitRunner::for_local(&local, work_dir, &cx)?;
                        let executor = cx.background_executor().clone();
                        executor.spawn(f(runner, executor.clone())).await
                    }
                    RepositoryState::Remote(_) => Err(anyhow!("not supported for remote projects")),
                }
            })
        });
        cx.spawn(async move |_| {
            result
                .await
                .map_err(|_| anyhow!("the git job was dropped"))?
        })
    }
}

/// Runs `git` processes for one repository. Built inside a repository job.
pub(crate) struct GitRunner {
    pub(crate) git_binary: PathBuf,
    pub(crate) work_dir: PathBuf,
    pub(crate) git_dir: PathBuf,
    pub(crate) env: HashMap<String, String>,
    pub(crate) is_trusted: bool,
}

impl GitRunner {
    fn for_local(local: &LocalRepositoryState, work_dir: PathBuf, cx: &AsyncApp) -> Result<Self> {
        #[allow(unused_mut)]
        let mut env = local
            .environment
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<HashMap<_, _>>();
        #[cfg(test)]
        if let Ok(extra) = TEST_ENV.lock() {
            env.extend(extra.clone());
        }
        let git_binary = find_git(&env, &work_dir, cx)?;
        Ok(Self {
            git_binary,
            work_dir,
            git_dir: local.backend.path(),
            env,
            is_trusted: local.backend.is_trusted(),
        })
    }

    pub(crate) async fn run(&self, args: &[&str]) -> Result<GitOutput> {
        self.run_raw(args, &[], future::pending())
            .await
            .map(RawOutput::text)
    }

    /// Like `run`, but kills git and fails once `deadline` completes.
    pub(crate) async fn run_until(
        &self,
        args: &[&str],
        deadline: impl Future<Output = ()>,
    ) -> Result<GitOutput> {
        self.run_raw(args, &[], deadline).await.map(RawOutput::text)
    }

    async fn run_raw(
        &self,
        args: &[&str],
        extra_env: &[(&str, &str)],
        deadline: impl Future<Output = ()>,
    ) -> Result<RawOutput> {
        let subcommand = args.first().copied().unwrap_or_default();
        let mut command = util::command::new_command(&self.git_binary);
        command.current_dir(&self.work_dir);
        // Same safety and parsing options as Zed's `GitBinary::build_command`.
        command.args(["-c", "core.fsmonitor=false"]);
        command.args(["-c", "log.showSignature=false"]);
        command.arg("--no-optional-locks");
        command.arg("--no-pager");
        if !self.is_trusted {
            command.args(["-c", "core.hooksPath=/dev/null"]);
            command.args(["-c", "core.sshCommand=ssh"]);
            command.args(["-c", "credential.helper="]);
            command.args(["-c", "protocol.ext.allow=never"]);
            command.args(["-c", "diff.external="]);
        }
        let mut args = args.iter();
        if let Some(subcommand) = args.next() {
            command.arg(subcommand);
            if !self.is_trusted && *subcommand == "diff" {
                command.arg("--no-ext-diff");
            }
            command.args(args);
        }
        command.envs(&self.env);
        // Never wait for an editor or a terminal prompt.
        command.env("GIT_EDITOR", "true");
        command.env("GIT_SEQUENCE_EDITOR", "true");
        command.env("GIT_MERGE_AUTOEDIT", "no");
        command.env("GIT_TERMINAL_PROMPT", "0");
        for (key, value) in extra_env {
            command.env(key, value);
        }
        // Dropping the output future (deadline reached) kills git.
        command.kill_on_drop(true);
        let output = pin!(command.output());
        let output = match future::select(output, pin!(deadline)).await {
            Either::Left((output, _)) => {
                output.with_context(|| format!("running {:?}", self.git_binary))?
            }
            Either::Right(_) => bail!("git {subcommand} timed out"),
        };
        Ok(RawOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    pub(crate) async fn merge(&self, branch: &str) -> Result<MergeOutcome> {
        check_revision(branch, "branch")?;
        let before = self.head().await?;
        let output = self.run(&["merge", "--no-edit", branch]).await?;
        self.outcome(output, before).await
    }

    pub(crate) async fn rebase(&self, onto: &str) -> Result<MergeOutcome> {
        check_revision(onto, "branch")?;
        let before = self.head().await?;
        let output = self.run(&["rebase", onto]).await?;
        self.outcome(output, before).await
    }

    pub(crate) async fn abort_merge(&self) -> Result<()> {
        self.run_checked(&["merge", "--abort"]).await
    }

    pub(crate) async fn continue_merge(&self) -> Result<MergeOutcome> {
        let before = self.head().await?;
        let output = self.run(&["merge", "--continue"]).await?;
        self.outcome(output, before).await
    }

    pub(crate) async fn abort_rebase(&self) -> Result<()> {
        self.run_checked(&["rebase", "--abort"]).await
    }

    pub(crate) async fn continue_rebase(&self) -> Result<MergeOutcome> {
        let output = self.run(&["rebase", "--continue"]).await?;
        // A finished rebase is `Merged` even when the last commit was dropped as empty and HEAD
        // did not move; `None` makes `outcome` skip the HEAD comparison.
        self.outcome(output, None).await
    }

    pub(crate) async fn abort_cherry_pick(&self) -> Result<()> {
        self.run_checked(&["cherry-pick", "--abort"]).await
    }

    pub(crate) async fn continue_cherry_pick(&self) -> Result<MergeOutcome> {
        let before = self.head().await?;
        let output = self.run(&["cherry-pick", "--continue"]).await?;
        self.outcome(output, before).await
    }

    pub(crate) async fn checkout_side(&self, paths: Vec<RepoPath>, side: Side) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let (flag, stage) = match side {
            Side::Ours => ("--ours", "2"),
            Side::Theirs => ("--theirs", "3"),
        };
        let mut take = Vec::new();
        let mut remove = Vec::new();
        for path in &paths {
            let path = path.as_unix_str();
            let stages = self
                .run_literal(&["ls-files", "-u", "-z", "--", path])
                .await?;
            if !stages.success() {
                bail!("{}", stages.message());
            }
            // Records are "<mode> <oid> <stage>\t<path>"; a conflicted path without the chosen
            // stage was deleted on that side.
            let has_stage = stages.stdout.split('\0').any(|record| {
                record
                    .split_once('\t')
                    .and_then(|(info, _)| info.split(' ').nth(2))
                    == Some(stage)
            });
            if !stages.stdout.is_empty() && !has_stage {
                remove.push(path);
            } else {
                take.push(path);
            }
        }
        if !take.is_empty() {
            let mut checkout = vec!["checkout", flag, "--"];
            checkout.extend(&take);
            self.run_literal_checked(&checkout).await?;
            let mut add = vec!["add", "--"];
            add.extend(&take);
            self.run_literal_checked(&add).await?;
        }
        if !remove.is_empty() {
            let mut rm = vec!["rm", "--quiet", "--force", "--"];
            rm.extend(&remove);
            self.run_literal_checked(&rm).await?;
        }
        Ok(())
    }

    /// Empty when the reflog cannot be read (e.g. a repository without commits).
    pub(crate) async fn recent_branches(&self, limit: usize) -> Result<Vec<String>> {
        let reflog = self
            .run(&["reflog", "show", "--format=%gs", "-n", "1000", "HEAD", "--"])
            .await?;
        if !reflog.success() {
            return Ok(Vec::new());
        }
        let local = self
            .run(&["for-each-ref", "--format=%(refname)", "refs/heads"])
            .await?;
        if !local.success() {
            bail!("{}", local.message());
        }
        let local = local
            .stdout
            .lines()
            .filter_map(|line| line.strip_prefix("refs/heads/"))
            .collect::<HashSet<_>>();

        let mut seen = HashSet::new();
        let mut recent = Vec::new();
        for line in reflog.stdout.lines() {
            let Some((from, to)) = line
                .strip_prefix("checkout: moving from ")
                .and_then(|rest| rest.split_once(" to "))
            else {
                continue;
            };
            for name in [to.trim(), from.trim()] {
                if recent.len() >= limit {
                    return Ok(recent);
                }
                if local.contains(name) && seen.insert(name) {
                    recent.push(name.to_string());
                }
            }
        }
        recent.truncate(limit);
        Ok(recent)
    }

    pub(crate) async fn show_at(&self, rev: &str, path: &RepoPath) -> Result<Option<String>> {
        match self.show_bytes_at(rev, path).await? {
            Some(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| anyhow!("{} at {rev} is not UTF-8 text", path.as_unix_str())),
            None => Ok(None),
        }
    }

    pub(crate) async fn show_bytes_at(
        &self,
        rev: &str,
        path: &RepoPath,
    ) -> Result<Option<Vec<u8>>> {
        check_revision(rev, "revision")?;
        let tree = format!("{rev}^{{tree}}");
        let revision = self
            .run(&["rev-parse", "--verify", "--quiet", &tree])
            .await?;
        if !revision.success() {
            bail!("unknown revision {rev}");
        }
        let object = format!("{rev}:{}", path.as_unix_str());
        if !self.run(&["cat-file", "-e", &object]).await?.success() {
            return Ok(None);
        }
        let blob = self
            .run_raw(&["cat-file", "blob", &object], &[], future::pending())
            .await?;
        if blob.status != 0 {
            bail!("{}", blob.text().message());
        }
        Ok(Some(blob.stdout))
    }

    pub(crate) async fn blob_size(&self, rev: &str, path: &RepoPath) -> Result<Option<u64>> {
        check_revision(rev, "revision")?;
        let tree = format!("{rev}^{{tree}}");
        if !self
            .run(&["rev-parse", "--verify", "--quiet", &tree])
            .await?
            .success()
        {
            bail!("unknown revision {rev}");
        }
        let object = format!("{rev}:{}", path.as_unix_str());
        if !self.run(&["cat-file", "-e", &object]).await?.success() {
            return Ok(None);
        }
        let kind = self.run(&["cat-file", "-t", &object]).await?;
        if kind.stdout.trim() != "blob" {
            bail!("{} at {rev} is not a file", path.as_unix_str());
        }
        let size = self.run(&["cat-file", "-s", &object]).await?;
        if !size.success() {
            bail!("{}", size.message());
        }
        Ok(Some(size.stdout.trim().parse()?))
    }

    pub(crate) async fn checked_out_bytes_at(
        &self,
        rev: &str,
        old_path: &RepoPath,
        path: &RepoPath,
    ) -> Result<Option<Vec<u8>>> {
        check_revision(rev, "revision")?;
        let object = format!("{rev}:{}", old_path.as_unix_str());
        if !self.run(&["cat-file", "-e", &object]).await?.success() {
            return Ok(None);
        }
        let attributes_path = format!("--path={}", path.as_unix_str());
        let blob = self
            .run_raw(
                &["cat-file", "--filters", &attributes_path, &object],
                &[],
                future::pending(),
            )
            .await?;
        if blob.status != 0 {
            bail!("{}", blob.text().message());
        }
        Ok(Some(blob.stdout))
    }

    pub(crate) async fn resolve_commit(&self, rev: &str) -> Result<Option<Oid>> {
        check_revision(rev, "revision")?;
        let commit = format!("{rev}^{{commit}}");
        let output = self
            .run(&["rev-parse", "--verify", "--quiet", &commit])
            .await?;
        if !output.success() {
            return Ok(None);
        }
        output.stdout.trim().parse::<Oid>().map(Some)
    }

    pub(crate) async fn rev_list(&self, range: &str, limit: usize) -> Result<Vec<Oid>> {
        check_revision(range, "range")?;
        let max_count = format!("--max-count={limit}");
        let output = self.run(&["rev-list", &max_count, range, "--"]).await?;
        if !output.success() {
            bail!("{}", output.message());
        }
        output
            .stdout
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| line.trim().parse::<Oid>())
            .collect()
    }

    pub(crate) async fn in_progress(&self) -> Result<Option<Operation>> {
        let git_dir = &self.git_dir;
        let rebase_apply = git_dir.join("rebase-apply");
        let operation = if git_dir.join("rebase-merge").is_dir()
            || (rebase_apply.is_dir() && !rebase_apply.join("applying").exists())
        {
            Some(Operation::Rebase)
        } else if git_dir.join("MERGE_HEAD").is_file() {
            Some(Operation::Merge)
        } else if git_dir.join("CHERRY_PICK_HEAD").is_file() {
            Some(Operation::CherryPick)
        } else {
            None
        };
        Ok(operation)
    }

    pub(crate) async fn conflict_stages(
        &self,
        path: &RepoPath,
        max_size: u64,
    ) -> Result<ConflictStages> {
        let object = |stage: u8| format!(":{stage}:{}", path.as_unix_str());
        Ok(ConflictStages {
            base: self.stage_blob(&object(1), max_size).await?,
            ours: self.stage_blob(&object(2), max_size).await?,
            theirs: self.stage_blob(&object(3), max_size).await?,
        })
    }

    async fn stage_blob(&self, object: &str, max_size: u64) -> Result<StageBlob> {
        if !self.run(&["cat-file", "-e", object]).await?.success() {
            return Ok(StageBlob::Missing);
        }
        let size = self.run(&["cat-file", "-s", object]).await?;
        if !size.success() {
            bail!("{}", size.message());
        }
        if size.stdout.trim().parse::<u64>()? > max_size {
            return Ok(StageBlob::TooLarge);
        }
        let blob = self
            .run_raw(&["cat-file", "blob", object], &[], future::pending())
            .await?;
        if blob.status != 0 {
            bail!("{}", blob.text().message());
        }
        Ok(StageBlob::Bytes(blob.stdout))
    }

    pub(crate) async fn conflict_sides(&self) -> Result<ConflictSides> {
        let operation = self.in_progress().await?;
        let current = self.current_branch().await?;
        let (yours, theirs) = match operation {
            Some(Operation::Rebase) => {
                let dir = [
                    self.git_dir.join("rebase-merge"),
                    self.git_dir.join("rebase-apply"),
                ]
                .into_iter()
                .find(|dir| dir.is_dir());
                let read = |name: &str| {
                    dir.as_ref()
                        .and_then(|dir| std::fs::read_to_string(dir.join(name)).ok())
                        .map(|text| text.trim().to_string())
                        .filter(|text| !text.is_empty())
                };
                let yours = match read("head-name") {
                    Some(head) if head != "detached HEAD" => head
                        .strip_prefix("refs/heads/")
                        .unwrap_or(&head)
                        .to_string(),
                    _ => match read("orig-head") {
                        Some(sha) => short_sha(&sha),
                        None => current.clone(),
                    },
                };
                let theirs = match read("onto") {
                    Some(onto) => self.name_commit(&onto, &yours).await?,
                    None => String::new(),
                };
                (yours, theirs)
            }
            Some(Operation::Merge) => {
                let message = std::fs::read_to_string(self.git_dir.join("MERGE_MSG")).ok();
                let theirs = match message.as_deref().and_then(merged_name_from_message) {
                    Some(name) => name,
                    None => self.name_head_file("MERGE_HEAD", &current).await?,
                };
                (current, theirs)
            }
            Some(Operation::CherryPick) => {
                let theirs = self.name_head_file("CHERRY_PICK_HEAD", &current).await?;
                (current, theirs)
            }
            None => (current, String::new()),
        };
        Ok(ConflictSides {
            operation,
            yours,
            theirs,
        })
    }

    pub(crate) async fn mark_resolved(&self, paths: Vec<RepoPath>) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut add = vec!["add", "--"];
        add.extend(paths.iter().map(|path| path.as_unix_str()));
        self.run_literal_checked(&add).await
    }

    /// The current branch, or HEAD's short sha when detached.
    async fn current_branch(&self) -> Result<String> {
        let branch = self.run(&["symbolic-ref", "--short", "-q", "HEAD"]).await?;
        if branch.success() && !branch.stdout.trim().is_empty() {
            return Ok(branch.stdout.trim().to_string());
        }
        Ok(self
            .head()
            .await?
            .map(|sha| short_sha(&sha))
            .unwrap_or_default())
    }

    /// Names the first commit in `.git/<file>` (e.g. `MERGE_HEAD`).
    async fn name_head_file(&self, file: &str, exclude: &str) -> Result<String> {
        let Some(sha) = std::fs::read_to_string(self.git_dir.join(file))
            .ok()
            .and_then(|text| text.lines().next().map(|line| line.trim().to_string()))
            .filter(|sha| !sha.is_empty())
        else {
            return Ok(String::new());
        };
        self.name_commit(&sha, exclude).await
    }

    /// A local branch, then a remote branch, then a tag pointing at `sha` (other than
    /// `exclude`), else the short sha.
    async fn name_commit(&self, sha: &str, exclude: &str) -> Result<String> {
        check_revision(sha, "commit")?;
        let points_at = format!("--points-at={sha}");
        let refs = self
            .run(&[
                "for-each-ref",
                "--format=%(refname:short)",
                &points_at,
                "refs/heads",
                "refs/remotes",
                "refs/tags",
            ])
            .await?;
        if refs.success()
            && let Some(name) = refs
                .stdout
                .lines()
                .map(str::trim)
                .find(|name| !name.is_empty() && *name != exclude && !name.ends_with("/HEAD"))
        {
            return Ok(name.to_string());
        }
        Ok(short_sha(sha))
    }

    /// Runs with pathspec magic off, so file names are taken literally.
    async fn run_literal(&self, args: &[&str]) -> Result<GitOutput> {
        self.run_raw(args, &[("GIT_LITERAL_PATHSPECS", "1")], future::pending())
            .await
            .map(RawOutput::text)
    }

    async fn run_literal_checked(&self, args: &[&str]) -> Result<()> {
        let output = self.run_literal(args).await?;
        if !output.success() {
            bail!("{}", output.message());
        }
        Ok(())
    }

    async fn run_checked(&self, args: &[&str]) -> Result<()> {
        let output = self.run(args).await?;
        if !output.success() {
            bail!("{}", output.message());
        }
        Ok(())
    }

    async fn head(&self) -> Result<Option<String>> {
        let output = self
            .run(&["rev-parse", "--verify", "--quiet", "HEAD"])
            .await?;
        Ok(output.success().then(|| output.stdout.trim().to_string()))
    }

    pub(crate) async fn unmerged_paths(&self) -> Result<Vec<RepoPath>> {
        let output = self
            .run(&["diff", "--name-only", "--diff-filter=U", "-z"])
            .await?;
        if !output.success() {
            bail!("{}", output.message());
        }
        let mut paths = output
            .stdout
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(RepoPath::new)
            .collect::<Result<Vec<_>>>()?;
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    async fn outcome(&self, output: GitOutput, before: Option<String>) -> Result<MergeOutcome> {
        if output.success() {
            return Ok(if before.is_some() && self.head().await? == before {
                MergeOutcome::UpToDate
            } else {
                MergeOutcome::Merged
            });
        }
        // A rebase that stopped without conflicts (a failing hook, an `edit` step) is `Failed`
        // with git's message; `in_progress` tells the caller it can still be continued.
        let conflicts = self.unmerged_paths().await?;
        if conflicts.is_empty() {
            return Ok(MergeOutcome::Failed(output.message()));
        }
        Ok(MergeOutcome::Conflicts(conflicts))
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars()
        .take(crate::revision_diff_view::SHORT_SHA_LEN)
        .collect()
}

/// What `git merge` named in its message: `Merge branch 'feature'`, `Merge remote-tracking
/// branch 'origin/x'`, `Merge tag 'v1'`, `Merge commit '<sha>'` (shortened).
fn merged_name_from_message(message: &str) -> Option<String> {
    let first = message.lines().next()?;
    if !first.starts_with("Merge ") {
        return None;
    }
    let start = first.find('\'')? + 1;
    let length = first[start..].find('\'')?;
    let name = &first[start..start + length];
    if name.is_empty() {
        return None;
    }
    if first.starts_with("Merge commit ") {
        return Some(short_sha(name));
    }
    Some(name.to_string())
}

/// Refuses user strings git would read as options.
pub(crate) fn check_revision(value: &str, what: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("empty {what}");
    }
    if value.starts_with('-') {
        bail!("refusing {what} {value:?}: it starts with '-'");
    }
    if value.contains('\0') {
        bail!("refusing {what} with a NUL character");
    }
    Ok(())
}

/// The `git` Zed's `LocalRepositoryState` picks: the project environment's PATH, then this
/// process's PATH, then the bundled one. Mirrors `LocalRepositoryState::new` in
/// crates/project/src/git_store.rs (the `which_in` / `fs.open_repo` lines) and the bundled
/// fallback in `RealFs`; keep them in step when merging Zed.
fn find_git(env: &HashMap<String, String>, work_dir: &Path, cx: &AsyncApp) -> Result<PathBuf> {
    env.get("PATH")
        .and_then(|paths| which::which_in("git", Some(paths), work_dir).ok())
        .or_else(|| which::which("git").ok())
        .or_else(|| cx.update(|cx| cx.path_for_auxiliary_executable("git").ok()))
        .context("git not found on PATH")
}

struct RawOutput {
    status: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl RawOutput {
    fn text(self) -> GitOutput {
        GitOutput {
            status: self.status,
            stdout: String::from_utf8_lossy(&self.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&self.stderr).into_owned(),
        }
    }
}

/// Extra environment for jobs run through `GitCli` in tests (hermetic git config).
#[cfg(test)]
static TEST_ENV: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub(crate) fn set_test_env(env: HashMap<String, String>) {
    if let Ok(mut extra) = TEST_ENV.lock() {
        *extra = env.into_iter().collect();
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;

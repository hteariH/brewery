//! IntelliJ-style Branches popup: top actions, Recent / Local / Remote groups and a per-branch
//! submenu (checkout, new branch from, rebase, merge, compare with current, rename, delete).
//! It replaces Zed's branch picker in the title bar (through
//! `git_ui_core::set_branch_picker_builder`) and for `git::Branch`.
//!
//! Checkout, create, rename and delete go through Zed's `Repository` methods; merge, rebase,
//! detached checkout, the smart checkout's stash and the reflog go through [`GitCli`]. Update
//! Project and Push dispatch Zed's `git::Pull` / `git::Push`, and deleting a remote branch
//! pushes through Zed's `Repository::push`, so credentials stay with Zed's askpass. Stashes…
//! opens Zed's stash picker (`git::ViewStash`). Zed's repository
//! watcher sees the `.git` changes these commands make and rescans, so the title bar and the
//! git panel refresh by themselves.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use fuzzy_nucleo::{Case, LengthPenalty, StringMatchCandidate};
use git::repository::{AskPassDelegate, Branch};
use git_ui_core::askpass_modal::AskPassModal;
use gpui::{
    Action, Anchor, App, AsyncWindowContext, Bounds, ClickEvent, Context, DismissEvent, Entity,
    EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, Pixels, Point, PromptLevel,
    Render, SharedString, Subscription, Task, WeakEntity, Window, anchored, canvas, deferred, rems,
};
use picker::{Picker, PickerDelegate};
use project::git_store::{Repository, RepositoryEvent};
use ui::{
    ContextMenu, ContextMenuEntry, DocumentationSide, HighlightedLabel, ListItem, ListItemSpacing,
    ListSubHeader, prelude::*,
};
use workspace::notifications::NotificationId;
use workspace::{ModalView, Toast, Workspace};

use crate::cli::{GitCli, MergeOutcome, Operation, check_revision};
use crate::compare;
use crate::conflicts_dialog::{self, ConflictsDialog};

/// How many recent branches the Recent group shows.
const RECENT_LIMIT: usize = 5;

// ---------------------------------------------------------------------------------------------
// Rows

/// The actions at the top of the popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TopAction {
    NewBranch,
    CheckoutRevision,
    UpdateProject,
    Push,
    Stashes,
}

impl TopAction {
    pub(crate) const ALL: [TopAction; 5] = [
        TopAction::NewBranch,
        TopAction::CheckoutRevision,
        TopAction::UpdateProject,
        TopAction::Push,
        TopAction::Stashes,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            TopAction::NewBranch => "New Branch…",
            TopAction::CheckoutRevision => "Checkout Tag or Revision…",
            TopAction::UpdateProject => "Update Project",
            TopAction::Push => "Push…",
            TopAction::Stashes => "Stashes…",
        }
    }

    fn icon(self) -> IconName {
        match self {
            TopAction::NewBranch => IconName::Plus,
            TopAction::CheckoutRevision => IconName::Hash,
            TopAction::UpdateProject => IconName::ArrowDown,
            TopAction::Push => IconName::ArrowUp,
            TopAction::Stashes => IconName::Archive,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Header {
    Recent,
    Local,
    Remote,
    /// One remote inside the Remote group.
    RemoteName(SharedString),
}

impl Header {
    pub(crate) fn title(&self) -> SharedString {
        match self {
            Header::Recent => "Recent".into(),
            Header::Local => "Local".into(),
            Header::Remote => "Remote".into(),
            Header::RemoteName(name) => name.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {
    Recent,
    Local,
    Remote,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BranchRow {
    pub branch: Branch,
    pub group: Group,
    pub is_current: bool,
    /// Matched character positions in `branch.name()`.
    pub positions: Vec<usize>,
}

/// The single row shown while the search field is used as a text input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InputRow {
    NewBranch { name: String, from: Option<String> },
    Rename { from: String, to: String },
    CheckoutRevision { rev: String },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Row {
    Action {
        action: TopAction,
        positions: Vec<usize>,
    },
    Header(Header),
    Branch(BranchRow),
    Input(InputRow),
}

impl Row {
    fn as_branch(&self) -> Option<&BranchRow> {
        match self {
            Row::Branch(row) => Some(row),
            _ => None,
        }
    }

    fn is_selectable(&self) -> bool {
        match self {
            Row::Header(_) => false,
            Row::Input(input) => input.is_ready(),
            Row::Action { .. } | Row::Branch(_) => true,
        }
    }
}

impl InputRow {
    fn is_ready(&self) -> bool {
        match self {
            InputRow::NewBranch { name, .. } => !name.is_empty(),
            InputRow::Rename { from, to } => !to.is_empty() && to != from,
            InputRow::CheckoutRevision { rev } => !rev.is_empty(),
        }
    }

    fn label(&self) -> String {
        match self {
            InputRow::NewBranch { name, from } if name.is_empty() => match from {
                Some(from) => format!("Type a name for the new branch from '{from}'"),
                None => "Type a name for the new branch".to_string(),
            },
            InputRow::NewBranch { name, from } => match from {
                Some(from) => format!("Create branch '{name}' from '{from}'"),
                None => format!("Create branch '{name}'"),
            },
            InputRow::Rename { from, to } if to.is_empty() || to == from => {
                format!("Type a new name for '{from}'")
            }
            InputRow::Rename { from, to } => format!("Rename '{from}' to '{to}'"),
            InputRow::CheckoutRevision { rev } if rev.is_empty() => {
                "Type a tag, branch or commit".to_string()
            }
            InputRow::CheckoutRevision { rev } => format!("Checkout '{rev}' (detached HEAD)"),
        }
    }
}

/// The rows of the popup and the row to select.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Grouped {
    pub rows: Vec<Row>,
    /// The best match for a query, else the first branch row.
    pub selected: Option<usize>,
}

/// Groups and orders the popup rows: the top actions, Recent (up to five, current excluded,
/// most recent first), Local and Remote (one sub-group per remote), each sorted by name.
/// A non-empty `query` keeps only the fuzzy matches (Zed's matcher) in every group and
/// selects the best one.
pub(crate) fn group_rows(
    branches: &[Branch],
    current: Option<&str>,
    recent: &[String],
    query: &str,
) -> Grouped {
    let mut local: Vec<&Branch> = branches.iter().filter(|b| !b.is_remote()).collect();
    local.sort_by(|a, b| a.name().cmp(b.name()));
    let mut remote: Vec<&Branch> = branches
        .iter()
        .filter(|b| b.is_remote() && !b.name().ends_with("/HEAD"))
        .collect();
    remote.sort_by(|a, b| a.name().cmp(b.name()));

    // Candidate ids: actions first, then local, then remote branches.
    let local_base = TopAction::ALL.len();
    let remote_base = local_base + local.len();
    let query = query.trim();
    let matches: Option<HashMap<usize, (f64, Vec<usize>)>> = (!query.is_empty()).then(|| {
        let candidates = TopAction::ALL
            .iter()
            .map(|action| action.label())
            .chain(local.iter().map(|b| b.name()))
            .chain(remote.iter().map(|b| b.name()))
            .enumerate()
            .map(|(id, text)| StringMatchCandidate::new(id, text.to_string()))
            .collect::<Vec<_>>();
        fuzzy_nucleo::match_strings(
            &candidates,
            query,
            Case::Smart,
            LengthPenalty::On,
            usize::MAX,
        )
        .into_iter()
        .map(|m| (m.candidate_id, (m.score, m.positions)))
        .collect()
    });
    let hit = |id: usize| -> Option<(f64, Vec<usize>)> {
        match &matches {
            None => Some((0.0, Vec::new())),
            Some(matches) => matches.get(&id).cloned(),
        }
    };

    let mut rows = Vec::new();
    let mut scores: Vec<(usize, f64)> = Vec::new();
    let mut push_scored = |rows: &mut Vec<Row>, row: Row, score: f64| {
        scores.push((rows.len(), score));
        rows.push(row);
    };

    for (id, action) in TopAction::ALL.iter().enumerate() {
        if let Some((score, positions)) = hit(id) {
            push_scored(
                &mut rows,
                Row::Action {
                    action: *action,
                    positions,
                },
                score,
            );
        }
    }

    let branch_row = |branch: &Branch, group: Group, positions: Vec<usize>| {
        Row::Branch(BranchRow {
            branch: branch.clone(),
            group,
            is_current: !branch.is_remote() && current == Some(branch.name()),
            positions,
        })
    };

    let recent_rows = recent
        .iter()
        .filter(|name| current != Some(name.as_str()))
        .filter_map(|name| {
            local
                .iter()
                .position(|b| b.name() == name.as_str())
                .map(|ix| (ix, local[ix]))
        })
        .take(RECENT_LIMIT)
        .filter_map(|(ix, branch)| hit(local_base + ix).map(|hit| (branch, hit)))
        .collect::<Vec<_>>();
    if !recent_rows.is_empty() {
        rows.push(Row::Header(Header::Recent));
        for (branch, (score, positions)) in recent_rows {
            push_scored(
                &mut rows,
                branch_row(branch, Group::Recent, positions),
                score,
            );
        }
    }

    let local_rows = local
        .iter()
        .enumerate()
        .filter_map(|(ix, branch)| hit(local_base + ix).map(|hit| (*branch, hit)))
        .collect::<Vec<_>>();
    if !local_rows.is_empty() {
        rows.push(Row::Header(Header::Local));
        for (branch, (score, positions)) in local_rows {
            push_scored(
                &mut rows,
                branch_row(branch, Group::Local, positions),
                score,
            );
        }
    }

    let mut by_remote: BTreeMap<&str, Vec<(&Branch, (f64, Vec<usize>))>> = BTreeMap::new();
    for (ix, branch) in remote.iter().enumerate() {
        if let Some(hit) = hit(remote_base + ix) {
            by_remote
                .entry(branch.remote_name().unwrap_or_default())
                .or_default()
                .push((*branch, hit));
        }
    }
    if !by_remote.is_empty() {
        rows.push(Row::Header(Header::Remote));
        for (remote_name, branches) in by_remote {
            rows.push(Row::Header(Header::RemoteName(
                remote_name.to_string().into(),
            )));
            for (branch, (score, positions)) in branches {
                push_scored(
                    &mut rows,
                    branch_row(branch, Group::Remote, positions),
                    score,
                );
            }
        }
    }

    let selected = if matches.is_some() {
        // Highest score; the earliest row wins a tie.
        scores
            .iter()
            .fold(None::<(usize, f64)>, |best, &(ix, score)| match best {
                Some((_, best_score)) if best_score >= score => best,
                _ => Some((ix, score)),
            })
            .map(|(ix, _)| ix)
    } else {
        rows.iter()
            .position(|row| row.as_branch().is_some())
            .or_else(|| rows.iter().position(Row::is_selectable))
    };

    Grouped { rows, selected }
}

// ---------------------------------------------------------------------------------------------
// Outcomes

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Success,
    Conflicts,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OutcomeToast {
    pub kind: ToastKind,
    pub message: String,
}

impl OutcomeToast {
    pub(crate) fn success(message: impl Into<String>) -> Self {
        Self {
            kind: ToastKind::Success,
            message: message.into(),
        }
    }

    pub(crate) fn error(what: &str, error: &str) -> Self {
        Self {
            kind: ToastKind::Error,
            message: format!("{what}: {}", git_error_line(error)),
        }
    }
}

/// A merge or rebase started from the popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GitOp {
    /// `git merge <branch>` while `into` is checked out.
    Merge { branch: String, into: String },
    /// `git rebase <onto>` while `branch` is checked out.
    Rebase { branch: String, onto: String },
}

/// The toast for a merge or rebase result.
pub(crate) fn outcome_toast(op: &GitOp, outcome: &Result<MergeOutcome>) -> OutcomeToast {
    let what = match op {
        GitOp::Merge { .. } => "Merge",
        GitOp::Rebase { .. } => "Rebase",
    };
    match outcome {
        Ok(MergeOutcome::Merged) => OutcomeToast::success(match op {
            GitOp::Merge { branch, into } => format!("Merged '{branch}' into '{into}'"),
            GitOp::Rebase { branch, onto } => format!("Rebased '{branch}' onto '{onto}'"),
        }),
        Ok(MergeOutcome::UpToDate) => OutcomeToast::success(match op {
            GitOp::Merge { into, .. } => format!("'{into}' is already up to date"),
            GitOp::Rebase { branch, onto } => {
                format!("'{branch}' is already up to date with '{onto}'")
            }
        }),
        Ok(MergeOutcome::Conflicts(paths)) => OutcomeToast {
            kind: ToastKind::Conflicts,
            message: format!(
                "{what} has conflicts: {} {}",
                paths.len(),
                if paths.len() == 1 { "file" } else { "files" }
            ),
        },
        Ok(MergeOutcome::Failed(message)) => {
            OutcomeToast::error(&format!("{what} failed"), message)
        }
        Err(error) => OutcomeToast::error(&format!("{what} failed"), &format!("{error:#}")),
    }
}

/// Zed's `GitBinaryCommandError` puts this header line before git's own output.
const ZED_GIT_ERROR_HEADER: &str = "Git command failed:";

/// How many indented detail lines (usually file names) follow the reason in a toast.
const DETAIL_LIMIT: usize = 5;

/// The reason git gave, in one line: the first `error:` / `fatal:` line (else the first
/// non-empty line other than Zed's error header), followed by the indented lines git lists
/// under it (such as the files a checkout would overwrite).
pub(crate) fn git_error_line(text: &str) -> String {
    let lines = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let is_header = |line: &str| line.trim() == ZED_GIT_ERROR_HEADER;
    let reason = lines
        .iter()
        .position(|line| {
            let line = line.trim_start();
            line.starts_with("error:") || line.starts_with("fatal:")
        })
        .or_else(|| lines.iter().position(|line| !is_header(line)));
    let Some(reason) = reason else {
        return "unknown error".to_string();
    };
    let details = lines[reason + 1..]
        .iter()
        .take_while(|line| line.starts_with([' ', '\t']))
        .take(DETAIL_LIMIT)
        .map(|line| line.trim())
        .collect::<Vec<_>>();
    let mut message = lines[reason].trim().to_string();
    if !details.is_empty() {
        message.push(' ');
        message.push_str(&details.join(", "));
    }
    message
}

fn toast_id(kind: ToastKind) -> NotificationId {
    NotificationId::named(
        match kind {
            ToastKind::Success => "brewery-git-success",
            ToastKind::Conflicts => "brewery-git-conflicts",
            ToastKind::Error => "brewery-git-error",
        }
        .into(),
    )
}

#[cfg(test)]
thread_local! {
    pub(crate) static SHOWN_TOASTS: std::cell::RefCell<Vec<OutcomeToast>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) fn show_toast(workspace: &WeakEntity<Workspace>, toast: OutcomeToast, cx: &mut App) {
    let Some(workspace) = workspace.upgrade() else {
        #[cfg(test)]
        SHOWN_TOASTS.with(|shown| shown.borrow_mut().push(toast.clone()));
        log::info!("{}", toast.message);
        return;
    };
    workspace.update(cx, |workspace, cx| {
        show_workspace_toast(workspace, toast, cx)
    });
}

/// `show_toast` from inside a workspace update.
pub(crate) fn show_workspace_toast(
    workspace: &mut Workspace,
    toast: OutcomeToast,
    cx: &mut Context<Workspace>,
) {
    #[cfg(test)]
    SHOWN_TOASTS.with(|shown| shown.borrow_mut().push(toast.clone()));
    let weak = cx.weak_entity();
    let mut notification = Toast::new(toast_id(toast.kind), toast.message);
    match toast.kind {
        ToastKind::Success => notification = notification.autohide(),
        ToastKind::Conflicts => {
            notification = notification.on_click("Resolve…", move |window, cx| {
                weak.update(cx, |workspace, cx| {
                    if let Some(repo) = workspace.project().read(cx).active_repository(cx) {
                        ConflictsDialog::open(workspace, repo, window, cx);
                    }
                })
                .ok();
            })
        }
        ToastKind::Error => {}
    }
    workspace.show_toast(notification, cx);
}

/// Git branch names cannot contain spaces; like Zed's picker, typed spaces become dashes.
fn normalize_branch_name(name: &str) -> String {
    name.trim().replace(' ', "-")
}

// ---------------------------------------------------------------------------------------------
// Picker delegate

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    List,
    NewBranch { from: Option<String> },
    Rename { branch: String },
    CheckoutRevision,
}

/// One entry of the per-branch submenu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BranchAction {
    Checkout,
    NewBranchFrom,
    CheckoutAndRebaseOntoCurrent,
    RebaseCurrentOnto,
    MergeIntoCurrent,
    CompareWithCurrent,
    Rename,
    Delete,
}

/// The submenu entries for `row`: (action, label, enabled).
pub(crate) fn submenu_entries(
    row: &BranchRow,
    current: Option<&str>,
) -> Vec<(BranchAction, String, bool)> {
    let name = row.branch.name();
    let is_local = !row.branch.is_remote();
    let mut entries = Vec::new();
    if !row.is_current {
        entries.push((BranchAction::Checkout, "Checkout".to_string(), true));
    }
    entries.push((
        BranchAction::NewBranchFrom,
        format!("New Branch from '{name}'…"),
        true,
    ));
    if let Some(current) = current.filter(|_| !row.is_current) {
        entries.push((
            BranchAction::CheckoutAndRebaseOntoCurrent,
            format!("Checkout and Rebase onto '{current}'"),
            true,
        ));
        entries.push((
            BranchAction::RebaseCurrentOnto,
            format!("Rebase '{current}' onto '{name}'"),
            true,
        ));
        entries.push((
            BranchAction::MergeIntoCurrent,
            format!("Merge '{name}' into '{current}'"),
            true,
        ));
        entries.push((
            BranchAction::CompareWithCurrent,
            format!("Compare with '{current}'"),
            true,
        ));
    }
    if is_local {
        entries.push((BranchAction::Rename, "Rename…".to_string(), true));
        if !row.is_current {
            entries.push((BranchAction::Delete, "Delete".to_string(), true));
        }
    } else {
        // `git push <remote> --delete <branch>`.
        entries.push((BranchAction::Delete, "Delete".to_string(), true));
    }
    entries
}

/// How checking out a branch row goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CheckoutPlan {
    /// `Repository::change_branch` with this name: a local branch, or a remote branch that
    /// has no local branch yet (Zed creates one tracking it).
    Checkout(String),
    /// A remote branch whose local branch exists and tracks something else (or nothing):
    /// Zed's checkout would silently move the local branch's upstream, so ask first.
    AskUpstream {
        local: String,
        remote: String,
        /// The local branch's upstream now, `None` when it has none.
        upstream: Option<String>,
    },
}

/// Plans the checkout of `row` given the repository's `branches`. A remote branch maps to the
/// local branch named after it without the remote (as Zed's `change_branch` does).
pub(crate) fn checkout_plan(row: &BranchRow, branches: &[Branch]) -> CheckoutPlan {
    let name = row.branch.name().to_string();
    if !row.branch.is_remote() {
        return CheckoutPlan::Checkout(name);
    }
    let Some((_, local)) = name.split_once('/') else {
        return CheckoutPlan::Checkout(name);
    };
    let Some(existing) = branches
        .iter()
        .find(|branch| !branch.is_remote() && branch.name() == local)
    else {
        return CheckoutPlan::Checkout(name);
    };
    let upstream = existing
        .upstream
        .as_ref()
        .map(|upstream| upstream.ref_name.to_string());
    if upstream.as_deref() == Some(row.branch.ref_name.as_ref()) {
        return CheckoutPlan::Checkout(local.to_string());
    }
    CheckoutPlan::AskUpstream {
        local: local.to_string(),
        remote: name,
        upstream: upstream.map(|upstream| {
            upstream
                .strip_prefix("refs/remotes/")
                .or_else(|| upstream.strip_prefix("refs/heads/"))
                .unwrap_or(&upstream)
                .to_string()
        }),
    }
}

/// Resolves a [`CheckoutPlan`] to the name `change_branch` gets, asking about the upstream
/// when needed; `Err(CANCELLED)` when the user cancels.
async fn checkout_target(plan: CheckoutPlan, cx: &mut AsyncWindowContext) -> Result<String> {
    let (local, remote, upstream) = match plan {
        CheckoutPlan::Checkout(name) => return Ok(name),
        CheckoutPlan::AskUpstream {
            local,
            remote,
            upstream,
        } => (local, remote, upstream),
    };
    let detail = match &upstream {
        Some(upstream) => format!("'{local}' tracks '{upstream}'."),
        None => format!("'{local}' has no upstream branch."),
    };
    let track = format!("Track '{remote}'");
    let keep = "Keep Its Upstream";
    let answer = cx.update(|window, cx| {
        window.prompt(
            PromptLevel::Info,
            &format!(
                "Local branch '{local}' already exists. Check it out and make it track '{remote}'?"
            ),
            Some(&detail),
            &[track.as_str(), keep, "Cancel"],
            cx,
        )
    })?;
    match answer.await {
        Ok(0) => Ok(remote),
        Ok(1) => Ok(local),
        _ => Err(anyhow!(CANCELLED)),
    }
}

/// Bounds for the smart checkout's stash commands.
const STASH_TIMEOUT: Duration = Duration::from_secs(120);

/// Checks out `target`. When git refuses because of local changes to tracked files, offers
/// IntelliJ's Smart Checkout: stash them, check out, apply the stash again. A stash that does
/// not apply cleanly stays in the stash list and its conflicts open the Conflicts dialog.
async fn checkout_with_smart_fallback(
    repo: &Entity<Repository>,
    target: String,
    cx: &mut AsyncWindowContext,
) -> Result<OutcomeToast> {
    let change_branch = async |cx: &mut AsyncWindowContext| {
        repo.update(cx, |repo, _| repo.change_branch(target.clone()))
            .await
            .map_err(|_| anyhow!("checkout was cancelled"))
            .and_then(|result| result)
    };
    let Err(error) = change_branch(cx).await else {
        return Ok(OutcomeToast::success(format!("Checked out '{target}'")));
    };
    let failed =
        |error: &anyhow::Error| OutcomeToast::error("Checkout failed", &format!("{error:#}"));
    // Checked structurally, not by git's (localised) message: are tracked files changed?
    let run = |args: &[&str], cx: &mut AsyncWindowContext| {
        let args = args.iter().map(|arg| arg.to_string()).collect();
        cx.update(|_, cx| GitCli::run(repo, args, Some(STASH_TIMEOUT), cx))
    };
    let status = run(&["status", "--porcelain", "--untracked-files=no"], cx)?.await?;
    if !status.success() || status.stdout.trim().is_empty() {
        return Ok(failed(&error));
    }
    let answer = cx.update(|window, cx| {
        window.prompt(
            PromptLevel::Warning,
            &format!("Your local changes would be overwritten by checking out '{target}'."),
            Some(&format!(
                "{}\n\nSmart Checkout stashes the local changes, checks out '{target}' and \
                 applies them again.",
                git_error_line(&format!("{error:#}"))
            )),
            &["Smart Checkout", "Cancel"],
            cx,
        )
    })?;
    if answer.await != Ok(0) {
        return Err(anyhow!(CANCELLED));
    }
    let message = format!("Brewery: uncommitted changes before checking out {target}");
    let stash = run(&["stash", "push", "--message", &message], cx)?.await?;
    if !stash.success() {
        return Ok(OutcomeToast::error(
            "Smart Checkout failed",
            &stash.message(),
        ));
    }
    if let Err(error) = change_branch(cx).await {
        // Put the changes back where they were.
        let restore = run(&["stash", "pop", "--index"], cx)?.await?;
        let mut toast = failed(&error);
        if !restore.success() {
            toast
                .message
                .push_str(" (your changes are in the stash list)");
        }
        return Ok(toast);
    }
    let pop = run(&["stash", "pop"], cx)?.await?;
    if pop.success() {
        return Ok(OutcomeToast::success(format!(
            "Checked out '{target}' and restored the local changes"
        )));
    }
    let conflicts = cx.update(|_, cx| GitCli::unmerged_paths(repo, cx))?.await?;
    if conflicts.is_empty() {
        return Ok(OutcomeToast::error(
            &format!(
                "Checked out '{target}', but the local changes did not apply (they are in the stash list)"
            ),
            &pop.message(),
        ));
    }
    Ok(OutcomeToast {
        kind: ToastKind::Conflicts,
        message: format!(
            "Checked out '{target}'; restoring the local changes has conflicts: {} {} (the \
             changes stay in the stash list)",
            conflicts.len(),
            if conflicts.len() == 1 {
                "file"
            } else {
                "files"
            }
        ),
    })
}

struct Submenu {
    menu: Entity<ContextMenu>,
    position: Point<Pixels>,
    _subscription: Subscription,
}

pub struct BranchesDelegate {
    workspace: WeakEntity<Workspace>,
    repo: Option<Entity<Repository>>,
    branches: Vec<Branch>,
    current: Option<String>,
    pub(crate) recent: Vec<String>,
    pub(crate) rows: Vec<Row>,
    selected_index: usize,
    mode: Mode,
    submenu: Option<Submenu>,
    /// Painted bounds of the branch rows by index; the submenu opens beside its row.
    row_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
}

impl BranchesDelegate {
    fn new(workspace: WeakEntity<Workspace>, repo: Option<Entity<Repository>>, cx: &App) -> Self {
        let mut this = Self {
            workspace,
            repo,
            branches: Vec::new(),
            current: None,
            recent: Vec::new(),
            rows: Vec::new(),
            selected_index: 0,
            mode: Mode::List,
            submenu: None,
            row_bounds: Rc::default(),
        };
        this.reload_branches(cx);
        this
    }

    fn reload_branches(&mut self, cx: &App) {
        let Some(repo) = self.repo.as_ref() else {
            return;
        };
        let repo = repo.read(cx);
        self.branches = repo.branch_list.to_vec();
        self.current = repo.branch.as_ref().map(|b| b.name().to_string());
    }

    fn selected_branch(&self) -> Option<&BranchRow> {
        self.rows.get(self.selected_index).and_then(Row::as_branch)
    }

    fn enter_mode(
        &mut self,
        mode: Mode,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        self.mode = mode;
        self.submenu = None;
        cx.defer_in(window, move |picker, window, cx| {
            picker.set_query(&query, window, cx);
            picker.select_query(window, cx);
            picker.refresh_placeholder(window, cx);
            picker.refresh(window, cx);
            picker.focus(window, cx);
        });
    }

    fn leave_mode(&mut self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.enter_mode(Mode::List, String::new(), window, cx);
    }

    fn open_submenu(
        &mut self,
        ix: usize,
        position: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        let Some(row) = self.rows.get(ix).and_then(Row::as_branch).cloned() else {
            return;
        };
        self.selected_index = ix;
        let position = position
            .or_else(|| {
                self.row_bounds
                    .borrow()
                    .get(&ix)
                    .map(|bounds| bounds.top_right())
            })
            .unwrap_or_default();
        let entries = submenu_entries(&row, self.current.as_deref());
        let picker = cx.weak_entity();
        let menu = ContextMenu::build(window, cx, move |mut menu, _, _| {
            for (action, label, enabled) in entries {
                if action == BranchAction::Rename || action == BranchAction::CompareWithCurrent {
                    menu = menu.separator();
                }
                let picker = picker.clone();
                let row = row.clone();
                let mut entry =
                    ContextMenuEntry::new(label)
                        .disabled(!enabled)
                        .handler(move |window, cx| {
                            picker
                                .update(cx, |picker, cx| {
                                    picker.delegate.run_branch_action(
                                        action.clone(),
                                        &row,
                                        window,
                                        cx,
                                    )
                                })
                                .ok();
                        });
                if !enabled {
                    entry = entry.documentation_aside(DocumentationSide::Right, |_| {
                        Label::new("Coming in a later update").into_any_element()
                    });
                }
                menu = menu.item(entry);
            }
            menu
        });
        // The menu is drawn deferred, so its focus handle joins the popup's dispatch tree only
        // after the next frame; focusing it earlier would blur (and close) it at once.
        let focus_handle = menu.focus_handle(cx);
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |window, cx| window.focus(&focus_handle, cx));
        });
        window.refresh();
        let subscription =
            cx.subscribe_in(&menu, window, |picker, _, _: &DismissEvent, window, cx| {
                picker.delegate.submenu = None;
                picker.focus(window, cx);
                cx.notify();
            });
        self.submenu = Some(Submenu {
            menu,
            position,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn run_top_action(
        &mut self,
        action: TopAction,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        match action {
            TopAction::NewBranch => {
                self.enter_mode(Mode::NewBranch { from: None }, String::new(), window, cx)
            }
            TopAction::CheckoutRevision => {
                self.enter_mode(Mode::CheckoutRevision, String::new(), window, cx)
            }
            TopAction::UpdateProject => {
                // Zed's pull keeps askpass and credentials; the popup goes away afterwards.
                window.dispatch_action(git::Pull.boxed_clone(), cx);
                cx.emit(DismissEvent);
            }
            TopAction::Push => {
                window.dispatch_action(git::Push.boxed_clone(), cx);
                cx.emit(DismissEvent);
            }
            TopAction::Stashes => {
                // Zed's stash picker: apply, pop, drop and view each stash.
                window.dispatch_action(zed_actions::git::ViewStash.boxed_clone(), cx);
                cx.emit(DismissEvent);
            }
        }
    }

    pub(crate) fn run_branch_action(
        &mut self,
        action: BranchAction,
        row: &BranchRow,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        let name = row.branch.name().to_string();
        let current = self.current.clone();
        match action {
            BranchAction::Checkout => self.checkout(row, window, cx),
            BranchAction::NewBranchFrom => self.enter_mode(
                Mode::NewBranch {
                    from: Some(name.clone()),
                },
                String::new(),
                window,
                cx,
            ),
            BranchAction::Rename => self.enter_mode(
                Mode::Rename {
                    branch: name.clone(),
                },
                name,
                window,
                cx,
            ),
            BranchAction::CompareWithCurrent => {
                // The project against the branch, as a merge-base diff, by its full ref name
                // (a tag of the same name would win otherwise).
                let Some(repo) = self.repo.clone() else {
                    return;
                };
                let full_ref = row.branch.ref_name.clone();
                self.workspace
                    .update(cx, |workspace, cx| {
                        compare::compare_project(workspace, repo, full_ref, window, cx)
                    })
                    .ok();
                cx.emit(DismissEvent);
            }
            BranchAction::MergeIntoCurrent => {
                let Some(current) = current else { return };
                self.spawn_operation(
                    window,
                    cx,
                    async move |repo, cx: &mut AsyncWindowContext| {
                        if let Some(toast) = refuse_while_in_progress("Merge", &repo, cx).await? {
                            return Ok(toast);
                        }
                        let outcome = cx.update(|_, cx| GitCli::merge(&repo, &name, cx))?.await;
                        Ok(outcome_toast(
                            &GitOp::Merge {
                                branch: name,
                                into: current,
                            },
                            &outcome,
                        ))
                    },
                );
            }
            BranchAction::RebaseCurrentOnto => {
                let Some(current) = current else { return };
                self.spawn_operation(
                    window,
                    cx,
                    async move |repo, cx: &mut AsyncWindowContext| {
                        if let Some(toast) = refuse_while_in_progress("Rebase", &repo, cx).await? {
                            return Ok(toast);
                        }
                        let outcome = cx.update(|_, cx| GitCli::rebase(&repo, &name, cx))?.await;
                        Ok(outcome_toast(
                            &GitOp::Rebase {
                                branch: current,
                                onto: name,
                            },
                            &outcome,
                        ))
                    },
                );
            }
            BranchAction::CheckoutAndRebaseOntoCurrent => {
                let Some(current) = current else { return };
                let is_remote = row.branch.is_remote();
                let plan = checkout_plan(row, &self.branches);
                self.spawn_operation(
                    window,
                    cx,
                    async move |repo, cx: &mut AsyncWindowContext| {
                        if let Some(toast) = refuse_while_in_progress("Rebase", &repo, cx).await? {
                            return Ok(toast);
                        }
                        let target = checkout_target(plan, cx).await?;
                        if let Err(error) = repo
                            .update(cx, |repo, _| repo.change_branch(target.clone()))
                            .await
                            .map_err(|_| anyhow!("checkout was cancelled"))
                            .and_then(|result| result)
                        {
                            return Ok(OutcomeToast::error(
                                "Checkout failed",
                                &format!("{error:#}"),
                            ));
                        }
                        // Checking out a remote branch creates the local tracking branch.
                        let local = if is_remote {
                            target
                                .split_once('/')
                                .filter(|_| target == name)
                                .map(|(_, local)| local.to_string())
                                .unwrap_or(target.clone())
                        } else {
                            target.clone()
                        };
                        let outcome = cx
                            .update(|_, cx| GitCli::rebase(&repo, &current, cx))?
                            .await;
                        Ok(outcome_toast(
                            &GitOp::Rebase {
                                branch: local,
                                onto: current,
                            },
                            &outcome,
                        ))
                    },
                );
            }
            BranchAction::Delete if row.branch.is_remote() => self.delete_remote(row, window, cx),
            BranchAction::Delete => self.delete(name, window, cx),
        }
    }

    /// Runs `operation` after the popup closes and shows the toast it returns (an `Err` becomes
    /// an error toast). Conflicts open the Conflicts dialog instead of a toast.
    fn spawn_operation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
        operation: impl AsyncFnOnce(Entity<Repository>, &mut AsyncWindowContext) -> Result<OutcomeToast>
        + 'static,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let workspace = self.workspace.clone();
        let guard = conflicts_dialog::OperationGuard::begin(repo.entity_id(), cx);
        cx.spawn_in(window, async move |_, cx| {
            let result = operation(repo.clone(), cx).await;
            drop(guard);
            cx.update(|window, cx| {
                let toast = match result {
                    Ok(toast) => toast,
                    Err(error) if error.to_string() == CANCELLED => return,
                    Err(error) => OutcomeToast::error("git failed", &format!("{error:#}")),
                };
                if toast.kind == ToastKind::Conflicts
                    && let Some(workspace) = workspace.upgrade()
                {
                    workspace.update(cx, |workspace, cx| {
                        ConflictsDialog::open(workspace, repo, window, cx);
                    });
                    return;
                }
                show_toast(&workspace, toast, cx);
            })
            .ok();
        })
        .detach();
        cx.emit(DismissEvent);
    }

    fn checkout(&mut self, row: &BranchRow, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        if row.is_current {
            cx.emit(DismissEvent);
            return;
        }
        let plan = checkout_plan(row, &self.branches);
        self.spawn_operation(
            window,
            cx,
            async move |repo, cx: &mut AsyncWindowContext| {
                let target = checkout_target(plan, cx).await?;
                checkout_with_smart_fallback(&repo, target, cx).await
            },
        );
    }

    /// Deletes a remote branch with `git push <remote> :<branch>` through Zed's push, which
    /// asks for credentials in Zed's askpass modal.
    fn delete_remote(
        &mut self,
        row: &BranchRow,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        let name = row.branch.name().to_string();
        let Some(remote) = row.branch.remote_name().map(str::to_string) else {
            return;
        };
        let Some(branch) = name.strip_prefix(&format!("{remote}/")).map(str::to_string) else {
            return;
        };
        let workspace = self.workspace.clone();
        self.spawn_operation(
            window,
            cx,
            async move |repo, cx: &mut AsyncWindowContext| {
                let answer = cx.update(|window, cx| {
                    window.prompt(
                        PromptLevel::Warning,
                        &format!("Delete remote branch '{name}'?"),
                        Some(&format!("The branch '{branch}' is deleted on '{remote}'.")),
                        &["Delete", "Cancel"],
                        cx,
                    )
                })?;
                if answer.await != Ok(0) {
                    return Err(anyhow!(CANCELLED));
                }
                let askpass = cx.update(|window, cx| {
                    askpass_delegate(workspace, format!("git push {remote} --delete"), window, cx)
                })?;
                let pushed = repo
                    .update(cx, |repo, cx| {
                        repo.push(
                            SharedString::default(),
                            branch.clone().into(),
                            remote.clone().into(),
                            None,
                            askpass,
                            cx,
                        )
                    })
                    .await
                    .map_err(|_| anyhow!("delete was cancelled"))
                    .and_then(|result| result);
                Ok(match pushed {
                    Ok(_) => OutcomeToast::success(format!("Deleted remote branch '{name}'")),
                    Err(error) => OutcomeToast::error("Delete failed", &format!("{error:#}")),
                })
            },
        );
    }

    fn delete(&mut self, name: String, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.spawn_operation(
            window,
            cx,
            async move |repo, cx: &mut AsyncWindowContext| {
                let answer = cx.update(|window, cx| {
                    window.prompt(
                        PromptLevel::Warning,
                        &format!("Delete branch '{name}'?"),
                        None,
                        &["Delete", "Cancel"],
                        cx,
                    )
                })?;
                if answer.await != Ok(0) {
                    return Err(anyhow!(CANCELLED));
                }
                let deleted = delete_branch(&repo, &name, false, cx).await;
                let deleted = match deleted {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        if !is_unmerged(&repo, &name, cx).await {
                            Err(error)
                        } else {
                            let answer = cx.update(|window, cx| {
                                window.prompt(
                                    PromptLevel::Warning,
                                    &format!(
                                        "Branch '{name}' is not fully merged. Force delete it?"
                                    ),
                                    None,
                                    &["Force Delete", "Cancel"],
                                    cx,
                                )
                            })?;
                            if answer.await != Ok(0) {
                                return Err(anyhow!(CANCELLED));
                            }
                            delete_branch(&repo, &name, true, cx).await
                        }
                    }
                };
                Ok(match deleted {
                    Ok(()) => OutcomeToast::success(format!("Deleted branch '{name}'")),
                    Err(error) => OutcomeToast::error("Delete failed", &format!("{error:#}")),
                })
            },
        );
    }

    fn confirm_input(
        &mut self,
        input: InputRow,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        if !input.is_ready() {
            return;
        }
        match input {
            InputRow::NewBranch { name, from } => self.spawn_operation(
                window,
                cx,
                async move |repo, cx: &mut AsyncWindowContext| {
                    check_revision(&name, "branch name")?;
                    let result = repo
                        .update(cx, |repo, _| repo.create_branch(name.clone(), from))
                        .await
                        .map_err(|_| anyhow!("branch creation was cancelled"))
                        .and_then(|result| result);
                    Ok(match result {
                        Ok(()) => OutcomeToast::success(format!("Created branch '{name}'")),
                        Err(error) => {
                            OutcomeToast::error("Create branch failed", &format!("{error:#}"))
                        }
                    })
                },
            ),
            InputRow::Rename { from, to } => self.spawn_operation(
                window,
                cx,
                async move |repo, cx: &mut AsyncWindowContext| {
                    check_revision(&to, "branch name")?;
                    let result = repo
                        .update(cx, |repo, _| repo.rename_branch(from.clone(), to.clone()))
                        .await
                        .map_err(|_| anyhow!("rename was cancelled"))
                        .and_then(|result| result);
                    Ok(match result {
                        Ok(()) => OutcomeToast::success(format!("Renamed '{from}' to '{to}'")),
                        Err(error) => OutcomeToast::error("Rename failed", &format!("{error:#}")),
                    })
                },
            ),
            InputRow::CheckoutRevision { rev } => self.spawn_operation(
                window,
                cx,
                async move |repo, cx: &mut AsyncWindowContext| {
                    check_revision(&rev, "revision")?;
                    let verify = cx
                        .update(|_, cx| {
                            GitCli::run(
                                &repo,
                                vec![
                                    "rev-parse".into(),
                                    "--verify".into(),
                                    "--quiet".into(),
                                    format!("{rev}^{{commit}}"),
                                ],
                                Some(REV_PARSE_TIMEOUT),
                                cx,
                            )
                        })?
                        .await?;
                    if !verify.success() {
                        return Ok(OutcomeToast::error(
                            "Checkout failed",
                            &format!("'{rev}' is not a tag, branch or commit"),
                        ));
                    }
                    let output = cx
                        .update(|_, cx| {
                            GitCli::run(
                                &repo,
                                vec!["switch".into(), "--detach".into(), rev.clone()],
                                Some(SWITCH_TIMEOUT),
                                cx,
                            )
                        })?
                        .await?;
                    Ok(if output.success() {
                        OutcomeToast::success(format!("Checked out '{rev}' (detached HEAD)"))
                    } else {
                        OutcomeToast::error("Checkout failed", &output.message())
                    })
                },
            ),
        }
    }
}

/// An `Err` with this message from an operation means the user cancelled: no toast.
const CANCELLED: &str = "cancelled by the user";

/// Asks for credentials in Zed's askpass modal, as the git panel's push does.
fn askpass_delegate(
    workspace: WeakEntity<Workspace>,
    operation: String,
    window: &mut Window,
    cx: &mut App,
) -> AskPassDelegate {
    let operation = SharedString::from(operation);
    let window = window.window_handle();
    AskPassDelegate::new_with_cancellation(
        &mut cx.to_async(),
        move |prompt, tx, cancellation, cx| {
            window
                .update(cx, |_, window, cx| {
                    workspace
                        .update(cx, |workspace, cx| {
                            workspace.toggle_modal(window, cx, |window, cx| {
                                AskPassModal::new(
                                    operation.clone(),
                                    prompt.into(),
                                    tx,
                                    cancellation,
                                    window,
                                    cx,
                                )
                            });
                        })
                        .ok();
                })
                .ok();
        },
    )
}

/// A merge or rebase refuses to start while git has one in progress, and the unmerged paths
/// left by it would make the refusal look like new conflicts: say what is in progress instead.
async fn refuse_while_in_progress(
    what: &str,
    repo: &Entity<Repository>,
    cx: &mut AsyncWindowContext,
) -> Result<Option<OutcomeToast>> {
    let in_progress = cx.update(|_, cx| GitCli::in_progress(repo, cx))?.await?;
    Ok(in_progress.map(|operation| {
        let operation = match operation {
            Operation::Merge => "a merge",
            Operation::Rebase => "a rebase",
            Operation::CherryPick => "a cherry-pick",
        };
        OutcomeToast::error(
            &format!("{what} failed"),
            &format!("{operation} is in progress; finish or abort it first"),
        )
    }))
}

/// Bounds for git commands the popup runs itself; hooks run in trusted repositories and must
/// not block the repository's job queue forever.
const REV_PARSE_TIMEOUT: Duration = Duration::from_secs(30);
const SWITCH_TIMEOUT: Duration = Duration::from_secs(600);

async fn delete_branch(
    repo: &Entity<Repository>,
    name: &str,
    force: bool,
    cx: &mut AsyncWindowContext,
) -> Result<()> {
    repo.update(cx, |repo, _| {
        repo.delete_branch(false, name.to_string(), force)
    })
    .await
    .map_err(|_| anyhow!("delete was cancelled"))
    .and_then(|result| result)
}

/// Whether `git branch -d` refused `name` because it is not merged into its upstream (or HEAD
/// when it has none), checked structurally rather than by git's localised message.
async fn is_unmerged(repo: &Entity<Repository>, name: &str, cx: &mut AsyncWindowContext) -> bool {
    let local_ref = format!("refs/heads/{name}");
    let run = |args: Vec<String>, cx: &mut AsyncWindowContext| {
        cx.update(|_, cx| GitCli::run(repo, args, Some(REV_PARSE_TIMEOUT), cx))
    };
    let Ok(upstream) = run(
        vec![
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            "--symbolic-full-name".into(),
            format!("{local_ref}@{{upstream}}"),
        ],
        cx,
    ) else {
        return false;
    };
    let base = match upstream.await {
        Ok(output) if output.success() && !output.stdout.trim().is_empty() => {
            output.stdout.trim().to_string()
        }
        _ => "HEAD".to_string(),
    };
    let Ok(ancestor) = run(
        vec!["merge-base".into(), "--is-ancestor".into(), local_ref, base],
        cx,
    ) else {
        return false;
    };
    matches!(ancestor.await, Ok(output) if output.status == 1)
}

impl PickerDelegate for BranchesDelegate {
    type ListItem = AnyElement;

    fn name() -> &'static str {
        "brewery branches popup"
    }

    fn match_count(&self) -> usize {
        self.rows.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected_index = ix;
    }

    fn can_select(&self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) -> bool {
        self.rows.get(ix).is_some_and(Row::is_selectable)
    }

    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        match &self.mode {
            Mode::List => "Search branches and actions…".into(),
            Mode::NewBranch { from: None } => "New branch name".into(),
            Mode::NewBranch { from: Some(from) } => format!("New branch from '{from}'").into(),
            Mode::Rename { branch } => format!("New name for '{branch}'").into(),
            Mode::CheckoutRevision => "Tag, branch or commit".into(),
        }
    }

    fn no_matches_text(&self, _: &mut Window, _: &mut App) -> Option<SharedString> {
        Some(if self.repo.is_none() {
            "No git repository".into()
        } else {
            "No matching branches".into()
        })
    }

    fn update_matches(
        &mut self,
        query: String,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let input = match &self.mode {
            Mode::List => None,
            Mode::NewBranch { from } => Some(InputRow::NewBranch {
                name: normalize_branch_name(&query),
                from: from.clone(),
            }),
            Mode::Rename { branch } => Some(InputRow::Rename {
                from: branch.clone(),
                to: normalize_branch_name(&query),
            }),
            Mode::CheckoutRevision => Some(InputRow::CheckoutRevision {
                rev: query.trim().to_string(),
            }),
        };
        self.row_bounds.borrow_mut().clear();
        match input {
            Some(input) => {
                self.rows = vec![Row::Input(input)];
                self.selected_index = 0;
            }
            None => {
                let grouped = group_rows(
                    &self.branches,
                    self.current.as_deref(),
                    &self.recent,
                    &query,
                );
                self.rows = grouped.rows;
                self.selected_index = grouped.selected.unwrap_or(0);
                if self.repo.is_none() {
                    // Without a repository only Zed's pull and push (which explain themselves)
                    // remain; the other actions would do nothing.
                    self.rows.retain(|row| {
                        matches!(
                            row,
                            Row::Action {
                                action: TopAction::UpdateProject | TopAction::Push,
                                ..
                            }
                        )
                    });
                    self.selected_index = 0;
                }
            }
        }
        Task::ready(())
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(row) = self.rows.get(self.selected_index).cloned() else {
            return;
        };
        match row {
            Row::Action { action, .. } => self.run_top_action(action, window, cx),
            Row::Branch(row) => self.checkout(&row, window, cx),
            Row::Input(input) => self.confirm_input(input, window, cx),
            Row::Header(_) => {}
        }
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let row = self.rows.get(ix)?;
        let element = match row {
            Row::Header(header) => {
                let is_remote_name = matches!(header, Header::RemoteName(_));
                div()
                    .when(is_remote_name, |this| this.pl_2())
                    .child(ListSubHeader::new(header.title()).inset(true))
                    .into_any_element()
            }
            Row::Action { action, positions } => ListItem::new(("action", ix))
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .start_slot(
                    Icon::new(action.icon())
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(HighlightedLabel::new(action.label(), positions.clone()))
                .into_any_element(),
            Row::Input(input) => ListItem::new(("input", ix))
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected && input.is_ready())
                .start_slot(
                    Icon::new(IconName::GitBranchPlus)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(Label::new(input.label()).color(if input.is_ready() {
                    Color::Default
                } else {
                    Color::Muted
                }))
                .into_any_element(),
            Row::Branch(branch_row) => {
                let icon = if branch_row.is_current {
                    Icon::new(IconName::Check).color(Color::Accent)
                } else if branch_row.branch.is_remote() {
                    Icon::new(IconName::Server).color(Color::Muted)
                } else {
                    Icon::new(IconName::GitBranch).color(Color::Muted)
                };
                let selector = format!("{:?}-{}", branch_row.group, branch_row.branch.name());
                let actions_selector = format!("BRANCH_ACTIONS-{selector}");
                let row_selector = format!("BRANCH_ROW-{selector}");
                let item = ListItem::new(("branch", ix))
                    .inset(true)
                    .spacing(ListItemSpacing::Sparse)
                    .toggle_state(selected)
                    .when(branch_row.group == Group::Remote, |this| {
                        this.indent_level(1)
                    })
                    .start_slot(icon.size(IconSize::Small))
                    .child(HighlightedLabel::new(
                        branch_row.branch.name().to_string(),
                        branch_row.positions.clone(),
                    ))
                    .end_slot(
                        div()
                            .debug_selector(move || actions_selector)
                            // A press on the arrow never reaches the picker row, whose click
                            // checks the branch out.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                IconButton::new(("branch-actions", ix), IconName::ChevronRight)
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted)
                                    .on_click(cx.listener(
                                        move |picker, _: &ClickEvent, window, cx| {
                                            cx.stop_propagation();
                                            // Beside the row like →, not under the pointer
                                            // where Checkout would take the next click.
                                            picker.delegate.open_submenu(ix, None, window, cx);
                                        },
                                    )),
                            ),
                    )
                    .on_secondary_mouse_down(cx.listener(
                        move |picker, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            picker
                                .delegate
                                .open_submenu(ix, Some(event.position), window, cx);
                        },
                    ));
                let bounds = self.row_bounds.clone();
                div()
                    .relative()
                    .debug_selector(move || row_selector)
                    // The picker confirms (checks out) on a right-button release too; here the
                    // right button opens the submenu instead.
                    .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .child(item)
                    .child(
                        canvas(
                            move |row_bounds, _, _| {
                                bounds.borrow_mut().insert(ix, row_bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .into_any_element()
            }
        };
        Some(element)
    }
}

// ---------------------------------------------------------------------------------------------
// The popup view

pub struct BranchesPopup {
    pub(crate) picker: Entity<Picker<BranchesDelegate>>,
    _subscriptions: Vec<Subscription>,
}

impl BranchesPopup {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        repo: Option<Entity<Repository>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = BranchesDelegate::new(workspace, repo.clone(), cx);
        let picker = cx.new(|cx| {
            Picker::list(delegate, window, cx)
                .initial_width(rems(34.))
                .show_scrollbar(true)
        });
        let mut subscriptions = vec![
            cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent)),
            cx.observe(&picker, |_, _, cx| cx.notify()),
        ];
        if let Some(repo) = repo {
            subscriptions.push(cx.subscribe_in(
                &repo,
                window,
                |this, _, event: &RepositoryEvent, window, cx| {
                    if matches!(
                        event,
                        RepositoryEvent::BranchListChanged | RepositoryEvent::HeadChanged
                    ) {
                        this.picker.update(cx, |picker, cx| {
                            picker.delegate.reload_branches(cx);
                            picker.refresh(window, cx);
                        });
                    }
                },
            ));
            let recent = GitCli::recent_branches(&repo, RECENT_LIMIT + 1, cx);
            cx.spawn_in(window, async move |this, cx| {
                let recent = match recent.await {
                    Ok(recent) => recent,
                    Err(error) => {
                        log::debug!("recent branches unavailable: {error:#}");
                        return;
                    }
                };
                this.update_in(cx, |this, window, cx| {
                    this.picker.update(cx, |picker, cx| {
                        picker.delegate.recent = recent;
                        if picker.delegate.mode == Mode::List {
                            picker.refresh(window, cx);
                        }
                    });
                })
                .ok();
            })
            .detach();
        }
        Self {
            picker,
            _subscriptions: subscriptions,
        }
    }

    fn on_move_right(
        &mut self,
        _: &editor::actions::MoveRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            let delegate = &mut picker.delegate;
            if delegate.mode == Mode::List && delegate.selected_branch().is_some() {
                cx.stop_propagation();
                let ix = delegate.selected_index;
                delegate.open_submenu(ix, None, window, cx);
            }
        });
    }

    fn on_cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.picker.update(cx, |picker, cx| {
            if picker.delegate.submenu.take().is_some() {
                // The submenu takes focus two frames after it opens; an Escape before that
                // closes only the submenu.
                cx.stop_propagation();
                picker.focus(window, cx);
                cx.notify();
            } else if picker.delegate.mode != Mode::List {
                cx.stop_propagation();
                picker.delegate.leave_mode(window, cx);
            }
        });
    }
}

impl ModalView for BranchesPopup {}
impl EventEmitter<DismissEvent> for BranchesPopup {}

impl Focusable for BranchesPopup {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for BranchesPopup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let submenu = self
            .picker
            .read(cx)
            .delegate
            .submenu
            .as_ref()
            .map(|submenu| (submenu.menu.clone(), submenu.position));
        v_flex()
            .key_context("BreweryBranchesPopup")
            .w(rems(34.))
            .capture_action(cx.listener(Self::on_move_right))
            .capture_action(cx.listener(Self::on_cancel))
            .child(self.picker.clone())
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                // Clicks in the submenu land outside the popup's bounds; the submenu closes
                // itself when it loses focus.
                if this.picker.read(cx).delegate.submenu.is_some() {
                    return;
                }
                this.picker.update(cx, |picker, cx| {
                    picker.cancel(&menu::Cancel, window, cx);
                });
            }))
            .children(submenu.map(|(menu, position)| {
                deferred(
                    anchored()
                        .position(position)
                        .anchor(Anchor::TopLeft)
                        .snap_to_window_with_margin(px(8.))
                        .child(menu),
                )
                .with_priority(2)
            }))
    }
}

// ---------------------------------------------------------------------------------------------
// Installation

/// Installs the popup as the title bar's branch picker and as the handler of `git::Branch`,
/// `git::Switch` and `git::CheckoutBranch`.
pub fn init(cx: &mut App) {
    git_ui_core::set_branch_picker_builder(
        |workspace, repository, window, cx| {
            let popup = cx.new(|cx| BranchesPopup::new(workspace, repository, window, cx));
            cx.new(|cx| git_ui_core::GitPickerPopover::new(popup, cx))
        },
        cx,
    );

    cx.observe_new(|workspace: &mut Workspace, _, _| {
        // Zed's git picker registers these actions first and the first bubble handler wins,
        // so the popup takes them in the capture phase. The commit modal keeps its own
        // branch selector.
        workspace.register_action_renderer(|div, _, _, cx| {
            div.capture_action(cx.listener(
                |workspace, _: &zed_actions::git::Branch, window, cx| {
                    toggle_popup(workspace, window, cx)
                },
            ))
            .capture_action(
                cx.listener(|workspace, _: &zed_actions::git::Switch, window, cx| {
                    toggle_popup(workspace, window, cx)
                }),
            )
            .capture_action(cx.listener(
                |workspace, _: &zed_actions::git::CheckoutBranch, window, cx| {
                    toggle_popup(workspace, window, cx)
                },
            ))
        });
    })
    .detach();
}

/// The key context of Zed's commit modal (`crates/git_ui/src/commit_modal.rs`, `key_context`).
/// Check it on Zed merges: if it is renamed, `git::Branch` there would replace the commit modal
/// with the popup (the `commit_modal_keeps_its_branch_selector` test pins this string).
pub(crate) const COMMIT_MODAL_CONTEXT: &str = "GitCommit";

fn toggle_popup(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if window
        .context_stack()
        .iter()
        .any(|context| context.contains(COMMIT_MODAL_CONTEXT))
    {
        return;
    }
    cx.stop_propagation();
    let handle = workspace.weak_handle();
    let repository = workspace.project().read(cx).active_repository(cx);
    workspace.toggle_modal(window, cx, |window, cx| {
        BranchesPopup::new(handle, repository, window, cx)
    });
}

#[cfg(test)]
#[path = "branches_popup_tests.rs"]
mod tests;

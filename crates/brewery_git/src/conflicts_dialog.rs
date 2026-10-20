//! IntelliJ's Conflicts dialog: the files a merge, rebase or cherry-pick stopped on, each with
//! Accept Yours, Accept Theirs and Merge… (the 3-pane [`MergeView`]), and Abort / Continue for
//! the operation.
//!
//! The list comes from `git diff --diff-filter=U` ([`GitCli::unmerged_paths`]) rather than
//! Zed's repository snapshot: the snapshot follows git commands only after the watcher rescans,
//! and the dialog must be right the moment Accept or Continue returns. It refreshes whenever
//! the snapshot's statuses change.
//!
//! Entry points:
//! - a merge or rebase started from the Branches popup that stops on conflicts opens it;
//! - `brewery::ResolveConflicts` opens it (or says "No conflicts");
//! - a conflicted file in the git panel opens the merge editor directly (the
//!   `git_ui_core::brewery_hooks` conflict opener; files the merge editor cannot take open this
//!   dialog instead);
//! - conflicts that appear without Brewery (Zed's own Pull, a terminal `git merge`) show one
//!   toast per conflict episode, with "Resolve…" opening the dialog. The dialog never opens by
//!   itself for those, because it would take the keyboard focus from whatever the user is
//!   typing. An episode starts when a repository goes from no conflicted files to some, and
//!   ends when it has none again.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use anyhow::{Result, anyhow};
use gpui::{
    App, Context, DismissEvent, Entity, EntityId, EventEmitter, FocusHandle, Focusable, Global,
    PromptLevel, Render, SharedString, Subscription, Task, WeakEntity, Window, actions, rems,
};
use project::git_store::{GitStoreEvent, Repository, RepositoryEvent};
use ui::{Tooltip, prelude::*};
use workspace::notifications::NotificationId;
use workspace::{ModalView, Toast, Workspace};

use crate::branches_popup::{OutcomeToast, show_toast};
use crate::cli::{ConflictSides, GitCli, MergeOutcome, Operation, RepoPath};
use crate::merge_view::{MergeApplied, MergeView};
use crate::revision_diff_view::MAX_FILE_SIZE;

actions!(
    brewery,
    [
        /// Opens the Conflicts dialog for the current repository: the conflicted files with
        /// Accept Yours, Accept Theirs and Merge…, and Abort / Continue.
        ResolveConflicts,
    ]
);

/// Which side "Accept" takes, named for the user (see [`ConflictSides`] for rebases).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Yours,
    Theirs,
}

pub struct ConflictsDialog {
    workspace: WeakEntity<Workspace>,
    repo: Entity<Repository>,
    focus_handle: FocusHandle,
    sides: Option<ConflictSides>,
    paths: Vec<RepoPath>,
    /// Whether each listed file can open in the merge editor; missing while being checked.
    mergeable: HashMap<RepoPath, bool>,
    loaded: bool,
    /// Why the last listing failed; shown instead of the list.
    error: Option<String>,
    /// A git command started from the dialog is running.
    busy: bool,
    selected: usize,
    refresh_task: Task<()>,
    mergeable_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl ConflictsDialog {
    /// Opens the dialog for `repo`, or refreshes the one already open for it. `None` when the
    /// modal that is open refused to close.
    pub fn open(
        workspace: &mut Workspace,
        repo: Entity<Repository>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Option<Entity<Self>> {
        if let Some(existing) = workspace.active_modal::<Self>(cx) {
            if existing.read(cx).repo == repo {
                existing.update(cx, |dialog, cx| dialog.refresh(cx));
                return Some(existing);
            }
            if !workspace.hide_modal(window, cx) {
                return None;
            }
        }
        let weak = workspace.weak_handle();
        workspace.toggle_modal(window, cx, |window, cx| Self::new(weak, repo, window, cx));
        workspace.active_modal::<Self>(cx)
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        repo: Entity<Repository>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        mark_episode(&repo, cx);
        let subscriptions = vec![cx.subscribe(&repo, |this, _, event: &RepositoryEvent, cx| {
            if matches!(event, RepositoryEvent::StatusesChanged) {
                this.refresh(cx);
            }
        })];
        let mut dialog = Self {
            workspace,
            repo,
            focus_handle: cx.focus_handle(),
            sides: None,
            paths: Vec::new(),
            mergeable: HashMap::new(),
            loaded: false,
            error: None,
            busy: false,
            selected: 0,
            refresh_task: Task::ready(()),
            mergeable_task: Task::ready(()),
            _subscriptions: subscriptions,
        };
        dialog.refresh(cx);
        dialog
    }

    pub fn repo(&self) -> &Entity<Repository> {
        &self.repo
    }

    /// The conflicted files, sorted.
    pub fn paths(&self) -> &[RepoPath] {
        &self.paths
    }

    /// The first listing has arrived.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn operation(&self) -> Option<Operation> {
        self.sides.as_ref().and_then(|sides| sides.operation)
    }

    /// "Merging feature into main", "Rebasing main onto feature".
    pub fn header(&self) -> SharedString {
        let Some(sides) = &self.sides else {
            return "Conflicts".into();
        };
        let theirs = if sides.theirs.is_empty() {
            "changes".to_string()
        } else {
            sides.theirs.clone()
        };
        match sides.operation {
            Some(Operation::Merge) => format!("Merging {theirs} into {}", sides.yours),
            Some(Operation::Rebase) => format!("Rebasing {} onto {theirs}", sides.yours),
            Some(Operation::CherryPick) => {
                format!("Cherry-picking {theirs} onto {}", sides.yours)
            }
            None => format!("Conflicts on {}", sides.yours),
        }
        .into()
    }

    /// Continue is offered: an operation is in progress and nothing is left to resolve.
    pub fn can_continue(&self) -> bool {
        self.loaded
            && self.error.is_none()
            && !self.busy
            && self.paths.is_empty()
            && self.operation().is_some()
    }

    /// Whether the file can open in the merge editor (`None` while that is being checked).
    pub fn can_merge(&self, path: &RepoPath) -> Option<bool> {
        self.mergeable.get(path).copied()
    }

    /// Re-reads the conflicted files and who is on which side.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let unmerged = GitCli::unmerged_paths(&self.repo, cx);
        let sides = GitCli::conflict_sides(&self.repo, cx);
        self.refresh_task = cx.spawn(async move |this, cx| {
            let (paths, sides) = (unmerged.await, sides.await);
            this.update(cx, |this, cx| {
                match (paths, sides) {
                    (Ok(paths), Ok(sides)) => {
                        this.paths = paths;
                        this.sides = Some(sides);
                        this.error = None;
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        log::warn!("listing conflicts failed: {error:#}");
                        this.error = Some(format!("{error:#}"));
                    }
                }
                this.loaded = true;
                this.selected = this.selected.min(this.paths.len().saturating_sub(1));
                this.check_mergeable(cx);
                cx.notify();
            })
            .ok();
        });
    }

    fn check_mergeable(&mut self, cx: &mut Context<Self>) {
        let unchecked = self
            .paths
            .iter()
            .filter(|path| !self.mergeable.contains_key(*path))
            .cloned()
            .collect::<Vec<_>>();
        if unchecked.is_empty() {
            return;
        }
        // One batched read for all files, not one merge-editor load per file.
        let check = GitCli::mergeable_conflicts(&self.repo, MAX_FILE_SIZE as u64, cx);
        self.mergeable_task = cx.spawn(async move |this, cx| {
            let mergeable = check
                .await
                .inspect_err(|error| log::warn!("checking mergeability failed: {error:#}"))
                .unwrap_or_default();
            this.update(cx, |this, cx| {
                for path in unchecked {
                    let can_merge = mergeable.get(&path).copied().unwrap_or(false);
                    this.mergeable.insert(path, can_merge);
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// Takes one side of a file and marks it resolved. A merge editor open for the file closes,
    /// since its result no longer applies; when it holds unapplied work, the user is asked first.
    pub fn accept(
        &mut self,
        path: RepoPath,
        choice: Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(sides) = self.sides.clone() else {
            return Task::ready(Err(anyhow!("the conflicts are still loading")));
        };
        let side = match choice {
            Choice::Yours => sides.yours_stage(),
            Choice::Theirs => sides.theirs_stage(),
        };
        let views = merge_views_of(&self.workspace, &self.repo, Some(&path), cx);
        let confirm = (!unapplied_files(&views, cx).is_empty()).then(|| {
            window.prompt(
                PromptLevel::Warning,
                &format!("Discard the merge in progress for {}?", path.as_unix_str()),
                Some("Its merge editor has changes that are not applied; accepting a side replaces them."),
                &["Discard", "Cancel"],
                cx,
            )
        });
        let workspace = self.workspace.clone();
        let repo = self.repo.clone();
        cx.spawn(async move |this, cx| {
            if let Some(confirm) = confirm
                && confirm.await.ok() != Some(0)
            {
                return Ok(());
            }
            this.update(cx, |this, cx| {
                this.busy = true;
                cx.notify();
            })
            .ok();
            let task = cx.update(|cx| {
                discard_views(views, cx);
                GitCli::checkout_side(&repo, vec![path], side, cx)
            });
            let result = task.await;
            cx.update(|cx| {
                if let Err(error) = &result {
                    let toast = OutcomeToast::error("Accept failed", &format!("{error:#}"));
                    show_toast(&workspace, toast, cx);
                }
            });
            this.update(cx, |this, cx| {
                this.busy = false;
                this.refresh(cx);
            })
            .ok();
            result
        })
    }

    /// Opens the file in the merge editor in place of the dialog. Once the editor applies its
    /// result, the dialog comes back if anything is left to do.
    pub fn merge(
        &mut self,
        path: RepoPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let workspace = self.workspace.clone();
        let repo = self.repo.clone();
        let open = MergeView::open(workspace.clone(), repo.clone(), path, window, cx);
        cx.emit(DismissEvent);
        window.spawn(cx, async move |cx| {
            match open.await {
                Ok(view) => workspace.update_in(cx, |_, window, cx| {
                    cx.subscribe_in(&view, window, move |_, _, _: &MergeApplied, window, cx| {
                        reopen_if_unfinished(repo.clone(), window, cx);
                    })
                    .detach();
                })?,
                Err(error) => {
                    workspace.update_in(cx, |workspace, window, cx| {
                        let toast = OutcomeToast::error("Merge failed", &format!("{error:#}"));
                        crate::branches_popup::show_workspace_toast(workspace, toast, cx);
                        Self::open(workspace, repo, window, cx);
                    })?;
                }
            }
            Ok(())
        })
    }

    /// Asks, then aborts the operation (HEAD goes back to where it was) and closes the dialog
    /// and any merge editors of the repository.
    pub fn abort(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<Result<()>> {
        let Some(operation) = self.operation() else {
            return Task::ready(Ok(()));
        };
        let what = operation_name(operation);
        let yours = self
            .sides
            .as_ref()
            .map(|sides| sides.yours.clone())
            .unwrap_or_default();
        let views = merge_views_of(&self.workspace, &self.repo, None, cx);
        let unapplied = unapplied_files(&views, cx);
        let message = if unapplied.is_empty() {
            format!("Abort the {what}?")
        } else {
            format!(
                "Abort the {what}? Discard the merge in progress for {}?",
                unapplied.join(", ")
            )
        };
        let detail = format!(
            "Files resolved so far are reset, open merge editors close without applying, and \
             {yours} goes back to where it was before the {what}. Uncommitted changes from before \
             the {what} may not be restored."
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(&detail),
            &["Abort", "Cancel"],
            cx,
        );
        let repo = self.repo.clone();
        let workspace = self.workspace.clone();
        cx.spawn(async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return Ok(());
            }
            this.update(cx, |this, cx| {
                this.busy = true;
                cx.notify();
            })
            .ok();
            let task = cx.update(|cx| match operation {
                Operation::Merge => GitCli::abort_merge(&repo, cx),
                Operation::Rebase => GitCli::abort_rebase(&repo, cx),
                Operation::CherryPick => GitCli::abort_cherry_pick(&repo, cx),
            });
            let result = task.await;
            cx.update(|cx| match &result {
                Ok(()) => {
                    discard_views(merge_views_of(&workspace, &repo, None, cx), cx);
                    let toast = OutcomeToast::success(format!("{} aborted", capitalized(what)));
                    show_toast(&workspace, toast, cx);
                }
                Err(error) => {
                    let toast =
                        OutcomeToast::error(&format!("Abort {what} failed"), &format!("{error:#}"));
                    show_toast(&workspace, toast, cx);
                }
            });
            this.update(cx, |this, cx| {
                if result.is_ok() {
                    cx.emit(DismissEvent);
                } else {
                    this.busy = false;
                    this.refresh(cx);
                }
            })
            .ok();
            result
        })
    }

    /// Commits the merge or cherry-pick, or continues the rebase. A rebase that stops on the
    /// next commit's conflicts keeps the dialog open with those.
    pub fn continue_operation(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(operation) = self.operation() else {
            return Task::ready(Ok(()));
        };
        let repo = self.repo.clone();
        let workspace = self.workspace.clone();
        self.busy = true;
        cx.notify();
        let guard = OperationGuard::begin(repo.entity_id(), cx);
        let task = match operation {
            Operation::Merge => GitCli::continue_merge(&repo, cx),
            Operation::Rebase => GitCli::continue_rebase(&repo, cx),
            Operation::CherryPick => GitCli::continue_cherry_pick(&repo, cx),
        };
        let what = capitalized(operation_name(operation));
        // Not tied to the dialog or the window: closing either while git runs keeps the outcome
        // toast, and the guard ends the operation whatever happens.
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            drop(guard);
            let toast = match &outcome {
                Ok(MergeOutcome::Merged | MergeOutcome::UpToDate) => {
                    Some(OutcomeToast::success(format!("{what} completed")))
                }
                Ok(MergeOutcome::Conflicts(_)) => None,
                Ok(MergeOutcome::Failed(message)) => {
                    Some(OutcomeToast::error(&format!("{what} failed"), message))
                }
                Err(error) => Some(OutcomeToast::error(
                    &format!("{what} failed"),
                    &format!("{error:#}"),
                )),
            };
            let finished = matches!(outcome, Ok(MergeOutcome::Merged | MergeOutcome::UpToDate));
            cx.update(|cx| {
                if let Some(toast) = toast {
                    show_toast(&workspace, toast, cx);
                }
            });
            this.update(cx, |this, cx| {
                this.busy = false;
                if finished {
                    cx.emit(DismissEvent);
                } else {
                    // A rebase's next commit: same file names, new contents.
                    this.mergeable.clear();
                    this.refresh(cx);
                }
            })
            .ok();
            if matches!(
                outcome,
                Ok(MergeOutcome::Merged | MergeOutcome::Conflicts(_))
            ) {
                close_stale_merge_views(&workspace, &repo, cx).await;
            }
            outcome.map(|_| ())
        })
    }

    fn dismiss(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn select_next(&mut self, _: &menu::SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected + 1 < self.paths.len() {
            self.selected += 1;
            cx.notify();
        }
    }

    fn select_previous(
        &mut self,
        _: &menu::SelectPrevious,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected > 0 {
            self.selected -= 1;
            cx.notify();
        }
    }

    /// Enter: Merge… on the selected file, or Continue once nothing is left.
    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_continue() {
            self.continue_operation(window, cx).detach();
            return;
        }
        if self.busy {
            return;
        }
        if let Some(path) = self.paths.get(self.selected).cloned()
            && self.can_merge(&path) == Some(true)
        {
            self.merge(path, window, cx).detach_and_log_err(cx);
        }
    }

    fn render_row(&self, ix: usize, path: &RepoPath, cx: &mut Context<Self>) -> impl IntoElement {
        let name = path.file_name().unwrap_or(path.as_unix_str()).to_string();
        let directory = path
            .parent()
            .map(|parent| parent.as_unix_str().to_string())
            .filter(|parent| !parent.is_empty());
        let mergeable = self.can_merge(path);
        let busy = self.busy;
        let accept = |choice: Choice| {
            let path = path.clone();
            cx.listener(move |this: &mut Self, _, window, cx| {
                this.accept(path.clone(), choice, window, cx)
                    .detach_and_log_err(cx);
            })
        };
        let accept_yours = accept(Choice::Yours);
        let accept_theirs = accept(Choice::Theirs);
        let merge_path = path.clone();
        h_flex()
            .id(("conflict", ix))
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .rounded_sm()
            .when(ix == self.selected, |this| {
                this.bg(cx.theme().colors().element_selected)
            })
            .hover(|this| this.bg(cx.theme().colors().element_hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = ix;
                cx.notify();
            }))
            .child(
                Icon::new(IconName::GitMergeConflict)
                    .size(IconSize::Small)
                    .color(Color::Conflict),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .overflow_x_hidden()
                    .child(Label::new(name).truncate())
                    .children(directory.map(|directory| {
                        Label::new(directory)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate()
                    })),
            )
            .child(
                Button::new(("accept-yours", ix), "Accept Yours")
                    .label_size(LabelSize::Small)
                    .disabled(busy)
                    .on_click(accept_yours),
            )
            .child(
                Button::new(("accept-theirs", ix), "Accept Theirs")
                    .label_size(LabelSize::Small)
                    .disabled(busy)
                    .on_click(accept_theirs),
            )
            .child(
                Button::new(("merge", ix), "Merge…")
                    .label_size(LabelSize::Small)
                    .disabled(busy || mergeable != Some(true))
                    .when(mergeable == Some(false), |this| {
                        this.tooltip(Tooltip::text(
                            "Binary, too large, a symlink or deleted on one side: take one side instead",
                        ))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.merge(merge_path.clone(), window, cx)
                            .detach_and_log_err(cx);
                    })),
            )
    }
}

impl ModalView for ConflictsDialog {}
impl EventEmitter<DismissEvent> for ConflictsDialog {}

impl Focusable for ConflictsDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConflictsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let operation = self.operation();
        let sides = self
            .sides
            .as_ref()
            .filter(|sides| !sides.yours.is_empty())
            .map(|sides| {
                let theirs = if sides.theirs.is_empty() {
                    "the other side"
                } else {
                    &sides.theirs
                };
                format!("Yours: {} · Theirs: {theirs}", sides.yours)
            });
        let rows = self
            .paths
            .iter()
            .enumerate()
            .map(|(ix, path)| self.render_row(ix, path, cx).into_any_element())
            .collect::<Vec<_>>();
        let status = if !self.loaded {
            Some("Loading…".to_string())
        } else if let Some(error) = &self.error {
            Some(format!("Could not list the conflicts: {error}"))
        } else if self.paths.is_empty() {
            Some(if operation.is_some() {
                "All conflicts are resolved. Continue to finish.".to_string()
            } else {
                "All conflicts are resolved.".to_string()
            })
        } else {
            None
        };
        let busy = self.busy;
        let can_continue = self.can_continue();
        v_flex()
            .key_context("BreweryConflictsDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .elevation_3(cx)
            .w(rems(46.))
            .p_2()
            .gap_2()
            .child(
                v_flex()
                    .px_2()
                    .gap_0p5()
                    .child(Headline::new(self.header()).size(HeadlineSize::Small))
                    .children(
                        sides.map(|sides| {
                            Label::new(sides).size(LabelSize::Small).color(Color::Muted)
                        }),
                    ),
            )
            .child(
                v_flex()
                    .id("conflicts-list")
                    .max_h(rems(24.))
                    .overflow_y_scroll()
                    .children(rows)
                    .children(status.map(|status| {
                        div()
                            .px_2()
                            .py_2()
                            .child(Label::new(status).color(Color::Muted))
                    })),
            )
            .child(
                h_flex()
                    .px_2()
                    .gap_1()
                    .when_some(operation, |this, _| {
                        this.child(
                            Button::new("conflicts-abort", "Abort")
                                .label_size(LabelSize::Small)
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.abort(window, cx).detach();
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("conflicts-close", "Close")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    )
                    .when_some(operation, |this, _| {
                        this.child(
                            Button::new("conflicts-continue", "Continue")
                                .label_size(LabelSize::Small)
                                .style(ButtonStyle::Filled)
                                .disabled(!can_continue)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.continue_operation(window, cx).detach();
                                })),
                        )
                    }),
            )
    }
}

fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::Merge => "merge",
        Operation::Rebase => "rebase",
        Operation::CherryPick => "cherry-pick",
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The merge editors of `repo` (only `path`'s, if given).
fn merge_views_of(
    workspace: &WeakEntity<Workspace>,
    repo: &Entity<Repository>,
    path: Option<&RepoPath>,
    cx: &App,
) -> Vec<Entity<MergeView>> {
    let Some(workspace) = workspace.upgrade() else {
        return Vec::new();
    };
    workspace
        .read(cx)
        .items_of_type::<MergeView>(cx)
        .filter(|view| {
            let view = view.read(cx);
            view.repo().entity_id() == repo.entity_id()
                && path.is_none_or(|path| view.path() == path)
        })
        .collect()
}

/// The files whose merge editors hold work that is not applied.
fn unapplied_files(views: &[Entity<MergeView>], cx: &App) -> Vec<String> {
    views
        .iter()
        .map(|view| view.read(cx))
        .filter(|view| view.has_unapplied_work())
        .map(|view| view.path().as_unix_str().to_string())
        .collect()
}

/// Closes merge editors without asking (the caller asked already, or nothing is unapplied).
fn discard_views(views: Vec<Entity<MergeView>>, cx: &mut App) {
    for view in views {
        view.update(cx, |view, cx| view.discard(cx));
    }
}

/// After a Continue: closes the repository's merge editors whose conflict is gone or has other
/// stages now (a rebase's next commit). Editors with unapplied work stay; their Apply refuses a
/// changed conflict and says so.
async fn close_stale_merge_views(
    workspace: &WeakEntity<Workspace>,
    repo: &Entity<Repository>,
    cx: &mut gpui::AsyncApp,
) {
    let views = cx.update(|cx| merge_views_of(workspace, repo, None, cx));
    for view in views {
        let (path, opened, dirty) = view.read_with(cx, |view, _| {
            (
                view.path().clone(),
                view.stage_ids().to_vec(),
                view.has_unapplied_work(),
            )
        });
        if dirty {
            continue;
        }
        let current = cx
            .update(|cx| GitCli::conflict_stage_ids(repo, path, cx))
            .await;
        if current.is_ok_and(|current| current != opened) {
            view.update(cx, |view, cx| view.discard(cx));
        }
    }
}

/// After the merge editor applied a file: back to the dialog while there is something left
/// (conflicts, or an operation to continue), unless another modal is open.
fn reopen_if_unfinished(
    repo: Entity<Repository>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let unmerged = GitCli::unmerged_paths(&repo, cx);
    let in_progress = GitCli::in_progress(&repo, cx);
    cx.spawn_in(window, async move |workspace, cx| {
        let unmerged = unmerged.await?;
        let in_progress = in_progress.await?;
        if unmerged.is_empty() && in_progress.is_none() {
            return anyhow::Ok(());
        }
        workspace.update_in(cx, |workspace, window, cx| {
            if !workspace.has_active_modal(window, cx) {
                ConflictsDialog::open(workspace, repo, window, cx);
            }
        })?;
        Ok(())
    })
    .detach_and_log_err(cx);
}

// ---------------------------------------------------------------------------------------------
// Conflict episodes

/// Which repositories' conflicts were already announced, and which are in the middle of a
/// Brewery-run operation (that opens the dialog itself).
#[derive(Default)]
struct ConflictEpisodes {
    announced: HashSet<EntityId>,
    /// Shared with each [`OperationGuard`], which decrements its repository's count on drop.
    brewery_running: Rc<RefCell<HashMap<EntityId, usize>>>,
}

impl Global for ConflictEpisodes {}

/// A Brewery merge, rebase or continue running on a repository: while one is alive, conflicts
/// it causes open the dialog and the watcher's toast stays away. Dropping the guard ends the
/// operation, on every path (success, error, a dropped task, window or dialog).
pub(crate) struct OperationGuard {
    running: Rc<RefCell<HashMap<EntityId, usize>>>,
    repo: EntityId,
}

impl OperationGuard {
    pub(crate) fn begin(repo: EntityId, cx: &mut App) -> Self {
        let running = cx
            .default_global::<ConflictEpisodes>()
            .brewery_running
            .clone();
        *running.borrow_mut().entry(repo).or_default() += 1;
        Self { running, repo }
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        let mut running = self.running.borrow_mut();
        if let Some(count) = running.get_mut(&self.repo) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                running.remove(&self.repo);
            }
        }
    }
}

/// Whether a Brewery operation is running on the repository.
pub(crate) fn operation_running(repo: EntityId, cx: &App) -> bool {
    cx.try_global::<ConflictEpisodes>().is_some_and(|episodes| {
        episodes
            .brewery_running
            .borrow()
            .get(&repo)
            .is_some_and(|count| *count > 0)
    })
}

/// The user is looking at `repo`'s conflicts already: no toast for this episode.
fn mark_episode(repo: &Entity<Repository>, cx: &mut App) {
    cx.default_global::<ConflictEpisodes>()
        .announced
        .insert(repo.entity_id());
}

fn conflicts_toast_id() -> NotificationId {
    NotificationId::named("brewery-git-conflicts".into())
}

/// Starts or ends a conflict episode for `repo` from its snapshot, and announces a new one
/// with a toast (never the dialog, which would take the focus).
fn check_episode(
    workspace: &mut Workspace,
    repo: &Entity<Repository>,
    cx: &mut Context<Workspace>,
) {
    // The status tree's summary counts conflicted files without walking the statuses.
    let conflicts = repo.read(cx).status_summary().conflict;
    let id = repo.entity_id();
    let episodes = cx.default_global::<ConflictEpisodes>();
    if conflicts == 0 {
        episodes.announced.remove(&id);
        return;
    }
    if !episodes.announced.insert(id) || operation_running(id, cx) {
        return;
    }
    if workspace
        .active_modal::<ConflictsDialog>(cx)
        .is_some_and(|dialog| dialog.read(cx).repo.entity_id() == id)
    {
        return;
    }
    let message = format!(
        "Conflicts in {conflicts} {}",
        if conflicts == 1 { "file" } else { "files" }
    );
    #[cfg(test)]
    crate::branches_popup::SHOWN_TOASTS.with(|shown| {
        shown.borrow_mut().push(OutcomeToast {
            kind: crate::branches_popup::ToastKind::Conflicts,
            message: message.clone(),
        })
    });
    let weak = cx.weak_entity();
    let repo = repo.clone();
    workspace.show_toast(
        Toast::new(conflicts_toast_id(), message).on_click("Resolve…", move |window, cx| {
            let repo = repo.clone();
            weak.update(cx, |workspace, cx| {
                ConflictsDialog::open(workspace, repo, window, cx);
            })
            .ok();
        }),
        cx,
    );
}

/// Opens the dialog for the active repository, or says there are no conflicts.
fn resolve_conflicts(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(repo) = workspace.project().read(cx).active_repository(cx) else {
        let toast = OutcomeToast::success("No conflicts");
        crate::branches_popup::show_workspace_toast(workspace, toast, cx);
        return;
    };
    let unmerged = GitCli::unmerged_paths(&repo, cx);
    let in_progress = GitCli::in_progress(&repo, cx);
    cx.spawn_in(window, async move |workspace, cx| {
        let unmerged = unmerged.await;
        let in_progress = in_progress.await.ok().flatten();
        workspace.update_in(cx, |workspace, window, cx| match unmerged {
            Ok(paths) if paths.is_empty() && in_progress.is_none() => {
                let toast = OutcomeToast::success("No conflicts");
                crate::branches_popup::show_workspace_toast(workspace, toast, cx);
            }
            Ok(_) => {
                ConflictsDialog::open(workspace, repo, window, cx);
            }
            Err(error) => {
                let toast = OutcomeToast::error("Listing conflicts failed", &format!("{error:#}"));
                crate::branches_popup::show_workspace_toast(workspace, toast, cx);
            }
        })
    })
    .detach_and_log_err(cx);
}

/// Opens the merge editor for a conflicted file from the git panel; files it cannot take
/// (binary, too large, deleted on one side) open the Conflicts dialog instead.
fn open_conflict_from_git_panel(
    workspace: WeakEntity<Workspace>,
    repo: Entity<Repository>,
    path: RepoPath,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if !workspace
        .upgrade()
        .is_some_and(|workspace| handles_conflicts(workspace.read(cx), cx))
    {
        return false;
    }
    let open = MergeView::open(workspace.clone(), repo.clone(), path, window, cx);
    window
        .spawn(cx, async move |cx| {
            if let Err(error) = open.await {
                log::info!("merge editor unavailable: {error:#}");
                workspace.update_in(cx, |workspace, window, cx| {
                    ConflictsDialog::open(workspace, repo, window, cx);
                })?;
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    true
}

/// Brewery resolves conflicts only in local projects: its git commands run on the local
/// repository. Remote (SSH, collab) projects keep Zed's own conflict UI.
pub(crate) fn handles_conflicts(workspace: &Workspace, cx: &App) -> bool {
    workspace.project().read(cx).is_local()
}

/// Registers `brewery::ResolveConflicts`, the git panel's conflict opener and the conflict
/// episode watcher.
pub fn init(cx: &mut App) {
    git_ui_core::brewery_hooks::set_conflict_opener(open_conflict_from_git_panel, cx);
    cx.observe_new(|workspace: &mut Workspace, _, cx| {
        workspace.register_action(|workspace, _: &ResolveConflicts, window, cx| {
            resolve_conflicts(workspace, window, cx)
        });
        let project = workspace.project().clone();
        if !project.read(cx).is_local() {
            return;
        }
        let git_store = project.read(cx).git_store().clone();
        cx.subscribe(&git_store, |workspace, git_store, event, cx| {
            let repos = match event {
                GitStoreEvent::RepositoryUpdated(id, RepositoryEvent::StatusesChanged, _) => {
                    git_store
                        .read(cx)
                        .repositories()
                        .get(id)
                        .cloned()
                        .into_iter()
                        .collect::<Vec<_>>()
                }
                GitStoreEvent::ConflictsUpdated => git_store
                    .read(cx)
                    .repositories()
                    .values()
                    .cloned()
                    .collect(),
                _ => return,
            };
            for repo in repos {
                check_episode(workspace, &repo, cx);
            }
        })
        .detach();
    })
    .detach();
}

#[cfg(test)]
#[path = "conflicts_dialog_tests.rs"]
mod tests;

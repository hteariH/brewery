//! Compares a file at a revision with the working copy or with the file at another revision,
//! in Zed's split diff editor, like `git_ui_core::file_diff_view::FileDiffView`.
//!
//! Revisions are resolved to commits once, when the view opens, and the file is read through
//! the repository's job queue (`GitCli`). The old (left) side is a read-only in-memory buffer;
//! the new (right) side is the project's worktree buffer for the working copy (editable, so
//! hunks can be restored or edited in place) or another read-only buffer for a revision.

use crate::cli::{GitCli, Oid, RepoPath};
use anyhow::{Result, anyhow, bail};
use buffer_diff::BufferDiff;
use editor::{
    Editor, EditorEvent, EditorSettings, HiddenUnstagedDiffHunkRenderer, MultiBuffer,
    SplittableEditor,
};
use futures::{FutureExt as _, select_biased};
use gpui::{
    AnyEntity, App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Font, IntoElement, Render, SharedString, Task, WeakEntity, Window,
};
use language::{Buffer, Capability, HighlightedText, Language, LanguageRegistry};
use project::git_store::Repository;
use project::{Project, ProjectPath};
use settings::Settings as _;
use std::any::{Any, TypeId};
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use ui::{Color, Icon, IconName, Label, prelude::*};
use workspace::item::{ItemEvent, SaveOptions};
use workspace::searchable::SearchableItemHandle;
use workspace::{Item, ItemHandle as _, ItemNavHistory, ToolbarItemLocation, Workspace};

/// Files larger than this (on either side) are not diffed.
pub const MAX_FILE_SIZE: usize = 5 * 1024 * 1024;
pub const BINARY_MESSAGE: &str = "Binary file — can't show differences";
pub const LARGE_MESSAGE: &str = "File is too large — can't show differences";
const WORKING_COPY: &str = "Working Copy";
const EMPTY: &str = "empty";
pub(crate) const SHORT_SHA_LEN: usize = 8;
pub(crate) const RECALCULATE_DIFF_DEBOUNCE: Duration = Duration::from_millis(250);

/// What the revision is compared with (the right-hand side).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewSide {
    /// The file in the working tree, as the project's (possibly unsaved) buffer.
    WorkingCopy,
    /// The file at another revision: a branch, tag, sha or any `git rev-parse` expression.
    Revision(String),
}

/// What is compared with (the left-hand side).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OldSide {
    /// The file at a revision: a branch, tag, sha or any `git rev-parse` expression.
    Revision(String),
    /// Nothing: the file did not exist (e.g. the parent side of a root commit), so the new side
    /// shows as entirely added.
    Empty,
}

/// A side of the comparison after its revision was resolved, when the view opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedSide {
    WorkingCopy,
    /// No file (the old side of a root commit).
    Empty,
    /// `rev` as the caller gave it and the commit it resolved to.
    Revision {
        rev: String,
        sha: Oid,
    },
}

impl ResolvedSide {
    /// `(main)`, `(abcdef12)` or `Working Copy`, as in the tab title.
    fn title(&self) -> String {
        match self {
            Self::WorkingCopy => WORKING_COPY.to_string(),
            Self::Empty => format!("({EMPTY})"),
            Self::Revision { rev, .. } => format!("({})", short_ref(rev)),
        }
    }

    /// `main (<full sha>)`, `<full sha>` or `Working Copy`, as in the tooltip.
    fn description(&self) -> String {
        match self {
            Self::WorkingCopy => WORKING_COPY.to_string(),
            Self::Empty => EMPTY.to_string(),
            Self::Revision { rev, sha } if *rev == sha.to_string() => rev.clone(),
            Self::Revision { rev, sha } => format!("{rev} ({sha})"),
        }
    }
}

/// A full or abbreviated hex sha is shortened; branch names and expressions stay as given.
fn short_ref(rev: &str) -> &str {
    if rev.len() > SHORT_SHA_LEN && rev.chars().all(|c| c.is_ascii_hexdigit()) {
        &rev[..SHORT_SHA_LEN]
    } else {
        rev
    }
}

enum Content {
    Diff {
        editor: Entity<SplittableEditor>,
        new_buffer: Entity<Buffer>,
        diff: Entity<BufferDiff>,
        _recalculate_diff_task: Task<Result<()>>,
    },
    Message {
        message: SharedString,
        focus_handle: FocusHandle,
    },
}

pub struct RevisionDiffView {
    /// The file on the new side.
    path: RepoPath,
    /// The file on the old side: differs from `path` across a rename.
    old_path: RepoPath,
    old: ResolvedSide,
    new: ResolvedSide,
    content: Content,
}

/// A side's file content, as read when the view opens.
enum Loaded {
    Missing,
    Text(String),
    Binary,
    TooLarge,
}

impl Loaded {
    fn from_bytes(bytes: Option<Vec<u8>>) -> Self {
        let Some(bytes) = bytes else {
            return Self::Missing;
        };
        if bytes.len() > MAX_FILE_SIZE {
            return Self::TooLarge;
        }
        // git's own heuristic: a NUL byte means binary. Non-UTF-8 text cannot be shown either.
        if bytes.contains(&0) {
            return Self::Binary;
        }
        match String::from_utf8(bytes) {
            Ok(text) => Self::Text(text),
            Err(_) => Self::Binary,
        }
    }

    fn text(&self) -> &str {
        match self {
            Self::Text(text) => text,
            _ => "",
        }
    }
}

impl RevisionDiffView {
    /// Opens `path` (repository-relative) at `old` compared with `new` in the workspace's
    /// active pane. Errors for unknown or `-`-prefixed revisions, a file missing on both
    /// sides and remote repositories; callers show them (e.g. as a toast).
    ///
    /// A `ProjectPath` maps to a repository and `RepoPath` with
    /// `GitStore::repository_and_path_for_project_path`.
    pub fn open(
        workspace: WeakEntity<Workspace>,
        repo: Entity<Repository>,
        path: RepoPath,
        old: String,
        new: NewSide,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Entity<Self>>> {
        Self::open_paths(
            workspace,
            repo,
            OldSide::Revision(old),
            path.clone(),
            new,
            path,
            window,
            cx,
        )
    }

    /// Like `open`, with the file under its own path on each side (a file renamed between the
    /// two) and an old side that may be empty (the parent of a root commit).
    #[allow(clippy::too_many_arguments)]
    pub fn open_paths(
        workspace: WeakEntity<Workspace>,
        repo: Entity<Repository>,
        old: OldSide,
        old_path: RepoPath,
        new: NewSide,
        path: RepoPath,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Entity<Self>>> {
        window.spawn(cx, async move |cx| {
            let project = workspace.update(cx, |workspace, _| workspace.project().clone())?;

            let old_side = match old {
                OldSide::Revision(rev) => ResolvedSide::Revision {
                    sha: resolve(&repo, &rev, cx).await?,
                    rev,
                },
                OldSide::Empty => ResolvedSide::Empty,
            };
            let new_side = match &new {
                NewSide::WorkingCopy => ResolvedSide::WorkingCopy,
                NewSide::Revision(rev) => ResolvedSide::Revision {
                    rev: rev.clone(),
                    sha: resolve(&repo, rev, cx).await?,
                },
            };
            let old_content = match &old_side {
                ResolvedSide::Revision { sha, .. } => {
                    read_revision(&repo, *sha, &old_path, cx).await?
                }
                _ => Loaded::Missing,
            };
            let new_content = match &new_side {
                ResolvedSide::Revision { sha, .. } => read_revision(&repo, *sha, &path, cx).await?,
                ResolvedSide::WorkingCopy => read_working_copy(&project, &repo, &path, cx).await?,
                ResolvedSide::Empty => Loaded::Missing,
            };

            let message = match (&old_content, &new_content) {
                (Loaded::Missing, Loaded::Missing) => bail!(
                    "{} exists neither at {} nor in {}",
                    path.as_unix_str(),
                    old_side.description(),
                    new_side.description()
                ),
                (Loaded::TooLarge, _) | (_, Loaded::TooLarge) => Some(LARGE_MESSAGE),
                (Loaded::Binary, _) | (_, Loaded::Binary) => Some(BINARY_MESSAGE),
                _ => None,
            };

            if let Some(message) = message {
                return workspace.update_in(cx, |workspace, window, cx| {
                    let view = cx.new(|cx| Self {
                        path,
                        old_path,
                        old: old_side,
                        new: new_side,
                        content: Content::Message {
                            message: message.into(),
                            focus_handle: cx.focus_handle(),
                        },
                    });
                    add_to_pane(workspace, &view, window, cx);
                    view
                });
            }

            let languages = project.read_with(cx, |project, _| project.languages().clone());
            let language = language_for_path(&languages, &path).await;
            let old_buffer = in_memory_buffer(old_content.text(), &language, &languages, cx);
            let new_buffer = match (&new_side, &new_content) {
                (ResolvedSide::WorkingCopy, Loaded::Text(_)) => {
                    open_worktree_buffer(&project, &repo, &path, cx)
                        .await?
                        .unwrap_or_else(|| {
                            in_memory_buffer(new_content.text(), &language, &languages, cx)
                        })
                }
                _ => in_memory_buffer(new_content.text(), &language, &languages, cx),
            };

            let diff = git_ui_core::file_diff_view::build_buffer_diff(&old_buffer, &new_buffer, cx)
                .await?;
            // The left side shows the diff's base text buffer: give it the path's language
            // even while the worktree buffer's own language is still loading.
            if let Some(language) = language {
                diff.read_with(cx, |diff, _| diff.base_text_buffer().clone())
                    .update(cx, |buffer, cx| {
                        buffer.set_language_async(Some(language), cx)
                    });
            }

            workspace.update_in(cx, |workspace, window, cx| {
                let workspace_entity = cx.entity();
                let view = cx.new(|cx| {
                    Self::new_diff(
                        path,
                        old_path,
                        old_side,
                        new_side,
                        old_content,
                        new_buffer,
                        diff,
                        project,
                        workspace_entity,
                        window,
                        cx,
                    )
                });
                add_to_pane(workspace, &view, window, cx);
                view
            })
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn new_diff(
        path: RepoPath,
        old_path: RepoPath,
        old: ResolvedSide,
        new: ResolvedSide,
        old_content: Loaded,
        new_buffer: Entity<Buffer>,
        diff: Entity<BufferDiff>,
        project: Entity<Project>,
        workspace: Entity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let multibuffer = cx.new(|cx| {
            let mut multibuffer = MultiBuffer::singleton(new_buffer.clone(), cx);
            multibuffer.add_diff(diff.clone(), cx);
            multibuffer
        });
        let editor = cx.new(|cx| {
            let splittable = SplittableEditor::new(
                EditorSettings::get_global(cx).diff_view_style,
                multibuffer,
                project,
                workspace,
                window,
                cx,
            );
            splittable.set_diff_hunk_renderer(Some(Arc::new(HiddenUnstagedDiffHunkRenderer)), cx);
            splittable
        });
        // Re-emitted so the pane updates the tab (dirty dot, title) and Zed's save logic runs.
        cx.subscribe(&editor, |_, _, event: &EditorEvent, cx| {
            cx.emit(event.clone())
        })
        .detach();
        let header = format!(
            "{} {} ↔ {}",
            paths_description(&old_path, &path),
            old.title(),
            new.title()
        );
        editor.update(cx, |editor, cx| {
            editor.rhs_editor().update(cx, |editor, _| {
                editor.set_breadcrumb_header(header);
            });
        });

        // A language the worktree buffer detects later (e.g. from file-type settings) wins.
        cx.subscribe(&new_buffer, {
            let base_text_buffer = diff.read(cx).base_text_buffer().downgrade();
            move |_, buffer, event, cx| {
                if let language::BufferEvent::LanguageChanged(_) = event
                    && let Some(language) = buffer.read(cx).language().cloned()
                {
                    base_text_buffer
                        .update(cx, |base_text_buffer, cx| {
                            base_text_buffer.set_language_async(Some(language), cx);
                        })
                        .ok();
                }
            }
        })
        .detach();

        // Only the working copy changes; the revision text is fixed.
        let (mut buffer_changes_tx, mut buffer_changes_rx) = watch::channel(());
        cx.subscribe(&new_buffer, move |_, _, event, _| match event {
            language::BufferEvent::Edited { .. }
            | language::BufferEvent::LanguageChanged(_)
            | language::BufferEvent::Reparsed => {
                buffer_changes_tx.send(()).ok();
            }
            _ => {}
        })
        .detach();

        let old_text: Arc<str> = old_content.text().into();
        let recalculate = cx.spawn({
            let diff = diff.clone();
            let new_buffer = new_buffer.downgrade();
            async move |_, cx| {
                while buffer_changes_rx.recv().await.is_ok() {
                    loop {
                        let mut timer = cx
                            .background_executor()
                            .timer(RECALCULATE_DIFF_DEBOUNCE)
                            .fuse();
                        let mut recv = pin!(buffer_changes_rx.recv().fuse());
                        select_biased! {
                            _ = timer => break,
                            _ = recv => continue,
                        }
                    }
                    let snapshot = new_buffer.read_with(cx, |buffer, _| buffer.text_snapshot())?;
                    diff.update(cx, |diff, cx| {
                        diff.set_base_text(Some(old_text.clone()), snapshot, cx)
                    })
                    .await;
                }
                Ok(())
            }
        });

        Self {
            path,
            old_path,
            old,
            new,
            content: Content::Diff {
                editor,
                new_buffer,
                diff,
                _recalculate_diff_task: recalculate,
            },
        }
    }

    /// The compared file, repository-relative.
    pub fn path(&self) -> &RepoPath {
        &self.path
    }

    /// The compared file on the old side: differs from `path` across a rename.
    pub fn old_path(&self) -> &RepoPath {
        &self.old_path
    }

    /// The left (old) side: a revision, or empty.
    pub fn old_side(&self) -> &ResolvedSide {
        &self.old
    }

    /// The right (new) side.
    pub fn new_side(&self) -> &ResolvedSide {
        &self.new
    }

    /// Why no diff is shown (binary or too large), if so.
    pub fn message(&self) -> Option<&SharedString> {
        match &self.content {
            Content::Message { message, .. } => Some(message),
            Content::Diff { .. } => None,
        }
    }

    pub fn splittable_editor(&self) -> Option<&Entity<SplittableEditor>> {
        match &self.content {
            Content::Diff { editor, .. } => Some(editor),
            Content::Message { .. } => None,
        }
    }

    /// The right side's buffer: the worktree buffer for the working copy.
    pub fn new_buffer(&self) -> Option<&Entity<Buffer>> {
        match &self.content {
            Content::Diff { new_buffer, .. } => Some(new_buffer),
            Content::Message { .. } => None,
        }
    }

    pub fn diff(&self) -> Option<&Entity<BufferDiff>> {
        match &self.content {
            Content::Diff { diff, .. } => Some(diff),
            Content::Message { .. } => None,
        }
    }

    /// The diff editor when its right side edits the working copy.
    fn working_copy_editor(&self) -> Option<&Entity<SplittableEditor>> {
        match &self.content {
            Content::Diff { editor, .. } if self.new == ResolvedSide::WorkingCopy => Some(editor),
            _ => None,
        }
    }

    fn file_name(&self) -> &str {
        self.path.file_name().unwrap_or(self.path.as_unix_str())
    }
}

/// `a.txt`, or `old.txt → new.txt` across a rename.
fn paths_description(old_path: &RepoPath, path: &RepoPath) -> String {
    if old_path == path {
        path.as_unix_str().to_string()
    } else {
        format!("{} → {}", old_path.as_unix_str(), path.as_unix_str())
    }
}

fn add_to_pane(
    workspace: &mut Workspace,
    view: &Entity<RevisionDiffView>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    workspace.active_pane().update(cx, |pane, cx| {
        pane.add_item(Box::new(view.clone()), true, true, None, window, cx);
    });
}

async fn resolve(repo: &Entity<Repository>, rev: &str, cx: &mut AsyncApp) -> Result<Oid> {
    cx.update(|cx| GitCli::resolve_commit(repo, rev, cx))
        .await?
        .ok_or_else(|| anyhow!("unknown revision {rev}"))
}

/// The file at `sha`. Its size is checked first, so a huge blob is never read into memory.
async fn read_revision(
    repo: &Entity<Repository>,
    sha: Oid,
    path: &RepoPath,
    cx: &mut AsyncApp,
) -> Result<Loaded> {
    let rev = sha.to_string();
    match cx
        .update(|cx| GitCli::blob_size(repo, &rev, path.clone(), cx))
        .await?
    {
        None => return Ok(Loaded::Missing),
        Some(size) if size > MAX_FILE_SIZE as u64 => return Ok(Loaded::TooLarge),
        Some(_) => {}
    }
    let bytes = cx
        .update(|cx| GitCli::show_bytes_at(repo, &rev, path.clone(), cx))
        .await?;
    Ok(Loaded::from_bytes(bytes))
}

/// The file on disk; an unsaved worktree buffer is picked up when the view opens it.
async fn read_working_copy(
    project: &Entity<Project>,
    repo: &Entity<Repository>,
    path: &RepoPath,
    cx: &mut AsyncApp,
) -> Result<Loaded> {
    let fs = project.read_with(cx, |project, _| project.fs().clone());
    let abs_path: PathBuf = repo.read_with(cx, |repo, _| {
        repo.work_directory_abs_path.join(path.as_std_path())
    });
    match fs.metadata(&abs_path).await? {
        Some(metadata) if !metadata.is_dir => {
            if metadata.len > MAX_FILE_SIZE as u64 {
                return Ok(Loaded::TooLarge);
            }
            Ok(Loaded::from_bytes(Some(fs.load_bytes(&abs_path).await?)))
        }
        _ => Ok(Loaded::Missing),
    }
}

async fn open_worktree_buffer(
    project: &Entity<Project>,
    repo: &Entity<Repository>,
    path: &RepoPath,
    cx: &mut AsyncApp,
) -> Result<Option<Entity<Buffer>>> {
    let project_path: Option<ProjectPath> =
        repo.read_with(cx, |repo, cx| repo.repo_path_to_project_path(path, cx));
    let Some(project_path) = project_path else {
        return Ok(None);
    };
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(project_path, cx))
        .await?;
    Ok(Some(buffer))
}

async fn language_for_path(
    languages: &Arc<LanguageRegistry>,
    path: &RepoPath,
) -> Option<Arc<Language>> {
    languages
        .load_language_for_file_path(path.as_std_path())
        .await
        .ok()
}

fn in_memory_buffer(
    text: &str,
    language: &Option<Arc<Language>>,
    languages: &Arc<LanguageRegistry>,
    cx: &mut AsyncApp,
) -> Entity<Buffer> {
    cx.new(|cx| {
        let mut buffer = Buffer::local(text, cx);
        buffer.set_language_registry(languages.clone());
        buffer.set_language(language.clone(), cx);
        buffer.set_capability(Capability::ReadOnly, cx);
        buffer
    })
}

impl EventEmitter<EditorEvent> for RevisionDiffView {}

impl Focusable for RevisionDiffView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.content {
            Content::Diff { editor, .. } => editor.focus_handle(cx),
            Content::Message { focus_handle, .. } => focus_handle.clone(),
        }
    }
}

impl Item for RevisionDiffView {
    type Event = EditorEvent;

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Diff).color(Color::Muted))
    }

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        format!(
            "{} {} ↔ {}",
            self.file_name(),
            self.old.title(),
            self.new.title()
        )
        .into()
    }

    fn tab_tooltip_text(&self, _cx: &App) -> Option<SharedString> {
        Some(
            format!(
                "{}\n{} ↔ {}",
                paths_description(&self.old_path, &self.path),
                self.old.description(),
                self.new.description()
            )
            .into(),
        )
    }

    fn to_item_events(event: &EditorEvent, f: &mut dyn FnMut(ItemEvent)) {
        Editor::to_item_events(event, f)
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        Some("Revision Diff View Opened")
    }

    fn deactivated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Content::Diff { editor, .. } = &self.content {
            editor.deactivated(window, cx);
        }
    }

    fn act_as_type<'a>(
        &'a self,
        type_id: TypeId,
        self_handle: &'a Entity<Self>,
        cx: &'a App,
    ) -> Option<AnyEntity> {
        if type_id == TypeId::of::<Self>() {
            Some(self_handle.clone().into())
        } else if let Content::Diff { editor, .. } = &self.content {
            editor.act_as_type(type_id, cx)
        } else {
            None
        }
    }

    fn as_searchable(&self, _: &Entity<Self>, _: &App) -> Option<Box<dyn SearchableItemHandle>> {
        match &self.content {
            Content::Diff { editor, .. } => Some(Box::new(editor.clone())),
            Content::Message { .. } => None,
        }
    }

    fn for_each_project_item(
        &self,
        cx: &App,
        f: &mut dyn FnMut(gpui::EntityId, &dyn project::ProjectItem),
    ) {
        if let Content::Diff { editor, .. } = &self.content {
            editor.for_each_project_item(cx, f)
        }
    }

    fn active_project_path(&self, cx: &App) -> Option<ProjectPath> {
        match &self.content {
            Content::Diff { editor, .. } => editor.read(cx).active_project_path(cx),
            Content::Message { .. } => None,
        }
    }

    fn set_nav_history(
        &mut self,
        nav_history: ItemNavHistory,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Content::Diff { editor, .. } = &self.content {
            editor.update(cx, |editor, cx| {
                editor.rhs_editor().update(cx, |editor, _| {
                    editor.set_nav_history(Some(nav_history));
                })
            });
        }
    }

    fn navigate(
        &mut self,
        data: Arc<dyn Any + Send>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match &self.content {
            Content::Diff { editor, .. } => editor.update(cx, |editor, cx| {
                editor
                    .rhs_editor()
                    .update(cx, |editor, cx| editor.navigate(data, window, cx))
            }),
            Content::Message { .. } => false,
        }
    }

    fn breadcrumb_location(&self, _: &App) -> ToolbarItemLocation {
        match &self.content {
            Content::Diff { .. } => ToolbarItemLocation::PrimaryLeft,
            Content::Message { .. } => ToolbarItemLocation::Hidden,
        }
    }

    fn breadcrumbs(&self, cx: &App) -> Option<(Vec<HighlightedText>, Option<Font>)> {
        match &self.content {
            Content::Diff { editor, .. } => editor.breadcrumbs(cx),
            Content::Message { .. } => None,
        }
    }

    fn added_to_workspace(
        &mut self,
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Content::Diff { editor, .. } = &self.content {
            editor.update(cx, |editor, cx| {
                editor.rhs_editor().update(cx, |editor, cx| {
                    editor.added_to_workspace(workspace, window, cx)
                })
            });
        }
    }

    // Only a working-copy compare edits a real file; its right side is the worktree buffer.
    fn is_dirty(&self, cx: &App) -> bool {
        self.working_copy_editor()
            .is_some_and(|editor| editor.read(cx).is_dirty(cx))
    }

    fn has_conflict(&self, cx: &App) -> bool {
        self.working_copy_editor()
            .is_some_and(|editor| editor.read(cx).has_conflict(cx))
    }

    fn has_deleted_file(&self, cx: &App) -> bool {
        self.working_copy_editor()
            .is_some_and(|editor| editor.read(cx).has_deleted_file(cx))
    }

    fn reload(
        &mut self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        match self.working_copy_editor().cloned() {
            Some(editor) => editor.update(cx, |editor, cx| editor.reload(project, window, cx)),
            None => Task::ready(Ok(())),
        }
    }

    fn can_save(&self, cx: &App) -> bool {
        match &self.content {
            Content::Diff { editor, .. } if self.new == ResolvedSide::WorkingCopy => {
                editor.read(cx).rhs_editor().read(cx).can_save(cx)
            }
            _ => false,
        }
    }

    fn save(
        &mut self,
        options: SaveOptions,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        match &self.content {
            Content::Diff { editor, .. } if self.new == ResolvedSide::WorkingCopy => {
                editor.save(options, project, window, cx)
            }
            _ => Task::ready(Ok(())),
        }
    }
}

impl Render for RevisionDiffView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match &self.content {
            Content::Diff { editor, .. } => editor.clone().into_any_element(),
            Content::Message {
                message,
                focus_handle,
            } => v_flex()
                .track_focus(focus_handle)
                .size_full()
                .items_center()
                .justify_center()
                .child(Label::new(message.clone()).color(Color::Muted))
                .into_any_element(),
        }
    }
}

#[cfg(test)]
#[path = "revision_diff_view_tests.rs"]
mod tests;

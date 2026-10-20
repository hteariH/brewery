//! IntelliJ's 3-pane merge editor: yours | result | theirs.
//!
//! The inputs are the conflicted file's index stages (`:1:` base, `:2:` ours, `:3:` theirs),
//! read through the repository's job queue. The left pane is always the user's own side and
//! the right pane the other side, whatever git calls them: during a rebase git swaps the
//! stages (see [`ConflictSides`]), so there the left pane shows stage 3.
//!
//! [`merge3`] splits the three texts into chunks. The result pane starts with every
//! non-conflicting change applied and each conflict showing its base text. Each chunk keeps an
//! anchor range in the result buffer, so the result stays fully editable: a manual edit inside a
//! chunk marks it `Edited` (resolved), and accepting a side for that chunk later still replaces
//! its current lines (Accept Left / Right for the whole file skip hand-edited chunks). Per-chunk `>>` / `<<` / `x` buttons are editor blocks above the chunk in the
//! side panes. Scrolling one pane scrolls the others so the same chunk stays aligned at the top.
//!
//! Apply writes the result to the worktree file (through the project buffer when it is open,
//! else straight to disk), marks it resolved with `git add` and closes the view. Save (Ctrl+S,
//! Save All, Save in the close prompt) only writes the result: the file stays conflicted and
//! the view stays open, as in IntelliJ where only Apply marks the file resolved.

use crate::cli::{ConflictSides, ConflictStages, GitCli, Operation, RepoPath, StageBlob};
use crate::merge3::{self, ChunkKind, Merge3, MergeResult, Resolution, Side, Version};
use crate::revision_diff_view::{MAX_FILE_SIZE, in_memory_buffer, language_for_path};
use anyhow::{Result, bail};
use editor::display_map::{
    BlockContext, BlockPlacement, BlockProperties, BlockStyle, CustomBlockId, DisplayRow,
};
use editor::{
    Bias, DisplayPoint, Editor, EditorEvent, RowHighlightOptions, SelectionEffects,
    scroll::Autoscroll,
};
use gpui::{
    AnyElement, App, AppContext as _, AsyncApp, Context, Entity, EntityId, EventEmitter,
    FocusHandle, Focusable, Hsla, IntoElement, PromptLevel, Render, SharedString, Subscription,
    Task, WeakEntity, Window, actions,
};
use language::{Buffer, BufferEvent, LineEnding, Point, TextBufferSnapshot, ToOffset as _};
use project::Project;
use project::git_store::Repository;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use ui::{Tooltip, prelude::*};
use workspace::item::{ItemBufferKind, ItemEvent, SaveOptions};
use workspace::notifications::NotificationId;
use workspace::{Item, Toast, Workspace};

actions!(
    brewery_merge,
    [
        /// Moves the cursor in the merge result to the next conflict.
        NextConflict,
        /// Moves the cursor in the merge result to the previous conflict.
        PreviousConflict,
        /// Takes the whole left (yours) file as the result.
        AcceptLeft,
        /// Takes the whole right (theirs) file as the result.
        AcceptRight,
        /// Writes the result, marks the file resolved and closes the merge editor.
        Apply,
        /// Closes the merge editor without changing the file.
        Cancel,
    ]
);

pub const BINARY_MESSAGE: &str = "is a binary file: take one side instead";
pub const LARGE_MESSAGE: &str = "is too large to merge: take one side instead";
pub const DELETED_MESSAGE: &str = "was deleted on one side: take one side instead";
pub const NOT_A_FILE_MESSAGE: &str = "is a symlink or a submodule: take one side instead";
const RESULT_LABEL: &str = "Result";

/// One of the three panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Left,
    Result,
    Right,
}

/// What a conflicted file offers the merge editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeInput {
    /// All present sides are text; `base` is empty when there is no common ancestor.
    Text {
        base: String,
        yours: String,
        theirs: String,
    },
    Binary,
    TooLarge,
    /// One side deleted the file (a modify/delete conflict).
    Deleted,
    /// A side is a symlink or a submodule, not a regular file.
    NotAFile,
}

impl MergeInput {
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }
}

/// Reads a conflicted file's stages and who is on which side.
pub async fn load_input(
    repo: &Entity<Repository>,
    path: &RepoPath,
    cx: &mut AsyncApp,
) -> Result<(ConflictSides, MergeInput)> {
    let sides = cx.update(|cx| GitCli::conflict_sides(repo, cx)).await?;
    let stages = cx
        .update(|cx| GitCli::conflict_stages(repo, path.clone(), MAX_FILE_SIZE as u64, cx))
        .await?;
    let input = merge_input(stages, &sides, path)?;
    Ok((sides, input))
}

/// What a conflicted file's stages offer the merge editor. [`GitCli::mergeable_conflicts`]
/// answers [`MergeInput::is_text`] for all conflicted files at once and must agree with this.
pub(crate) fn merge_input(
    stages: ConflictStages,
    sides: &ConflictSides,
    path: &RepoPath,
) -> Result<MergeInput> {
    let (yours, theirs) = match sides.yours_stage() {
        crate::cli::Side::Ours => (stages.ours, stages.theirs),
        crate::cli::Side::Theirs => (stages.theirs, stages.ours),
    };
    let base = stages.base;
    if [&base, &yours, &theirs]
        .iter()
        .all(|blob| **blob == StageBlob::Missing)
    {
        bail!("{} is not conflicted", path.as_unix_str());
    }
    if [&base, &yours, &theirs]
        .iter()
        .any(|blob| matches!(blob, StageBlob::NotAFile(_)))
    {
        return Ok(MergeInput::NotAFile);
    }
    if [&base, &yours, &theirs]
        .iter()
        .any(|blob| **blob == StageBlob::TooLarge)
    {
        return Ok(MergeInput::TooLarge);
    }
    let text = |blob: StageBlob| -> Option<Option<String>> {
        match blob {
            StageBlob::Bytes(bytes) if bytes.contains(&0) => None,
            StageBlob::Bytes(bytes) => String::from_utf8(bytes).ok().map(Some),
            _ => Some(None),
        }
    };
    let (Some(base), Some(yours), Some(theirs)) = (text(base), text(yours), text(theirs)) else {
        return Ok(MergeInput::Binary);
    };
    let (Some(yours), Some(theirs)) = (yours, theirs) else {
        return Ok(MergeInput::Deleted);
    };
    Ok(MergeInput::Text {
        base: base.unwrap_or_default(),
        yours,
        theirs,
    })
}

pub struct MergeView {
    project: Entity<Project>,
    workspace: WeakEntity<Workspace>,
    repo: Entity<Repository>,
    path: RepoPath,
    sides: ConflictSides,
    /// In merge3 terms `Ours` is the left pane (yours) and `Theirs` the right pane.
    merge: Merge3,
    resolutions: Vec<Resolution>,
    left: Entity<Editor>,
    result: Entity<Editor>,
    right: Entity<Editor>,
    result_buffer: Entity<Buffer>,
    /// Each chunk's range in the result buffer. Starts are left-biased, so text typed at a
    /// boundary belongs to the chunk that starts there; ends are left-biased too, except the
    /// last chunk's, which grows with text typed at the end of the file.
    anchors: Vec<Range<language::Anchor>>,
    /// The result buffer as last accounted for, and each chunk's offsets in it.
    synced: TextBufferSnapshot,
    synced_offsets: Vec<Range<usize>>,
    left_blocks: HashSet<CustomBlockId>,
    right_blocks: HashSet<CustomBlockId>,
    /// Scroll events still to come from editors this view scrolled itself.
    pending_scrolls: HashMap<EntityId, usize>,
    /// The user changed something (took a side, ignored a chunk, edited the result).
    touched: bool,
    /// The result was applied, or the merge was abandoned: closing needs no confirmation.
    finished: bool,
    /// The conflict's index records when the view opened (`GitCli::conflict_stage_ids`).
    /// Apply refuses when they changed: the view would write a result for another conflict.
    stage_ids: Vec<String>,
    /// The working tree file when the view opened; `None` when it did not exist.
    opened_file: Option<Vec<u8>>,
    /// The working tree file as this view last saved it, and the result it saved: Apply does
    /// not take that file for a change made outside, and the view is clean until the next
    /// edit.
    saved: Option<(Vec<u8>, TextBufferSnapshot)>,
    /// Each chunk's text in the result when the view opened (LF line endings).
    initial_texts: Vec<String>,
    /// Chunk offsets and resolutions for result texts this view produced itself, keyed by a
    /// hash of the text, so undo and redo of an accept restore the chunk state exactly.
    known_states: HashMap<u64, (Vec<Range<usize>>, Vec<Resolution>)>,
    _subscriptions: Vec<Subscription>,
}

/// Why Apply did not write the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// The file is no longer conflicted: it was resolved, or the merge was ended, elsewhere.
    NotConflicted(String),
    /// The file has a different conflict now (e.g. the next commit of a rebase).
    ConflictChanged(String),
    /// The user declined to replace changes made outside the merge editor.
    Cancelled,
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConflicted(path) => write!(
                f,
                "{path} is no longer conflicted: it was resolved, or the merge was ended, \
                 outside the merge editor"
            ),
            Self::ConflictChanged(path) => write!(
                f,
                "the conflict in {path} changed since the merge editor opened"
            ),
            Self::Cancelled => write!(f, "the merge was not applied"),
        }
    }
}

impl std::error::Error for ApplyError {}

/// Emitted once the result is written and the file is marked resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MergeApplied;

impl MergeView {
    /// Opens the 3-pane merge editor for a conflicted file (repository-relative) in the
    /// workspace's active pane, or activates the one already open. Errors for binary, too
    /// large and modify/delete conflicts (use [`MergeView::can_merge`] first), files that are
    /// not conflicted and remote repositories.
    pub fn open(
        workspace: WeakEntity<Workspace>,
        repo: Entity<Repository>,
        path: RepoPath,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Entity<Self>>> {
        window.spawn(cx, async move |cx| {
            let project = workspace.update(cx, |workspace, _| workspace.project().clone())?;
            let stage_ids = cx
                .update(|_, cx| GitCli::conflict_stage_ids(&repo, path.clone(), cx))?
                .await?;

            // A view already open for this file is reused only while its conflict is current.
            let repo_id = repo.entity_id();
            let existing = workspace.update(cx, |workspace, cx| {
                workspace.items_of_type::<MergeView>(cx).find(|view| {
                    let view = view.read(cx);
                    !view.finished && view.path == path && view.repo.entity_id() == repo_id
                })
            })?;
            if let Some(existing) = existing {
                let (current, dirty) = existing.read_with(cx, |view, _| {
                    (view.stage_ids == stage_ids, view.has_unapplied_work())
                });
                let reload = if current {
                    false
                } else if dirty {
                    let answer = cx.update(|window, cx| {
                        window.prompt(
                            PromptLevel::Warning,
                            &format!(
                                "The conflict in {} changed since the merge editor opened",
                                path.as_unix_str()
                            ),
                            Some("Reload the merge editor? What you did there is not applied and is lost."),
                            &["Reload", "Keep"],
                            cx,
                        )
                    })?;
                    answer.await.ok() == Some(0)
                } else {
                    true
                };
                if !reload {
                    workspace.update_in(cx, |workspace, window, cx| {
                        workspace.activate_item(&existing, true, true, window, cx);
                    })?;
                    return Ok(existing);
                }
                existing.update(cx, |view, cx| view.discard(cx));
            }

            let (sides, input) = load_input(&repo, &path, cx).await?;
            let (base, yours, theirs) = match input {
                MergeInput::Text {
                    base,
                    yours,
                    theirs,
                } => (base, yours, theirs),
                MergeInput::Binary => bail!("{} {BINARY_MESSAGE}", path.as_unix_str()),
                MergeInput::TooLarge => bail!("{} {LARGE_MESSAGE}", path.as_unix_str()),
                MergeInput::Deleted => bail!("{} {DELETED_MESSAGE}", path.as_unix_str()),
                MergeInput::NotAFile => bail!("{} {NOT_A_FILE_MESSAGE}", path.as_unix_str()),
            };
            let merge = cx
                .background_spawn(async move { merge3::merge3(&base, &yours, &theirs) })
                .await;

            let languages = project.read_with(cx, |project, _| project.languages().clone());
            let language = language_for_path(&languages, &path).await;
            let left_buffer =
                in_memory_buffer(merge.text(Version::Ours), &language, &languages, cx);
            let right_buffer =
                in_memory_buffer(merge.text(Version::Theirs), &language, &languages, cx);
            let abs_path = repo.read_with(cx, |repo, _| {
                repo.work_directory_abs_path.join(path.as_std_path())
            });
            let fs = project.read_with(cx, |project, _| project.fs().clone());
            let opened_file = fs.load_bytes(&abs_path).await.ok();
            let result = MergeResult::new(&merge);
            let result_buffer = cx.new(|cx| {
                let mut buffer = Buffer::local(result.text(), cx);
                buffer.set_language_registry(languages.clone());
                buffer.set_language(language.clone(), cx);
                buffer
            });

            workspace.update_in(cx, |workspace, window, cx| {
                let weak_workspace = cx.entity().downgrade();
                let view = cx.new(|cx| {
                    Self::new(
                        project,
                        weak_workspace,
                        repo,
                        path,
                        sides,
                        merge,
                        result,
                        stage_ids,
                        opened_file,
                        [left_buffer, result_buffer, right_buffer],
                        window,
                        cx,
                    )
                });
                workspace.active_pane().update(cx, |pane, cx| {
                    pane.add_item(Box::new(view.clone()), true, true, None, window, cx);
                });
                view
            })
        })
    }

    /// Whether the merge editor can open this conflicted file: false for binary, too large,
    /// symlink, submodule and modify/delete conflicts, which can only take one side.
    pub fn can_merge(
        repo: &Entity<Repository>,
        path: RepoPath,
        cx: &mut App,
    ) -> Task<Result<bool>> {
        let repo = repo.clone();
        cx.spawn(async move |cx| {
            let (_, input) = load_input(&repo, &path, cx).await?;
            Ok(input.is_text())
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        repo: Entity<Repository>,
        path: RepoPath,
        sides: ConflictSides,
        merge: Merge3,
        result: MergeResult,
        stage_ids: Vec<String>,
        opened_file: Option<Vec<u8>>,
        [left_buffer, result_buffer, right_buffer]: [Entity<Buffer>; 3],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let side_editor = |buffer: Entity<Buffer>, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut editor = Editor::for_buffer(buffer, None, window, cx);
                editor.set_read_only(true);
                editor
            })
        };
        let left = side_editor(left_buffer, window, cx);
        let right = side_editor(right_buffer, window, cx);
        let result_editor =
            cx.new(|cx| Editor::for_buffer(result_buffer.clone(), None, window, cx));

        let snapshot = result_buffer.read(cx).text_snapshot();
        let max_point = snapshot.max_point();
        let line_offset = |line: usize| {
            let point = Point::new(line as u32, 0);
            if point > max_point {
                snapshot.len()
            } else {
                point.to_offset(&snapshot)
            }
        };
        let offsets = result
            .chunks()
            .iter()
            .map(|chunk| line_offset(chunk.lines.start)..line_offset(chunk.lines.end))
            .collect::<Vec<_>>();
        let resolutions = result
            .chunks()
            .iter()
            .map(|chunk| chunk.resolution)
            .collect();

        let mut subscriptions = vec![cx.subscribe(&result_buffer, |this, _, event, cx| {
            if let BufferEvent::Edited { .. } = event {
                this.result_edited(cx);
            }
        })];
        for (editor, column) in [
            (&left, Column::Left),
            (&result_editor, Column::Result),
            (&right, Column::Right),
        ] {
            subscriptions.push(cx.subscribe_in(
                editor,
                window,
                move |this, _, event: &EditorEvent, window, cx| {
                    if let EditorEvent::ScrollPositionChanged { local: true, .. } = event {
                        this.sync_scroll(column, window, cx);
                    }
                },
            ));
        }

        let mut view = Self {
            project,
            workspace,
            repo,
            path,
            sides,
            merge,
            resolutions,
            left,
            result: result_editor,
            right,
            result_buffer,
            anchors: Vec::new(),
            synced: snapshot.clone(),
            synced_offsets: Vec::new(),
            left_blocks: HashSet::new(),
            right_blocks: HashSet::new(),
            pending_scrolls: HashMap::new(),
            touched: false,
            finished: false,
            initial_texts: offsets
                .iter()
                .map(|range| snapshot.text_for_range(range.clone()).collect())
                .collect(),
            stage_ids,
            opened_file,
            saved: None,
            known_states: HashMap::new(),
            _subscriptions: subscriptions,
        };
        view.reanchor(&snapshot, offsets);
        view.remember_state(&snapshot);
        view.refresh(cx);
        if let Some(&first) = view.conflict_chunks().first() {
            view.move_to_chunk(first, window, cx);
        }
        view
    }

    pub fn repo(&self) -> &Entity<Repository> {
        &self.repo
    }

    /// The conflict's index records when the view opened (`GitCli::conflict_stage_ids`).
    pub fn stage_ids(&self) -> &[String] {
        &self.stage_ids
    }

    /// The conflicted file, repository-relative.
    pub fn path(&self) -> &RepoPath {
        &self.path
    }

    pub fn sides(&self) -> &ConflictSides {
        &self.sides
    }

    /// `Yours (main)`.
    pub fn left_label(&self) -> SharedString {
        side_label("Yours", &self.sides.yours)
    }

    /// `Theirs (feature)`.
    pub fn right_label(&self) -> SharedString {
        side_label("Theirs", &self.sides.theirs)
    }

    pub fn left_editor(&self) -> &Entity<Editor> {
        &self.left
    }

    pub fn result_editor(&self) -> &Entity<Editor> {
        &self.result
    }

    pub fn right_editor(&self) -> &Entity<Editor> {
        &self.right
    }

    /// The chunks, with `Ours` = the left pane and `Theirs` = the right pane.
    pub fn merge(&self) -> &Merge3 {
        &self.merge
    }

    /// How each chunk of `merge().chunks` is resolved.
    pub fn resolutions(&self) -> &[Resolution] {
        &self.resolutions
    }

    pub fn result_text(&self, cx: &App) -> String {
        self.result_buffer.read(cx).text()
    }

    /// Indices of the conflict chunks, resolved or not.
    pub fn conflict_chunks(&self) -> Vec<usize> {
        self.merge
            .chunks
            .iter()
            .enumerate()
            .filter(|(_, chunk)| chunk.is_conflict())
            .map(|(ix, _)| ix)
            .collect()
    }

    pub fn unresolved_count(&self) -> usize {
        self.resolutions
            .iter()
            .filter(|resolution| !resolution.is_resolved())
            .count()
    }

    /// Replaces chunk `chunk`'s current lines in the result with `side`'s (`Both` = yours then
    /// theirs) and marks it resolved that way.
    pub fn accept_chunk(&mut self, chunk: usize, side: Side, cx: &mut Context<Self>) {
        self.replace_chunk(chunk, side, cx);
        self.refresh(cx);
    }

    /// Marks a chunk resolved, keeping the result's lines as they are.
    pub fn ignore_chunk(&mut self, chunk: usize, cx: &mut Context<Self>) {
        if let Some(resolution) = self.resolutions.get_mut(chunk) {
            *resolution = Resolution::Ignored;
            self.touch(cx);
            self.refresh(cx);
        }
    }

    /// The whole left file becomes the result: every change takes yours, except chunks the
    /// user edited by hand.
    pub fn accept_left(&mut self, cx: &mut Context<Self>) {
        self.accept_everywhere(Side::Ours, cx);
    }

    /// The whole right file becomes the result: every change takes theirs, except chunks the
    /// user edited by hand.
    pub fn accept_right(&mut self, cx: &mut Context<Self>) {
        self.accept_everywhere(Side::Theirs, cx);
    }

    fn accept_everywhere(&mut self, side: Side, cx: &mut Context<Self>) {
        // Account for edits not synced yet, so a chunk typed into just now counts as edited.
        self.result_edited(cx);
        for chunk in 0..self.merge.chunks.len() {
            if self.merge.chunks[chunk].is_change() && self.resolutions[chunk] != Resolution::Edited
            {
                self.replace_chunk(chunk, side, cx);
            }
        }
        self.refresh(cx);
    }

    pub fn next_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.cursor_row(cx);
        let rows = self.result_rows(cx);
        if let Some(chunk) = self
            .conflict_chunks()
            .into_iter()
            .find(|&chunk| rows[chunk].start > row)
        {
            self.move_to_chunk(chunk, window, cx);
        }
    }

    pub fn previous_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.cursor_row(cx);
        let rows = self.result_rows(cx);
        if let Some(chunk) = self
            .conflict_chunks()
            .into_iter()
            .rev()
            .find(|&chunk| rows[chunk].start < row)
        {
            self.move_to_chunk(chunk, window, cx);
        }
    }

    /// Writes the result to the file, marks it resolved and closes the view. Does not ask about
    /// unresolved conflicts; the Apply button does.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<Result<()>> {
        let write = self.write_result(true, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            write.await?;
            this.update(cx, |_, cx| cx.emit(ItemEvent::CloseItem))?;
            Ok(())
        })
    }

    /// Closes this view and opens a new one for the file's current conflict.
    pub fn reload(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Self>>> {
        self.discard(cx);
        MergeView::open(
            self.workspace.clone(),
            self.repo.clone(),
            self.path.clone(),
            window,
            cx,
        )
    }

    /// Conflicts Apply warns about: unresolved ones, and any conflict whose result lines are
    /// still its starting (base) text without a side taken, e.g. ignored ones.
    pub fn conflicts_without_a_side(&self, cx: &App) -> usize {
        let snapshot = self.result_buffer.read(cx).text_snapshot();
        let offsets = self.chunk_offsets(&snapshot);
        self.conflict_chunks()
            .into_iter()
            .filter(|&ix| match self.resolutions[ix] {
                Resolution::Unresolved => true,
                Resolution::Ours | Resolution::Theirs | Resolution::Both => false,
                _ => snapshot
                    .text_for_range(offsets[ix].clone())
                    .equals_str(&self.initial_texts[ix]),
            })
            .count()
    }

    /// Closes the view without asking and without touching the file, e.g. after the merge was
    /// aborted.
    pub fn discard(&mut self, cx: &mut Context<Self>) {
        self.finished = true;
        cx.emit(ItemEvent::CloseItem);
    }

    /// Whether the user changed something that is not applied yet.
    pub fn has_unapplied_work(&self) -> bool {
        self.touched && !self.finished
    }

    /// Unapplied work that is not saved to the file either.
    fn has_unsaved_work(&self, cx: &App) -> bool {
        self.has_unapplied_work()
            && self.saved.as_ref().is_none_or(|(_, snapshot)| {
                self.result_buffer
                    .read(cx)
                    .version()
                    .changed_since(snapshot.version())
            })
    }

    fn touch(&mut self, cx: &mut Context<Self>) {
        if !self.touched {
            self.touched = true;
            cx.emit(ItemEvent::UpdateTab);
        }
    }

    /// Asks first when conflicts are left; `false` when the user declined.
    fn confirm_unresolved(&self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        let unresolved = self.conflicts_without_a_side(cx);
        if unresolved == 0 {
            return Task::ready(true);
        }
        let message = if unresolved == 1 {
            "1 conflict is not resolved".to_string()
        } else {
            format!("{unresolved} conflicts are not resolved")
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(
                "The result is written as it is now, and the file is marked resolved. A \
                 conflict without a side taken keeps its base text, without either side's \
                 change.",
            ),
            &["Apply Anyway", "Cancel"],
            cx,
        );
        cx.background_spawn(async move { answer.await.ok() == Some(0) })
    }

    /// Writes the result to the worktree file and, with `mark_resolved`, marks it resolved.
    ///
    /// Refuses ([`ApplyError`]) when the file is no longer conflicted or has another conflict
    /// now, and asks before replacing changes made outside the merge editor: unsaved edits in
    /// the file's open buffer, a file changed on disk since the view opened, or a file that
    /// had no conflict markers left when it opened (resolved by hand, e.g. with Zed's inline
    /// conflict buttons).
    ///
    /// Line endings: the stages are read as git checks them out (eol conversion applied), so
    /// the result has the working tree's line endings. An open buffer keeps its own; a file
    /// written to disk takes the line ending the file has now.
    fn write_result(
        &mut self,
        mark_resolved: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let (snapshot, line_ending) = {
            let buffer = self.result_buffer.read(cx);
            (buffer.text_snapshot(), buffer.line_ending())
        };
        let rope = snapshot.as_rope().clone();
        let stage_ids = self.stage_ids.clone();
        let opened_file = self.opened_file.clone();
        let saved_file = self.saved.as_ref().map(|(file, _)| file.clone());
        let repo = self.repo.clone();
        let path = self.path.clone();
        let project = self.project.clone();
        let open_buffer = repo
            .read(cx)
            .repo_path_to_project_path(&path, cx)
            .and_then(|project_path| project.read(cx).get_open_buffer(&project_path, cx));
        let abs_path = repo
            .read(cx)
            .work_directory_abs_path
            .join(path.as_std_path());
        let fs = project.read(cx).fs().clone();
        cx.spawn_in(window, async move |this, cx| {
            let display_path = path.as_unix_str().to_string();
            let current = cx
                .update(|_, cx| GitCli::conflict_stage_ids(&repo, path.clone(), cx))?
                .await?;
            if current.is_empty() {
                return Err(ApplyError::NotConflicted(display_path).into());
            }
            if current != stage_ids {
                return Err(ApplyError::ConflictChanged(display_path).into());
            }
            // A symlink put in the file's place would make the save write through it.
            if fs
                .metadata(&abs_path)
                .await?
                .is_some_and(|metadata| metadata.is_symlink)
            {
                bail!("{display_path} {NOT_A_FILE_MESSAGE}");
            }

            let on_disk = fs.load_bytes(&abs_path).await.ok();
            let buffer_dirty = open_buffer
                .as_ref()
                .is_some_and(|buffer| buffer.read_with(cx, |buffer, _| buffer.is_dirty()));
            // The file as this view saved it is not a change made outside.
            let saved_by_view = saved_file.is_some() && on_disk == saved_file;
            let changed_outside = buffer_dirty
                || !saved_by_view
                    && (on_disk != opened_file
                        || !opened_file.as_deref().is_some_and(has_conflict_markers));
            if changed_outside {
                let file_name = path.file_name().unwrap_or(path.as_unix_str()).to_string();
                let answer = this.update_in(cx, |_, window, cx| {
                    window.prompt(
                        PromptLevel::Warning,
                        &format!(
                            "{file_name} has changes outside the merge editor. Replace them \
                             with the merge result?"
                        ),
                        None,
                        &["Replace", "Cancel"],
                        cx,
                    )
                })?;
                if answer.await.ok() != Some(0) {
                    return Err(ApplyError::Cancelled.into());
                }
            }

            match open_buffer {
                Some(buffer) => {
                    // The buffer keeps the line ending it detected in the working tree file.
                    buffer.update(cx, |buffer, cx| {
                        buffer.set_text(rope.to_string(), cx);
                    });
                    project
                        .update(cx, |project, cx| project.save_buffer(buffer, cx))
                        .await?;
                }
                None => {
                    let line_ending = on_disk
                        .as_deref()
                        .and_then(|bytes| std::str::from_utf8(bytes).ok())
                        .filter(|text| text.contains('\n'))
                        .map(LineEnding::detect)
                        .unwrap_or(line_ending);
                    fs.save(&abs_path, &rope, line_ending).await?
                }
            }
            if !mark_resolved {
                let written = fs.load_bytes(&abs_path).await?;
                this.update(cx, |this, cx| {
                    this.saved = Some((written, snapshot));
                    cx.emit(ItemEvent::UpdateTab);
                })?;
                return Ok(());
            }
            cx.update(|_, cx| GitCli::mark_resolved(&repo, vec![path], cx))?
                .await?;
            this.update(cx, |this, cx| {
                this.finished = true;
                cx.emit(ItemEvent::UpdateTab);
                cx.emit(MergeApplied);
            })?;
            Ok(())
        })
    }

    /// The Apply button: asks first when conflicts are left, shows failures as a toast.
    fn apply_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let confirm = self.confirm_unresolved(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if !confirm.await {
                return anyhow::Ok(());
            }
            let task = this.update_in(cx, |this, window, cx| this.apply(window, cx))?;
            let Err(error) = task.await else {
                return Ok(());
            };
            match error.downcast_ref::<ApplyError>() {
                Some(ApplyError::Cancelled) => {}
                Some(stale @ (ApplyError::NotConflicted(_) | ApplyError::ConflictChanged(_))) => {
                    let changed = matches!(stale, ApplyError::ConflictChanged(_));
                    let message = stale.to_string();
                    let answers: &[&str] = if changed {
                        &["Reload", "Cancel"]
                    } else {
                        &["Close Merge Editor", "Cancel"]
                    };
                    let answer = this.update_in(cx, |_, window, cx| {
                        window.prompt(PromptLevel::Warning, &message, None, answers, cx)
                    })?;
                    if answer.await.ok() == Some(0) {
                        this.update_in(cx, |this, window, cx| {
                            if changed {
                                this.reload(window, cx).detach_and_log_err(cx);
                            } else {
                                this.discard(cx);
                            }
                        })?;
                    }
                }
                None => this.update(cx, |this, cx| this.show_error(&error, cx))?,
            }
            Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(ItemEvent::CloseItem);
    }

    fn show_error(&self, error: &anyhow::Error, cx: &mut Context<Self>) {
        let message = format!("Could not apply the merge: {error:#}");
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.show_toast(Toast::new(NotificationId::unique::<Self>(), message), cx)
            })
            .ok();
    }

    // -----------------------------------------------------------------------------------------
    // The result buffer and its chunks
    // -----------------------------------------------------------------------------------------

    fn chunk_offsets(&self, snapshot: &TextBufferSnapshot) -> Vec<Range<usize>> {
        self.anchors
            .iter()
            .map(|range| {
                let start = range.start.to_offset(snapshot);
                start..range.end.to_offset(snapshot).max(start)
            })
            .collect()
    }

    /// Anchors the chunks at `offsets`. Text typed at a boundary goes to the chunk after it,
    /// except after an empty chunk, which takes it (so typing at an empty conflict fills that
    /// conflict) and after the last chunk (text typed at the end of the file).
    fn reanchor(&mut self, snapshot: &TextBufferSnapshot, offsets: Vec<Range<usize>>) {
        let last = offsets.len().saturating_sub(1);
        let boundary_takes_insertions = |ix: usize| ix == last || offsets[ix].is_empty();
        self.anchors = offsets
            .iter()
            .enumerate()
            .map(|(ix, range)| {
                let start = if ix > 0 && boundary_takes_insertions(ix - 1) {
                    snapshot.anchor_after(range.start)
                } else {
                    snapshot.anchor_before(range.start)
                };
                let end = if boundary_takes_insertions(ix) {
                    snapshot.anchor_after(range.end)
                } else {
                    snapshot.anchor_before(range.end)
                };
                start..end
            })
            .collect();
        self.synced = snapshot.clone();
        self.synced_offsets = offsets;
    }

    fn replace_chunk(&mut self, chunk: usize, side: Side, cx: &mut Context<Self>) {
        if chunk >= self.merge.chunks.len() {
            return;
        }
        self.touch(cx);
        // Account for any user edits first, so they are not mistaken for this one.
        self.result_edited(cx);
        let snapshot = self.result_buffer.read(cx).text_snapshot();
        let mut offsets = self.chunk_offsets(&snapshot);
        let range = offsets[chunk].clone();

        let mut piece = self.merge.choice_text(chunk, side);
        LineEnding::normalize(&mut piece);
        let mut new_text = String::new();
        // Keep lines from running together across the chunk's edges.
        let line_before_open =
            range.start > 0 && snapshot.reversed_chars_at(range.start).next() != Some('\n');
        if !piece.is_empty() && line_before_open {
            new_text.push('\n');
        }
        new_text.push_str(&piece);
        if !piece.is_empty() && !piece.ends_with('\n') && range.end < snapshot.len() {
            new_text.push('\n');
        }

        self.result_buffer.update(cx, |buffer, cx| {
            buffer.edit([(range.clone(), new_text.as_str())], None, cx);
        });
        let delta = new_text.len() as isize - range.len() as isize;
        offsets[chunk] = range.start..range.start + new_text.len();
        for later in offsets.iter_mut().skip(chunk + 1) {
            *later =
                later.start.saturating_add_signed(delta)..later.end.saturating_add_signed(delta);
        }
        let snapshot = self.result_buffer.read(cx).text_snapshot();
        self.reanchor(&snapshot, offsets);
        self.resolutions[chunk] = match side {
            Side::Ours => Resolution::Ours,
            Side::Theirs => Resolution::Theirs,
            Side::Both => Resolution::Both,
        };
        self.remember_state(&snapshot);
    }

    fn remember_state(&mut self, snapshot: &TextBufferSnapshot) {
        self.known_states.insert(
            text_hash(snapshot),
            (self.synced_offsets.clone(), self.resolutions.clone()),
        );
    }

    /// Marks the chunks touched by edits made since the last sync as `Edited`.
    fn result_edited(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.result_buffer.read(cx).text_snapshot();
        let edits = snapshot
            .edits_since::<usize>(self.synced.version())
            .collect::<Vec<_>>();
        if edits.is_empty() {
            return;
        }
        self.touch(cx);
        // Back to a text this view produced (undo or redo of an accept, or of the edits after
        // one): restore that state exactly.
        if let Some((offsets, resolutions)) = self.known_states.get(&text_hash(&snapshot)).cloned()
            && offsets.last().is_none_or(|last| last.end == snapshot.len())
        {
            self.reanchor(&snapshot, offsets);
            self.resolutions = resolutions;
            self.refresh(cx);
            return;
        }
        let new_offsets = self.chunk_offsets(&snapshot);
        let mut changed = false;
        for (ix, (old, new)) in self.synced_offsets.iter().zip(&new_offsets).enumerate() {
            let touched = edits
                .iter()
                .any(|edit| touches(&edit.old, &edit.new, old, new));
            if touched && self.resolutions[ix] != Resolution::Edited {
                self.resolutions[ix] = Resolution::Edited;
                changed = true;
            }
            // Edited back to where it started: nothing was chosen.
            if self.resolutions[ix] == Resolution::Edited
                && snapshot
                    .text_for_range(new.clone())
                    .equals_str(&self.initial_texts[ix])
            {
                self.resolutions[ix] = if self.merge.chunks[ix].is_conflict() {
                    Resolution::Unresolved
                } else {
                    Resolution::Auto
                };
                changed = true;
            }
        }
        self.synced = snapshot;
        self.synced_offsets = new_offsets;
        if changed {
            self.refresh(cx);
        } else {
            // Rows moved: highlights follow their anchors, but blocks are re-placed.
            cx.notify();
        }
    }

    /// Each chunk's row range in the result.
    fn result_rows(&self, cx: &App) -> Vec<Range<u32>> {
        let snapshot = self.result_buffer.read(cx).text_snapshot();
        self.chunk_offsets(&snapshot)
            .into_iter()
            .map(|range| {
                let start = snapshot.offset_to_point(range.start);
                let end = snapshot.offset_to_point(range.end);
                let end_row = if end.column == 0 || range.is_empty() {
                    end.row
                } else {
                    end.row + 1
                };
                start.row..end_row.max(start.row)
            })
            .collect()
    }

    fn rows(&self, column: Column, cx: &App) -> Vec<Range<u32>> {
        let version = match column {
            Column::Left => Version::Ours,
            Column::Right => Version::Theirs,
            Column::Result => return self.result_rows(cx),
        };
        self.merge
            .chunks
            .iter()
            .map(|chunk| {
                let range = chunk.range(version);
                range.start as u32..range.end as u32
            })
            .collect()
    }

    fn editor(&self, column: Column) -> &Entity<Editor> {
        match column {
            Column::Left => &self.left,
            Column::Result => &self.result,
            Column::Right => &self.right,
        }
    }

    fn cursor_row(&self, cx: &mut App) -> u32 {
        self.result.update(cx, |editor, cx| {
            let snapshot = editor.display_snapshot(cx);
            editor.selections.newest::<Point>(&snapshot).head().row
        })
    }

    fn move_to_chunk(&mut self, chunk: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.result_rows(cx).get(chunk).map(|rows| rows.start) else {
            return;
        };
        let point = Point::new(row, 0);
        self.result.update(cx, |editor, cx| {
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::center()),
                window,
                cx,
                |selections| selections.select_ranges([point..point]),
            );
        });
        window.focus(&self.result.focus_handle(cx), cx);
    }

    // -----------------------------------------------------------------------------------------
    // Scroll sync
    // -----------------------------------------------------------------------------------------

    /// The display row (blocks included) of a buffer row in a pane.
    fn display_row_of(&self, column: Column, row: u32, cx: &mut App) -> u32 {
        self.editor(column).update(cx, |editor, cx| {
            editor
                .display_snapshot(cx)
                .point_to_display_point(Point::new(row, 0), Bias::Left)
                .row()
                .0
        })
    }

    fn sync_scroll(&mut self, from: Column, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.editor(from).clone();
        if let Some(pending) = self.pending_scrolls.get_mut(&source.entity_id())
            && *pending > 0
        {
            *pending -= 1;
            return;
        }
        let top = source.update(cx, |editor, cx| {
            let y = editor.scroll_position(cx).y.max(0.);
            let row = editor
                .display_snapshot(cx)
                .display_point_to_point(DisplayPoint::new(DisplayRow(y as u32), 0), Bias::Left)
                .row;
            row as f64 + y.fract()
        });
        let from_rows = self.rows(from, cx);
        for to in [Column::Left, Column::Result, Column::Right] {
            if to == from {
                continue;
            }
            let target = aligned_row(&from_rows, &self.rows(to, cx), top);
            let display_row = self.display_row_of(to, target as u32, cx) as f64 + target.fract();
            let editor = self.editor(to).clone();
            let moved = editor.update(cx, |editor, cx| {
                let before = editor.scroll_position(cx);
                if (before.y - display_row).abs() < 1e-3 {
                    return false;
                }
                editor.set_scroll_position(gpui::point(before.x, display_row), window, cx);
                editor.scroll_position(cx) != before
            });
            if moved {
                *self.pending_scrolls.entry(editor.entity_id()).or_default() += 1;
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Highlights and per-chunk buttons
    // -----------------------------------------------------------------------------------------

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let view = cx.weak_entity();
        for column in [Column::Left, Column::Right] {
            let version = if column == Column::Left {
                Version::Ours
            } else {
                Version::Theirs
            };
            let mut highlights = Vec::new();
            let mut blocks = Vec::new();
            for (ix, chunk) in self.merge.chunks.iter().enumerate() {
                let range = chunk.range(version);
                let kind = match (chunk.kind, column) {
                    (ChunkKind::Conflict, _) if self.resolutions[ix].is_resolved() => {
                        Some(RowKind::Resolved)
                    }
                    (ChunkKind::Conflict, _) => Some(RowKind::Conflict),
                    (ChunkKind::Ours | ChunkKind::BothSame, Column::Left) => Some(RowKind::Yours),
                    (ChunkKind::Theirs | ChunkKind::BothSame, Column::Right) => {
                        Some(RowKind::Theirs)
                    }
                    _ => None,
                };
                if let Some(kind) = kind {
                    highlights.push((range.start as u32..range.end as u32, kind));
                }
                if chunk.is_conflict()
                    && let Some(controls) = ChunkControls::new(column, self.resolutions[ix])
                {
                    blocks.push((range.start as u32, ix, controls));
                }
            }
            let old_blocks = std::mem::take(if column == Column::Left {
                &mut self.left_blocks
            } else {
                &mut self.right_blocks
            });
            let new_blocks = self.editor(column).update(cx, |editor, cx| {
                editor.remove_blocks(old_blocks.into_iter().collect(), None, cx);
                highlight_rows(editor, highlights, cx);
                let snapshot = editor.buffer().read(cx).snapshot(cx);
                let max_point = snapshot.max_point();
                let blocks = blocks.into_iter().map(|(row, chunk, controls)| {
                    let point = Point::new(row, 0).min(max_point);
                    let view = view.clone();
                    BlockProperties {
                        placement: BlockPlacement::Above(snapshot.anchor_before(point)),
                        height: Some(1),
                        style: BlockStyle::Sticky,
                        render: Arc::new(move |cx| {
                            render_chunk_controls(view.clone(), chunk, controls, cx)
                        }),
                        priority: 0,
                    }
                });
                editor.insert_blocks(blocks, None, cx)
            });
            *(if column == Column::Left {
                &mut self.left_blocks
            } else {
                &mut self.right_blocks
            }) = new_blocks.into_iter().collect();
        }

        let rows = self.result_rows(cx);
        let highlights = self
            .merge
            .chunks
            .iter()
            .zip(&self.resolutions)
            .zip(rows)
            .filter_map(|((chunk, resolution), rows)| {
                let kind = match (resolution, chunk.kind) {
                    (Resolution::Unresolved, _) => RowKind::Conflict,
                    (Resolution::Auto, ChunkKind::Unchanged) => return None,
                    (Resolution::Auto, ChunkKind::Ours) | (Resolution::Ours, _) => RowKind::Yours,
                    (Resolution::Auto, ChunkKind::Theirs) | (Resolution::Theirs, _) => {
                        RowKind::Theirs
                    }
                    _ => RowKind::Resolved,
                };
                Some((rows, kind))
            })
            .collect();
        self.result
            .update(cx, |editor, cx| highlight_rows(editor, highlights, cx));
        cx.notify();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let unresolved = self.unresolved_count();
        let status = match unresolved {
            0 => "All conflicts resolved".to_string(),
            1 => "1 conflict left".to_string(),
            count => format!("{count} conflicts left"),
        };
        let apply_label = if unresolved == 0 { "Apply" } else { "Apply…" };
        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                IconButton::new("merge-previous-conflict", IconName::ArrowUp)
                    .tooltip(Tooltip::text("Previous Conflict"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.previous_conflict(window, cx)),
                    ),
            )
            .child(
                IconButton::new("merge-next-conflict", IconName::ArrowDown)
                    .tooltip(Tooltip::text("Next Conflict"))
                    .on_click(cx.listener(|this, _, window, cx| this.next_conflict(window, cx))),
            )
            .child(
                Label::new(status)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(div().flex_1())
            .child(
                Button::new("merge-accept-left", "Accept Left")
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text("Take the whole left file"))
                    .on_click(cx.listener(|this, _, _, cx| this.accept_left(cx))),
            )
            .child(
                Button::new("merge-accept-right", "Accept Right")
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text("Take the whole right file"))
                    .on_click(cx.listener(|this, _, _, cx| this.accept_right(cx))),
            )
            .child(
                Button::new("merge-cancel", "Cancel")
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
            )
            .child(
                Button::new("merge-apply", apply_label)
                    .label_size(LabelSize::Small)
                    .style(ButtonStyle::Filled)
                    .on_click(cx.listener(|this, _, window, cx| this.apply_clicked(window, cx))),
            )
    }
}

fn side_label(side: &str, name: &str) -> SharedString {
    if name.is_empty() {
        side.to_string().into()
    } else {
        format!("{side} ({name})").into()
    }
}

/// Whether an edit (its range before and after) touches a chunk (its offsets before and
/// after). A chunk with lines is touched by an edit that overlaps them, before or after. An
/// empty chunk is touched only by text inserted at its position or an edit spanning across
/// it: deleting up to it (e.g. the newline of the line above) leaves it alone.
fn touches(
    edit_old: &Range<usize>,
    edit_new: &Range<usize>,
    old_chunk: &Range<usize>,
    new_chunk: &Range<usize>,
) -> bool {
    let overlaps = |edit: &Range<usize>, chunk: &Range<usize>| {
        !edit.is_empty() && !chunk.is_empty() && edit.start < chunk.end && chunk.start < edit.end
    };
    if old_chunk.is_empty() {
        let at = old_chunk.start;
        (edit_old.start < at && at < edit_old.end) || (edit_old.start == at && !edit_new.is_empty())
    } else {
        overlaps(edit_old, old_chunk) || overlaps(edit_new, new_chunk)
    }
}

fn text_hash(snapshot: &TextBufferSnapshot) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for chunk in snapshot.as_rope().chunks() {
        chunk.hash(&mut hasher);
    }
    snapshot.len().hash(&mut hasher);
    hasher.finish()
}

/// Whether a file still has git's conflict markers (a `<<<<<<<` and a `>>>>>>>` line).
fn has_conflict_markers(bytes: &[u8]) -> bool {
    let mut lines = bytes.split(|byte| *byte == b'\n');
    lines.any(|line| line.starts_with(b"<<<<<<<")) && lines.any(|line| line.starts_with(b">>>>>>>"))
}

/// The row in another pane that shows the same place as `row` (fractional, the top of the
/// view) in this one: the same offset into the same chunk, kept inside the other pane's chunk.
fn aligned_row(from: &[Range<u32>], to: &[Range<u32>], row: f64) -> f64 {
    let line = row.max(0.) as u32;
    let chunk = from
        .iter()
        .position(|range| range.start <= line && line < range.end)
        .or_else(|| from.iter().rposition(|range| range.start <= line));
    let Some(chunk) = chunk else {
        return row;
    };
    let (Some(source), Some(target)) = (from.get(chunk), to.get(chunk)) else {
        return row;
    };
    let offset = row - source.start as f64;
    if target.is_empty() {
        target.start as f64
    } else if offset >= target.len() as f64 {
        (target.end - 1) as f64
    } else {
        target.start as f64 + offset
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    Conflict,
    Yours,
    Theirs,
    Resolved,
}

struct ConflictRows;
struct YoursRows;
struct TheirsRows;
struct ResolvedRows;

fn conflict_color(cx: &App) -> Hsla {
    cx.theme().status().conflict_background.opacity(0.3)
}

fn yours_color(cx: &App) -> Hsla {
    cx.theme().colors().version_control_conflict_marker_ours
}

fn theirs_color(cx: &App) -> Hsla {
    cx.theme().colors().version_control_conflict_marker_theirs
}

fn resolved_color(cx: &App) -> Hsla {
    cx.theme().status().ignored_background.opacity(0.2)
}

fn highlight_rows(editor: &mut Editor, rows: Vec<(Range<u32>, RowKind)>, cx: &mut Context<Editor>) {
    editor.clear_row_highlights::<ConflictRows>();
    editor.clear_row_highlights::<YoursRows>();
    editor.clear_row_highlights::<TheirsRows>();
    editor.clear_row_highlights::<ResolvedRows>();
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let max_point = snapshot.max_point();
    let options = RowHighlightOptions {
        include_gutter: true,
        ..Default::default()
    };
    for (rows, kind) in rows {
        if rows.is_empty() {
            continue;
        }
        let start = snapshot.anchor_before(Point::new(rows.start, 0).min(max_point));
        let end = snapshot.anchor_before(Point::new(rows.end, 0).min(max_point));
        // A last line without a newline ends at the buffer end, not at a line start.
        let end = if Point::new(rows.end, 0) > max_point {
            snapshot.anchor_after(max_point)
        } else {
            end
        };
        let range = start..end;
        match kind {
            RowKind::Conflict => {
                editor.highlight_rows::<ConflictRows>(range, conflict_color, options, cx)
            }
            RowKind::Yours => editor.highlight_rows::<YoursRows>(range, yours_color, options, cx),
            RowKind::Theirs => {
                editor.highlight_rows::<TheirsRows>(range, theirs_color, options, cx)
            }
            RowKind::Resolved => {
                editor.highlight_rows::<ResolvedRows>(range, resolved_color, options, cx)
            }
        }
    }
}

/// The buttons above a conflict in a side pane.
#[derive(Clone, Copy, Debug)]
struct ChunkControls {
    column: Column,
    /// What the arrow button takes.
    side: Side,
    /// Whether `x` (ignore) is offered: only while unresolved.
    ignore: bool,
}

impl ChunkControls {
    fn new(column: Column, resolution: Resolution) -> Option<Self> {
        let (side, ignore) = match (column, resolution) {
            (Column::Left, Resolution::Unresolved) => (Side::Ours, true),
            (Column::Right, Resolution::Unresolved) => (Side::Theirs, true),
            // The other side was taken: this one can still be added to it.
            (Column::Left, Resolution::Theirs) | (Column::Right, Resolution::Ours) => {
                (Side::Both, false)
            }
            _ => return None,
        };
        Some(Self {
            column,
            side,
            ignore,
        })
    }
}

fn render_chunk_controls(
    view: WeakEntity<MergeView>,
    chunk: usize,
    controls: ChunkControls,
    cx: &mut BlockContext,
) -> AnyElement {
    let (label, tooltip) = match (controls.column, controls.side) {
        (Column::Left, Side::Both) => (">> Both", "Add yours above theirs in the result"),
        (Column::Left, _) => (">>", "Accept yours into the result"),
        (_, Side::Both) => ("<< Both", "Add theirs below yours in the result"),
        _ => ("<<", "Accept theirs into the result"),
    };
    let side = controls.side;
    h_flex()
        .id(cx.block_id)
        .h(cx.line_height)
        .ml(cx.margins.gutter.width)
        .gap_1()
        .bg(cx.theme().colors().editor_background)
        .child(
            Button::new(("merge-chunk-accept", chunk), label)
                .label_size(LabelSize::Small)
                .tooltip(Tooltip::text(tooltip))
                .on_click({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |view, cx| view.accept_chunk(chunk, side, cx))
                            .ok();
                    }
                }),
        )
        .when(controls.ignore, |this| {
            this.child(
                Button::new(("merge-chunk-ignore", chunk), "x")
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text("Ignore: keep the result's lines"))
                    .on_click(move |_, _, cx| {
                        view.update(cx, |view, cx| view.ignore_chunk(chunk, cx))
                            .ok();
                    }),
            )
        })
        .into_any()
}

impl EventEmitter<ItemEvent> for MergeView {}
impl EventEmitter<MergeApplied> for MergeView {}

impl Focusable for MergeView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.result.focus_handle(cx)
    }
}

impl Item for MergeView {
    type Event = ItemEvent;

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::GitMergeConflict).color(Color::Muted))
    }

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        let name = self.path.file_name().unwrap_or(self.path.as_unix_str());
        format!("{name} (merge)").into()
    }

    fn tab_tooltip_text(&self, _cx: &App) -> Option<SharedString> {
        let operation = match self.sides.operation {
            Some(Operation::Rebase) => "rebase",
            Some(Operation::CherryPick) => "cherry-pick",
            _ => "merge",
        };
        Some(
            format!(
                "{} ({operation})\n{} ↔ {}",
                self.path.as_unix_str(),
                self.left_label(),
                self.right_label()
            )
            .into(),
        )
    }

    fn to_item_events(event: &ItemEvent, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }

    // Unsaved, unapplied work makes the view dirty, so closing it (the tab, Cancel, quitting)
    // goes through Zed's "Save / Don't Save / Cancel" prompt, where Save writes the result
    // without marking the file resolved. Zed only asks for singleton items without project
    // items, hence `Singleton`.
    fn buffer_kind(&self, _cx: &App) -> ItemBufferKind {
        ItemBufferKind::Singleton
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.has_unsaved_work(cx)
    }

    fn can_save(&self, _cx: &App) -> bool {
        true
    }

    /// Writes the result to the file; the file stays conflicted and the view stays open (Apply
    /// is the explicit step that stages it and closes the view).
    fn save(
        &mut self,
        _options: SaveOptions,
        _project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.write_result(false, window, cx)
    }

    /// "Don't Save" in the close prompt: the result is dropped with the view.
    fn reload(
        &mut self,
        _project: Entity<Project>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }
}

impl Render for MergeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().colors().border;
        let pane = |label: SharedString, editor: &Entity<Editor>, first: bool| {
            v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .when(!first, |this| this.border_l_1().border_color(border))
                .child(
                    h_flex()
                        .px_2()
                        .py_0p5()
                        .border_b_1()
                        .border_color(border)
                        .child(Label::new(label).size(LabelSize::Small)),
                )
                .child(div().flex_1().min_h_0().child(editor.clone()))
        };
        v_flex()
            .key_context("BreweryMergeView")
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .on_action(
                cx.listener(|this, _: &NextConflict, window, cx| this.next_conflict(window, cx)),
            )
            .on_action(cx.listener(|this, _: &PreviousConflict, window, cx| {
                this.previous_conflict(window, cx)
            }))
            .on_action(cx.listener(|this, _: &AcceptLeft, _, cx| this.accept_left(cx)))
            .on_action(cx.listener(|this, _: &AcceptRight, _, cx| this.accept_right(cx)))
            .on_action(cx.listener(|this, _: &Apply, window, cx| this.apply_clicked(window, cx)))
            .on_action(cx.listener(|this, _: &Cancel, _, cx| this.cancel(cx)))
            .child(self.render_toolbar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(pane(self.left_label(), &self.left, true))
                    .child(pane(RESULT_LABEL.into(), &self.result, false))
                    .child(pane(self.right_label(), &self.right, false)),
            )
    }
}

#[cfg(test)]
#[path = "merge_view_tests.rs"]
mod tests;

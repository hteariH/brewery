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
//! chunk marks it `Edited` (resolved), and accepting a side later still replaces the chunk's
//! current lines. Per-chunk `>>` / `<<` / `x` buttons are editor blocks above the chunk in the
//! side panes. Scrolling one pane scrolls the others so the same chunk stays aligned at the top.
//!
//! Apply writes the result to the worktree file (through the project buffer when it is open,
//! else straight to disk), marks it resolved with `git add` and closes the view.

use crate::cli::{ConflictSides, GitCli, Operation, RepoPath, StageBlob};
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
use workspace::item::ItemEvent;
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
        .any(|blob| **blob == StageBlob::TooLarge)
    {
        return Ok((sides, MergeInput::TooLarge));
    }
    let text = |blob: StageBlob| -> Option<Option<String>> {
        match blob {
            StageBlob::Bytes(bytes) if bytes.contains(&0) => None,
            StageBlob::Bytes(bytes) => String::from_utf8(bytes).ok().map(Some),
            _ => Some(None),
        }
    };
    let (Some(base), Some(yours), Some(theirs)) = (text(base), text(yours), text(theirs)) else {
        return Ok((sides, MergeInput::Binary));
    };
    let (Some(yours), Some(theirs)) = (yours, theirs) else {
        return Ok((sides, MergeInput::Deleted));
    };
    Ok((
        sides,
        MergeInput::Text {
            base: base.unwrap_or_default(),
            yours,
            theirs,
        },
    ))
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
    _subscriptions: Vec<Subscription>,
}

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
            let result = MergeResult::new(&merge);
            let result_buffer = cx.new(|cx| {
                let mut buffer = Buffer::local(result.text(), cx);
                buffer.set_language_registry(languages.clone());
                buffer.set_language(language.clone(), cx);
                buffer
            });

            workspace.update_in(cx, |workspace, window, cx| {
                let repo_id = repo.entity_id();
                let existing = workspace.items_of_type::<MergeView>(cx).find(|view| {
                    let view = view.read(cx);
                    view.path == path && view.repo.entity_id() == repo_id
                });
                if let Some(existing) = existing {
                    workspace.activate_item(&existing, true, true, window, cx);
                    return existing;
                }
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

    /// Whether the merge editor can open this conflicted file: false for binary, too large and
    /// modify/delete conflicts, which can only take one side.
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
            _subscriptions: subscriptions,
        };
        view.reanchor(&snapshot, offsets);
        view.refresh(cx);
        if let Some(&first) = view.conflict_chunks().first() {
            view.move_to_chunk(first, window, cx);
        }
        view
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
            self.refresh(cx);
        }
    }

    /// The whole left file becomes the result: every change takes yours.
    pub fn accept_left(&mut self, cx: &mut Context<Self>) {
        self.accept_everywhere(Side::Ours, cx);
    }

    /// The whole right file becomes the result: every change takes theirs.
    pub fn accept_right(&mut self, cx: &mut Context<Self>) {
        self.accept_everywhere(Side::Theirs, cx);
    }

    fn accept_everywhere(&mut self, side: Side, cx: &mut Context<Self>) {
        for chunk in 0..self.merge.chunks.len() {
            if self.merge.chunks[chunk].is_change() {
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
        let (rope, line_ending) = {
            let buffer = self.result_buffer.read(cx);
            (buffer.as_rope().clone(), buffer.line_ending())
        };
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
            match open_buffer {
                Some(buffer) => {
                    buffer.update(cx, |buffer, cx| {
                        buffer.set_text(rope.to_string(), cx);
                        buffer.set_line_ending(line_ending, cx);
                    });
                    project
                        .update(cx, |project, cx| project.save_buffer(buffer, cx))
                        .await?;
                }
                None => fs.save(&abs_path, &rope, line_ending).await?,
            }
            cx.update(|_, cx| GitCli::mark_resolved(&repo, vec![path], cx))?
                .await?;
            this.update(cx, |_, cx| cx.emit(ItemEvent::CloseItem))?;
            Ok(())
        })
    }

    /// The Apply button: asks first when conflicts are left, shows failures as a toast.
    fn apply_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let unresolved = self.unresolved_count();
        let confirm = (unresolved > 0).then(|| {
            let message = if unresolved == 1 {
                "1 conflict is not resolved".to_string()
            } else {
                format!("{unresolved} conflicts are not resolved")
            };
            window.prompt(
                PromptLevel::Warning,
                &message,
                Some("The result is written as it is now, and the file is marked resolved."),
                &["Apply Anyway", "Cancel"],
                cx,
            )
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Some(confirm) = confirm
                && confirm.await.ok() != Some(0)
            {
                return anyhow::Ok(());
            }
            let task = this.update_in(cx, |this, window, cx| this.apply(window, cx))?;
            if let Err(error) = task.await {
                this.update(cx, |this, cx| this.show_error(&error, cx))?;
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

    fn reanchor(&mut self, snapshot: &TextBufferSnapshot, offsets: Vec<Range<usize>>) {
        let last = offsets.len().saturating_sub(1);
        self.anchors = offsets
            .iter()
            .enumerate()
            .map(|(ix, range)| {
                let end = if ix == last {
                    snapshot.anchor_after(range.end)
                } else {
                    snapshot.anchor_before(range.end)
                };
                snapshot.anchor_before(range.start)..end
            })
            .collect();
        self.synced = snapshot.clone();
        self.synced_offsets = offsets;
    }

    fn replace_chunk(&mut self, chunk: usize, side: Side, cx: &mut Context<Self>) {
        if chunk >= self.merge.chunks.len() {
            return;
        }
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
        let new_offsets = self.chunk_offsets(&snapshot);
        let mut changed = false;
        for (ix, (old, new)) in self.synced_offsets.iter().zip(&new_offsets).enumerate() {
            let touched = edits
                .iter()
                .any(|edit| touches(&edit.old, old) || touches(&edit.new, new));
            if touched && self.resolutions[ix] != Resolution::Edited {
                self.resolutions[ix] = Resolution::Edited;
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

/// Whether an edit range (in one version of the buffer) touches a chunk's range (in the same
/// version). Ranges with content must overlap; an empty range touches what it is next to.
fn touches(edit: &Range<usize>, chunk: &Range<usize>) -> bool {
    if edit.is_empty() && chunk.is_empty() {
        false
    } else if edit.is_empty() || chunk.is_empty() {
        // Only an empty chunk takes in what happens at its position; a pure deletion is
        // judged by its old range.
        chunk.is_empty() && edit.start <= chunk.end && chunk.start <= edit.end
    } else {
        edit.start < chunk.end && chunk.start < edit.end
    }
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

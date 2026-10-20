//! Line-based three-way merge (diff3) for the merge editor.
//!
//! [`merge3`] splits base, ours and theirs into [`Chunk`]s that tile all three texts: each
//! chunk is a line range in every version plus a [`ChunkKind`]. The chunk boundaries follow
//! git's content merge as `git merge`, `rebase` and `cherry-pick` run it (merge-ort: histogram
//! diff, `XDL_MERGE_ZEALOUS_ALNUM`, merge style), which is `git merge-file
//! --diff-algorithm=histogram`: a merge without conflicts produces `git merge-file -p
//! --diff-algorithm=histogram ours base theirs`, and the conflicts are the ones git marked in
//! the working tree file. The histogram diff is a port of git's (`merge3_histogram.rs`); the one
//! known source of difference is imara-diff's Myers, which git's histogram falls back to for
//! very repetitive ranges (and `git merge-file` uses by default): it and git's can choose
//! different, equally short, edit scripts when lines repeat (rare; `merge3_tests.rs` measures
//! it). The merge logic on top is a port of git's.
//!
//! [`MergeResult`] is the result buffer's model: it starts from [`Merge3::auto_merged_result`]
//! (non-conflicting changes applied, conflicts showing the base text) and tracks how each chunk
//! was resolved; [`MergeResult::apply_choice`] returns the text edit to mirror into an editor.
//!
//! Lines keep their own terminators (`\n` or `\r\n`), so CRLF files stay CRLF and the
//! trailing-newline state follows whichever side's last line is taken.

use imara_diff::{Algorithm, Diff, Interner, Token};
use std::ops::Range;

#[path = "merge3_histogram.rs"]
mod histogram;

/// One of the three inputs of a merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Version {
    Base,
    Ours,
    Theirs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChunkKind {
    /// The same lines in all three versions.
    Unchanged,
    /// Only ours changed these lines (theirs equals base); the merge takes ours.
    Ours,
    /// Only theirs changed these lines (ours equals base); the merge takes theirs.
    Theirs,
    /// Both sides made the same change; the merge takes it.
    BothSame,
    /// The sides changed these lines differently. Not resolved automatically.
    Conflict,
}

/// A run of lines, as half-open line ranges into base, ours and theirs. Consecutive chunks are
/// contiguous in every version, and together they cover all lines of all three texts. Only
/// `Unchanged` chunks are guaranteed non-empty in every version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub base: Range<usize>,
    pub ours: Range<usize>,
    pub theirs: Range<usize>,
    pub kind: ChunkKind,
}

impl Chunk {
    pub fn range(&self, version: Version) -> Range<usize> {
        match version {
            Version::Base => self.base.clone(),
            Version::Ours => self.ours.clone(),
            Version::Theirs => self.theirs.clone(),
        }
    }

    pub fn is_conflict(&self) -> bool {
        self.kind == ChunkKind::Conflict
    }

    /// Whether this chunk differs from base on at least one side.
    pub fn is_change(&self) -> bool {
        self.kind != ChunkKind::Unchanged
    }
}

/// Which side's lines to put into the result for a chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Ours,
    Theirs,
    /// Ours, then theirs.
    Both,
}

/// How a chunk of the result got its current text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resolution {
    /// A conflict still showing its base text.
    Unresolved,
    /// Not a conflict: the merge applied it (or it is unchanged).
    Auto,
    /// The user took ours.
    Ours,
    /// The user took theirs.
    Theirs,
    /// The user took ours followed by theirs.
    Both,
    /// The user dismissed the chunk and kept the result's text as it was.
    Ignored,
    /// The user edited the chunk's lines by hand.
    Edited,
}

impl Resolution {
    pub fn is_resolved(self) -> bool {
        self != Resolution::Unresolved
    }
}

/// A chunk's place in the result text. `MergeResult::chunks()[i]` belongs to `Merge3::chunks[i]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultChunk {
    /// Line range in the result.
    pub lines: Range<usize>,
    pub resolution: Resolution,
}

/// A replacement of one chunk's text in the result, for mirroring into an editor buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultEdit {
    pub chunk: usize,
    /// The chunk's line range before the edit.
    pub old_lines: Range<usize>,
    /// The byte range replaced, in the text before the edit.
    pub old_bytes: Range<usize>,
    pub new_text: String,
    /// The chunk's line range after the edit.
    pub new_lines: Range<usize>,
}

/// A text with its line table. A line includes its terminator; a final line without one counts.
#[derive(Clone, Debug)]
struct Text {
    text: String,
    /// Byte offset where each line starts, plus `text.len()` at the end.
    line_starts: Vec<usize>,
}

impl Text {
    fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            line_starts: line_starts(text),
        }
    }

    fn line_count(&self) -> usize {
        self.line_starts.len() - 1
    }

    fn byte_range(&self, lines: Range<usize>) -> Range<usize> {
        let last = self.line_count();
        let start = self.line_starts[lines.start.min(last)];
        let end = self.line_starts[lines.end.min(last)];
        start..end.max(start)
    }

    fn lines(&self, lines: Range<usize>) -> &str {
        &self.text[self.byte_range(lines)]
    }

    fn line(&self, line: usize) -> &str {
        self.lines(line..line + 1)
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
    if *starts.last().unwrap_or(&0) != text.len() {
        starts.push(text.len());
    }
    starts
}

fn line_count(text: &str) -> usize {
    line_starts(text).len() - 1
}

/// The outcome of a three-way merge: the chunks and the three input texts.
#[derive(Clone, Debug)]
pub struct Merge3 {
    pub chunks: Vec<Chunk>,
    texts: [Text; 3],
    line_ending: &'static str,
}

/// Merges `ours` and `theirs`, both derived from `base`, line by line, as git's merges do
/// (histogram diff).
pub fn merge3(base: &str, ours: &str, theirs: &str) -> Merge3 {
    merge3_with(Algorithm::Histogram, base, ours, theirs)
}

/// [`merge3`] with another two-way diff: `Algorithm::Myers` is `git merge-file`'s default.
pub(crate) fn merge3_with(algorithm: Algorithm, base: &str, ours: &str, theirs: &str) -> Merge3 {
    let texts = [Text::new(base), Text::new(ours), Text::new(theirs)];
    let chunks = compute_chunks(&texts, algorithm);
    let line_ending = [ours, theirs, base]
        .into_iter()
        .find(|text| text.contains('\n'))
        .map_or("\n", dominant_line_ending);
    Merge3 {
        chunks,
        texts,
        line_ending,
    }
}

fn dominant_line_ending(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > lf { "\r\n" } else { "\n" }
}

impl Merge3 {
    fn source(&self, version: Version) -> &Text {
        match version {
            Version::Base => &self.texts[0],
            Version::Ours => &self.texts[1],
            Version::Theirs => &self.texts[2],
        }
    }

    pub fn text(&self, version: Version) -> &str {
        &self.source(version).text
    }

    pub fn line_count(&self, version: Version) -> usize {
        self.source(version).line_count()
    }

    /// The byte range of a line range in one version (clamped to the text).
    pub fn byte_range(&self, version: Version, lines: Range<usize>) -> Range<usize> {
        self.source(version).byte_range(lines)
    }

    pub fn lines_text(&self, version: Version, lines: Range<usize>) -> &str {
        self.source(version).lines(lines)
    }

    /// The text of chunk `chunk` in one version.
    pub fn chunk_text(&self, chunk: usize, version: Version) -> &str {
        self.chunks
            .get(chunk)
            .map_or("", |c| self.lines_text(version, c.range(version)))
    }

    /// The line ending used when lines have to be joined: `\r\n` if most of ours' lines (or
    /// theirs', or base's when ours has none) end that way.
    pub fn line_ending(&self) -> &'static str {
        self.line_ending
    }

    pub fn conflict_count(&self) -> usize {
        self.chunks
            .iter()
            .filter(|chunk| chunk.is_conflict())
            .count()
    }

    pub fn has_conflicts(&self) -> bool {
        self.chunks.iter().any(Chunk::is_conflict)
    }

    /// The merged text when there are no conflicts: equal to `git merge-file -p`.
    pub fn merged_text(&self) -> Option<String> {
        if self.has_conflicts() {
            None
        } else {
            Some(self.auto_merged_result().0)
        }
    }

    /// The text a chunk would have in the result with `side` taken.
    pub fn choice_text(&self, chunk: usize, side: Side) -> String {
        match side {
            Side::Ours => self.chunk_text(chunk, Version::Ours).to_string(),
            Side::Theirs => self.chunk_text(chunk, Version::Theirs).to_string(),
            Side::Both => {
                let mut text = String::new();
                push_piece(
                    &mut text,
                    self.chunk_text(chunk, Version::Ours),
                    self.line_ending,
                );
                push_piece(
                    &mut text,
                    self.chunk_text(chunk, Version::Theirs),
                    self.line_ending,
                );
                text
            }
        }
    }

    /// The text a chunk starts with in the result: the changed side for non-conflicts, base
    /// for conflicts.
    fn initial_text(&self, chunk: usize) -> &str {
        let version = match self.chunks[chunk].kind {
            ChunkKind::Theirs => Version::Theirs,
            ChunkKind::Conflict => Version::Base,
            ChunkKind::Unchanged | ChunkKind::Ours | ChunkKind::BothSame => Version::Ours,
        };
        self.chunk_text(chunk, version)
    }

    /// The result buffer's starting text and each chunk's line range in it: non-conflicting
    /// changes applied, conflicts showing the base text (`Resolution::Unresolved`).
    pub fn auto_merged_result(&self) -> (String, Vec<ResultChunk>) {
        let mut text = String::new();
        let mut chunks = Vec::with_capacity(self.chunks.len());
        for (ix, chunk) in self.chunks.iter().enumerate() {
            let start = line_count(&text);
            push_piece(&mut text, self.initial_text(ix), self.line_ending);
            chunks.push(ResultChunk {
                lines: start..line_count(&text),
                resolution: if chunk.is_conflict() {
                    Resolution::Unresolved
                } else {
                    Resolution::Auto
                },
            });
        }
        (text, chunks)
    }
}

/// Appends `piece`, first ending the text's last line if it has no terminator (a side whose
/// last line had none, followed by more lines). git does the same between conflict sides.
fn push_piece(text: &mut String, piece: &str, line_ending: &str) {
    if piece.is_empty() {
        return;
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push_str(line_ending);
    }
    text.push_str(piece);
}

/// The result buffer's model: its text and how each chunk was resolved.
#[derive(Clone, Debug)]
pub struct MergeResult {
    text: String,
    chunks: Vec<ResultChunk>,
    line_ending: &'static str,
}

impl MergeResult {
    pub fn new(merge: &Merge3) -> Self {
        let (text, chunks) = merge.auto_merged_result();
        Self {
            text,
            chunks,
            line_ending: merge.line_ending,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn chunks(&self) -> &[ResultChunk] {
        &self.chunks
    }

    pub fn unresolved_count(&self) -> usize {
        self.chunks
            .iter()
            .filter(|chunk| !chunk.resolution.is_resolved())
            .count()
    }

    pub fn is_resolved(&self) -> bool {
        self.unresolved_count() == 0
    }

    /// Replaces chunk `chunk`'s lines with `side`'s and marks it resolved that way. Returns the
    /// edit so an editor showing the result can apply the same change; `None` for a chunk
    /// index out of range.
    pub fn apply_choice(&mut self, merge: &Merge3, chunk: usize, side: Side) -> Option<ResultEdit> {
        let old_lines = self.chunks.get(chunk)?.lines.clone();
        let starts = line_starts(&self.text);
        let last = starts.len() - 1;
        let old_bytes = starts[old_lines.start.min(last)]..starts[old_lines.end.min(last)];

        let piece = merge.choice_text(chunk, side);
        let mut new_text = String::new();
        // Keep lines from running together across the chunk's edges.
        let before = &self.text[..old_bytes.start];
        if !piece.is_empty() && !before.is_empty() && !before.ends_with('\n') {
            new_text.push_str(self.line_ending);
        }
        new_text.push_str(&piece);
        let after_empty = old_bytes.end == self.text.len();
        if !piece.is_empty() && !piece.ends_with('\n') && !after_empty {
            new_text.push_str(self.line_ending);
        }

        let new_line_count = line_count(&piece);
        let new_lines = old_lines.start..old_lines.start + new_line_count;
        let delta = new_line_count as isize - old_lines.len() as isize;
        self.text.replace_range(old_bytes.clone(), &new_text);
        if let Some(result_chunk) = self.chunks.get_mut(chunk) {
            result_chunk.lines = new_lines.clone();
            result_chunk.resolution = match side {
                Side::Ours => Resolution::Ours,
                Side::Theirs => Resolution::Theirs,
                Side::Both => Resolution::Both,
            };
        }
        for later in self.chunks.iter_mut().skip(chunk + 1) {
            later.lines = shift(&later.lines, delta);
        }
        Some(ResultEdit {
            chunk,
            old_lines,
            old_bytes,
            new_text,
            new_lines,
        })
    }

    /// Marks a chunk resolved without changing its text (the `x` button).
    pub fn ignore(&mut self, chunk: usize) {
        self.set_resolution(chunk, Resolution::Ignored);
    }

    /// Marks a chunk resolved because the user edited its lines.
    pub fn mark_edited(&mut self, chunk: usize) {
        self.set_resolution(chunk, Resolution::Edited);
    }

    pub fn set_resolution(&mut self, chunk: usize, resolution: Resolution) {
        if let Some(chunk) = self.chunks.get_mut(chunk) {
            chunk.resolution = resolution;
        }
    }
}

fn shift(range: &Range<usize>, delta: isize) -> Range<usize> {
    let move_by = |at: usize| at.saturating_add_signed(delta);
    move_by(range.start)..move_by(range.end)
}

// ---------------------------------------------------------------------------------------------
// The merge itself
// ---------------------------------------------------------------------------------------------

// This is a port of git's `xdl_merge` (xdiff/xmerge.c) at the level and style `git
// merge-file` uses by default (`XDL_MERGE_ZEALOUS_ALNUM`, no diff3 markers):
//
// 1. Two-way line diffs base→ours and base→theirs ([`diff_lines`]: Myers, then xdiff's slider
//    compaction).
// 2. A sweep over both hunk lists. A hunk that ends strictly before the other side's next hunk
//    starts is one-sided. Anything else overlaps, and *touching counts as overlapping*: an
//    edit that ends where the other side's edit begins conflicts even with no line in common,
//    and so do two insertions at the same place. Identical hunks (same base range, same new
//    lines) are a clean "both changed the same way". A new record that touches the previous
//    one in ours or theirs is folded into it (`xdl_append_merge`); folding records of
//    different sides makes a conflict.
// 3. Refinement (`xdl_refine_conflicts`, from `XDL_MERGE_ZEALOUS`): each conflict whose sides
//    are both non-empty is diffed ours-against-theirs; lines both sides agree on leave the
//    conflict, which may split it in several or dissolve it.
// 4. Simplification (`xdl_simplify_non_conflicts`): two conflicts separated by at most three
//    lines, or by lines without any ASCII letter or digit (the `_ALNUM` part), become one.
//
// git keeps no base range for refined conflicts (the merge style does not print base); we
// assign them by mapping ours line positions through the base→ours diff, which keeps the
// chunks a tiling of base.

/// A two-way diff hunk: `before` lines replaced by `after` lines.
#[derive(Clone, Debug)]
struct Hunk2 {
    before: Range<usize>,
    after: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Conflict,
    Ours,
    Theirs,
    /// Both sides have the same lines (git's mode 4 after refinement, or identical hunks).
    Same,
}

/// A record of git's merge list, with ranges in base, ours and theirs.
#[derive(Clone, Debug)]
struct Record {
    mode: Mode,
    base: Range<usize>,
    ours: Range<usize>,
    theirs: Range<usize>,
}

fn compute_chunks(texts: &[Text; 3], algorithm: Algorithm) -> Vec<Chunk> {
    let [base, ours, theirs] = texts;
    let mut interner: Interner<&str> =
        Interner::new(base.line_count() + ours.line_count() + theirs.line_count());
    let [base_tokens, ours_tokens, theirs_tokens] = [base, ours, theirs].map(|text| {
        imara_diff::sources::lines(&text.text)
            .map(|line| interner.intern(line))
            .collect::<Vec<Token>>()
    });
    let num_tokens = interner.num_tokens();

    let ours_hunks = diff_lines(algorithm, &base_tokens, &ours_tokens, num_tokens);
    let theirs_hunks = diff_lines(algorithm, &base_tokens, &theirs_tokens, num_tokens);
    let (records, mut fillers) = sweep(
        &ours_hunks,
        &theirs_hunks,
        ours_tokens.len() as isize - base_tokens.len() as isize,
        theirs_tokens.len() as isize - base_tokens.len() as isize,
        &ours_tokens,
        &theirs_tokens,
    );
    let records = refine_conflicts(
        records,
        &ours_hunks,
        &ours_tokens,
        &theirs_tokens,
        num_tokens,
        algorithm,
        &mut fillers,
    );
    let records = simplify_non_conflicts(records, ours);
    compose(records, fillers, texts)
}

/// A range from git's `(i, chg)` pair. git's positions can go negative (a trailing record's
/// position estimated from the file length, for a hunk the sweep already used); such a
/// record is always folded into the previous one, where only its end matters.
fn range_at(start: isize, len: usize) -> Range<usize> {
    let end = (start + len as isize).max(0) as usize;
    (start.max(0) as usize).min(end)..end
}

/// Step 2: `xdl_do_merge`'s sweep. Returns git's merge list and, separately, the identical
/// hunks git leaves out of it.
fn sweep(
    ours_hunks: &[Hunk2],
    theirs_hunks: &[Hunk2],
    ours_growth: isize,
    theirs_growth: isize,
    ours_tokens: &[Token],
    theirs_tokens: &[Token],
) -> (Vec<Record>, Vec<Record>) {
    let mut records: Vec<Record> = Vec::new();
    let mut same = Vec::new();
    let (mut x1, mut x2) = (0, 0);
    while let (Some(h1), Some(h2)) = (ours_hunks.get(x1), theirs_hunks.get(x2)) {
        if h1.before.end < h2.before.start {
            // Ours only. Theirs is unchanged up to its next hunk, so its position follows from
            // that hunk's offset.
            let theirs_start =
                h2.after.start as isize - h2.before.start as isize + h1.before.start as isize;
            append_merge(
                &mut records,
                Record {
                    mode: Mode::Ours,
                    base: h1.before.clone(),
                    ours: h1.after.clone(),
                    theirs: range_at(theirs_start, h1.before.len()),
                },
            );
            x1 += 1;
            continue;
        }
        if h2.before.end < h1.before.start {
            let ours_start =
                h1.after.start as isize - h1.before.start as isize + h2.before.start as isize;
            append_merge(
                &mut records,
                Record {
                    mode: Mode::Theirs,
                    base: h2.before.clone(),
                    ours: range_at(ours_start, h2.before.len()),
                    theirs: h2.after.clone(),
                },
            );
            x2 += 1;
            continue;
        }
        let identical = h1.before == h2.before
            && ours_tokens[h1.after.clone()] == theirs_tokens[h2.after.clone()];
        if identical {
            same.push(Record {
                mode: Mode::Same,
                base: h1.before.clone(),
                ours: h1.after.clone(),
                theirs: h2.after.clone(),
            });
        } else {
            // The union of the two overlapping (or touching) hunks, in all three versions.
            let off = h1.before.start as isize - h2.before.start as isize;
            let ffo = off + h1.before.len() as isize - h2.before.len() as isize;
            let mut i0 = h1.before.start as isize;
            let mut i1 = h1.after.start as isize;
            let mut i2 = h2.after.start as isize;
            if off > 0 {
                i0 -= off;
                i1 -= off;
            } else {
                i2 += off;
            }
            let mut chg0 = h1.before.end as isize - i0;
            let mut chg1 = h1.after.end as isize - i1;
            let mut chg2 = h2.after.end as isize - i2;
            if ffo < 0 {
                chg0 -= ffo;
                chg1 -= ffo;
            } else {
                chg2 += ffo;
            }
            append_merge(
                &mut records,
                Record {
                    mode: Mode::Conflict,
                    base: range_at(i0, chg0.max(0) as usize),
                    ours: range_at(i1, chg1.max(0) as usize),
                    theirs: range_at(i2, chg2.max(0) as usize),
                },
            );
        }
        let (end1, end2) = (h1.before.end, h2.before.end);
        if end1 >= end2 {
            x2 += 1;
        }
        if end2 >= end1 {
            x1 += 1;
        }
    }
    for h1 in &ours_hunks[x1.min(ours_hunks.len())..] {
        append_merge(
            &mut records,
            Record {
                mode: Mode::Ours,
                base: h1.before.clone(),
                ours: h1.after.clone(),
                theirs: range_at(h1.before.start as isize + theirs_growth, h1.before.len()),
            },
        );
    }
    for h2 in &theirs_hunks[x2.min(theirs_hunks.len())..] {
        append_merge(
            &mut records,
            Record {
                mode: Mode::Theirs,
                base: h2.before.clone(),
                ours: range_at(h2.before.start as isize + ours_growth, h2.before.len()),
                theirs: h2.after.clone(),
            },
        );
    }
    (records, same)
}

/// `xdl_append_merge`: folds a record that touches the previous one (in ours or in theirs)
/// into it; folding different modes makes a conflict.
fn append_merge(records: &mut Vec<Record>, record: Record) {
    if let Some(last) = records.last_mut()
        && (record.ours.start <= last.ours.end || record.theirs.start <= last.theirs.end)
    {
        if record.mode != last.mode {
            last.mode = Mode::Conflict;
        }
        extend(last, &record);
        return;
    }
    records.push(record);
}

/// Extends `record` to end where `with` ends, as git does (`m->chg1 = i1 + chg1 - m->i1`).
/// The end can move back: a conflict's first estimate assumes a side follows base after its
/// hunk, which the side's next hunk, folded in here, corrects.
fn extend(record: &mut Record, with: &Record) {
    record.base.end = with.base.end.max(record.base.start);
    record.ours.end = with.ours.end.max(record.ours.start);
    record.theirs.end = with.theirs.end.max(record.theirs.start);
}

/// Maps a line position in ours to base through the base→ours diff. Positions inside a hunk
/// map to the hunk's base end. Monotone, so it maps a tiling of ours to a tiling of base.
fn ours_to_base(ours_hunks: &[Hunk2], position: usize) -> usize {
    let mut delta: isize = 0;
    for hunk in ours_hunks {
        if position <= hunk.after.start {
            break;
        }
        if position < hunk.after.end {
            return hunk.before.end;
        }
        delta = hunk.before.end as isize - hunk.after.end as isize;
    }
    (position as isize + delta).max(0) as usize
}

/// Step 3: `xdl_refine_conflicts`. The common lines found inside conflicts go to `fillers`:
/// they are not records in git's list, so they do not keep step 4 from joining conflicts.
fn refine_conflicts(
    records: Vec<Record>,
    ours_hunks: &[Hunk2],
    ours_tokens: &[Token],
    theirs_tokens: &[Token],
    num_tokens: u32,
    algorithm: Algorithm,
    fillers: &mut Vec<Record>,
) -> Vec<Record> {
    let mut refined = Vec::with_capacity(records.len());
    for record in records {
        if record.mode != Mode::Conflict || record.ours.is_empty() || record.theirs.is_empty() {
            refined.push(record);
            continue;
        }
        // git refines conflicts with the merge's own diff algorithm.
        let hunks = diff_lines(
            algorithm,
            &ours_tokens[record.ours.clone()],
            &theirs_tokens[record.theirs.clone()],
            num_tokens,
        );
        if hunks.is_empty() {
            refined.push(Record {
                mode: Mode::Same,
                ..record
            });
            continue;
        }
        // Common lines (possibly none) alternate with conflicts.
        let mut pieces: Vec<(Mode, Range<usize>, Range<usize>)> = Vec::new();
        let (mut ours_at, mut theirs_at) = (record.ours.start, record.theirs.start);
        for hunk in &hunks {
            let ours_range =
                record.ours.start + hunk.before.start..record.ours.start + hunk.before.end;
            let theirs_range =
                record.theirs.start + hunk.after.start..record.theirs.start + hunk.after.end;
            if ours_at < ours_range.start {
                pieces.push((
                    Mode::Same,
                    ours_at..ours_range.start,
                    theirs_at..theirs_range.start,
                ));
            }
            ours_at = ours_range.end;
            theirs_at = theirs_range.end;
            pieces.push((Mode::Conflict, ours_range, theirs_range));
        }
        if ours_at < record.ours.end {
            pieces.push((
                Mode::Same,
                ours_at..record.ours.end,
                theirs_at..record.theirs.end,
            ));
        }
        let mut base_at = record.base.start;
        let last = pieces.len().saturating_sub(1);
        for (ix, (mode, ours_range, theirs_range)) in pieces.into_iter().enumerate() {
            let base_end = if ix == last {
                record.base.end
            } else {
                ours_to_base(ours_hunks, ours_range.end).clamp(base_at, record.base.end)
            };
            let piece = Record {
                mode,
                base: base_at..base_end,
                ours: ours_range,
                theirs: theirs_range,
            };
            base_at = base_end;
            if mode == Mode::Conflict {
                refined.push(piece);
            } else {
                fillers.push(piece);
            }
        }
    }
    refined
}

/// Step 4: `xdl_simplify_non_conflicts` with `simplify_if_no_alnum`.
fn simplify_non_conflicts(records: Vec<Record>, ours: &Text) -> Vec<Record> {
    let mut simplified: Vec<Record> = Vec::with_capacity(records.len());
    for record in records {
        if let Some(last) = simplified.last_mut()
            && last.mode == Mode::Conflict
            && record.mode == Mode::Conflict
        {
            let between = last.ours.end..record.ours.start.max(last.ours.end);
            let has_alnum = between
                .clone()
                .any(|line| ours.line(line).bytes().any(|b| b.is_ascii_alphanumeric()));
            if between.len() <= 3 || !has_alnum {
                extend(last, &record);
                continue;
            }
        }
        simplified.push(record);
    }
    simplified
}

/// Puts git's list and the fillers in order, drops fillers that step 4 swallowed into a
/// conflict, and fills the gaps with unchanged chunks.
fn compose(records: Vec<Record>, fillers: Vec<Record>, texts: &[Text; 3]) -> Vec<Chunk> {
    let contains = |outer: &Range<usize>, inner: &Range<usize>| {
        outer.start <= inner.start && inner.end <= outer.end
    };
    let swallowed = |filler: &Record| {
        records.iter().any(|record| {
            record.mode == Mode::Conflict
                && contains(&record.base, &filler.base)
                && contains(&record.ours, &filler.ours)
                && contains(&record.theirs, &filler.theirs)
        })
    };
    let fillers: Vec<Record> = fillers
        .into_iter()
        .filter(|filler| !swallowed(filler))
        .collect();
    let mut all = records;
    all.extend(fillers);
    // Every record is non-empty in some version and positions only grow along the chunk
    // sequence, so the sum of the starts orders the records.
    all.sort_by_key(|record| record.base.start + record.ours.start + record.theirs.start);

    let [base, ours, theirs] = texts;
    let ends = [base.line_count(), ours.line_count(), theirs.line_count()];
    let mut chunks = Vec::with_capacity(all.len() * 2 + 1);
    let mut at = [0usize; 3];
    let end_marker = Record {
        mode: Mode::Same,
        base: ends[0]..ends[0],
        ours: ends[1]..ends[1],
        theirs: ends[2]..ends[2],
    };
    for record in all.into_iter().chain(std::iter::once(end_marker)) {
        let starts = [record.base.start, record.ours.start, record.theirs.start];
        let gaps = [0, 1, 2].map(|side| starts[side].saturating_sub(at[side]));
        if gaps.iter().any(|gap| *gap > 0) {
            let consistent = (0..3).all(|side| starts[side] >= at[side])
                && gaps[0] == gaps[1]
                && gaps[1] == gaps[2];
            chunks.push(Chunk {
                base: at[0]..at[0] + gaps[0],
                ours: at[1]..at[1] + gaps[1],
                theirs: at[2]..at[2] + gaps[2],
                // An inconsistent gap cannot come out of a correct merge list; never let one
                // merge silently.
                kind: if consistent {
                    ChunkKind::Unchanged
                } else {
                    ChunkKind::Conflict
                },
            });
            at = [0, 1, 2].map(|side| at[side] + gaps[side]);
        }
        let clamp = |range: &Range<usize>, side: usize| {
            let start = range.start.max(at[side]);
            start..range.end.max(start)
        };
        let base_range = clamp(&record.base, 0);
        let ours_range = clamp(&record.ours, 1);
        let theirs_range = clamp(&record.theirs, 2);
        if base_range.is_empty() && ours_range.is_empty() && theirs_range.is_empty() {
            continue;
        }
        let same_sides = ours.lines(ours_range.clone()) == theirs.lines(theirs_range.clone());
        let kind = match record.mode {
            Mode::Conflict => ChunkKind::Conflict,
            _ if same_sides => ChunkKind::BothSame,
            Mode::Ours => ChunkKind::Ours,
            Mode::Theirs => ChunkKind::Theirs,
            // `Same` records always have equal sides; anything else is not safe to merge.
            Mode::Same => ChunkKind::Conflict,
        };
        at = [base_range.end, ours_range.end, theirs_range.end];
        chunks.push(Chunk {
            base: base_range,
            ours: ours_range,
            theirs: theirs_range,
            kind,
        });
    }
    chunks
}

/// A two-way line diff as git's xdiff computes it for a merge: `algorithm` (a port of xdiff's
/// histogram, or imara-diff's port of xdiff's Myers), then xdiff's slider compaction (`xdl_change_compact`
/// without the indent heuristic, which xdiff runs after every algorithm), applied to `before`
/// and then to `after` like `xdl_merge` does. imara-diff's own `postprocess_*` passes differ
/// from xdiff's in where they leave ambiguous hunks.
fn diff_lines(
    algorithm: Algorithm,
    before: &[Token],
    after: &[Token],
    num_tokens: u32,
) -> Vec<Hunk2> {
    let (mut removed, mut added) = if algorithm == Algorithm::Histogram {
        let mut removed = vec![false; before.len()];
        let mut added = vec![false; after.len()];
        histogram::diff(before, after, &mut removed, &mut added);
        (removed, added)
    } else {
        let mut diff = Diff::default();
        diff.compute_with(algorithm, before, after, num_tokens);
        let removed: Vec<bool> = (0..before.len() as u32)
            .map(|ix| diff.is_removed(ix))
            .collect();
        let added: Vec<bool> = (0..after.len() as u32)
            .map(|ix| diff.is_added(ix))
            .collect();
        (removed, added)
    };
    change_compact(&mut removed, before, &added);
    change_compact(&mut added, after, &removed);

    let mut hunks = Vec::new();
    let (mut b, mut a) = (0, 0);
    let is_set = |flags: &[bool], ix: usize| flags.get(ix).copied().unwrap_or(false);
    while b < removed.len() || a < added.len() {
        if is_set(&removed, b) || is_set(&added, a) {
            let (b_start, a_start) = (b, a);
            while is_set(&removed, b) {
                b += 1;
            }
            while is_set(&added, a) {
                a += 1;
            }
            hunks.push(Hunk2 {
                before: b_start..b,
                after: a_start..a,
            });
        } else {
            b += 1;
            a += 1;
        }
    }
    hunks
}

/// A run of changed lines in one file of a two-way diff (`struct xdlgroup`); empty between
/// two unchanged lines.
#[derive(Clone, Copy)]
struct Group {
    start: usize,
    end: usize,
}

/// One file's changed-line flags, with xdiff's group navigation.
struct Changes<'a> {
    changed: &'a mut [bool],
    lines: &'a [Token],
}

impl Changes<'_> {
    fn is_changed(&self, line: usize) -> bool {
        self.changed.get(line).copied().unwrap_or(false)
    }

    fn first_group(&self) -> Group {
        let mut end = 0;
        while self.is_changed(end) {
            end += 1;
        }
        Group { start: 0, end }
    }

    fn next_group(&self, group: &mut Group) -> bool {
        if group.end >= self.changed.len() {
            return false;
        }
        group.start = group.end + 1;
        group.end = group.start;
        while self.is_changed(group.end) {
            group.end += 1;
        }
        true
    }

    fn previous_group(&self, group: &mut Group) -> bool {
        if group.start == 0 {
            return false;
        }
        group.end = group.start - 1;
        group.start = group.end;
        while group.start > 0 && self.is_changed(group.start - 1) {
            group.start -= 1;
        }
        true
    }

    fn slide_down(&mut self, group: &mut Group) -> bool {
        if group.end < self.changed.len() && self.lines[group.start] == self.lines[group.end] {
            self.changed[group.start] = false;
            self.changed[group.end] = true;
            group.start += 1;
            group.end += 1;
            while self.is_changed(group.end) {
                group.end += 1;
            }
            true
        } else {
            false
        }
    }

    fn slide_up(&mut self, group: &mut Group) -> bool {
        if group.start > 0 && self.lines[group.start - 1] == self.lines[group.end - 1] {
            group.start -= 1;
            group.end -= 1;
            self.changed[group.start] = true;
            self.changed[group.end] = false;
            while group.start > 0 && self.is_changed(group.start - 1) {
                group.start -= 1;
            }
            true
        } else {
            false
        }
    }
}

/// `xdl_change_compact` without `XDF_INDENT_HEURISTIC`: slides each group of changed lines up
/// and down as far as it goes (merging the groups it runs into), then leaves it at the lowest
/// position, or at the last one aligned with a change in the other file.
fn change_compact(changed: &mut [bool], lines: &[Token], other_changed: &[bool]) {
    let mut other_flags = other_changed.to_vec();
    let mut file = Changes { changed, lines };
    let other = Changes {
        changed: &mut other_flags,
        lines: &[],
    };
    let mut group = file.first_group();
    let mut other_group = other.first_group();
    loop {
        if group.end != group.start {
            let mut earliest_end;
            let mut end_matching_other;
            loop {
                let size = group.end - group.start;
                end_matching_other = None;
                while file.slide_up(&mut group) {
                    // The groups of both files stay in step; if they do not, stop compacting
                    // rather than corrupt the diff (xdiff calls this a BUG).
                    if !other.previous_group(&mut other_group) {
                        return;
                    }
                }
                earliest_end = group.end;
                if other_group.end > other_group.start {
                    end_matching_other = Some(group.end);
                }
                while file.slide_down(&mut group) {
                    if !other.next_group(&mut other_group) {
                        return;
                    }
                    if other_group.end > other_group.start {
                        end_matching_other = Some(group.end);
                    }
                }
                if size == group.end - group.start {
                    break;
                }
            }
            if group.end != earliest_end && end_matching_other.is_some() {
                while other_group.end == other_group.start {
                    if !file.slide_up(&mut group) || !other.previous_group(&mut other_group) {
                        return;
                    }
                }
            }
        }
        if !file.next_group(&mut group) || !other.next_group(&mut other_group) {
            break;
        }
    }
}

#[cfg(test)]
#[path = "merge3_tests.rs"]
mod tests;

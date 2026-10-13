use super::*;
use std::collections::HashMap;
use std::path::Path;

use ChunkKind::{BothSame, Conflict, Ours as OursKind, Theirs as TheirsKind, Unchanged};

fn kinds(merge: &Merge3) -> Vec<ChunkKind> {
    merge.chunks.iter().map(|chunk| chunk.kind).collect()
}

fn chunk(base: Range<usize>, ours: Range<usize>, theirs: Range<usize>, kind: ChunkKind) -> Chunk {
    Chunk {
        base,
        ours,
        theirs,
        kind,
    }
}

/// Checks the structural guarantees `MergeView` relies on: the chunks tile all three texts
/// without gaps, and each kind says the truth about its text.
fn check_invariants(merge: &Merge3) {
    let mut cursor = [0usize; 3];
    for (ix, chunk) in merge.chunks.iter().enumerate() {
        for (side, version) in [Version::Base, Version::Ours, Version::Theirs]
            .into_iter()
            .enumerate()
        {
            let range = chunk.range(version);
            assert_eq!(
                range.start, cursor[side],
                "chunk {ix} {version:?} is not contiguous"
            );
            assert!(range.start <= range.end);
            cursor[side] = range.end;
        }
        let base = merge.chunk_text(ix, Version::Base);
        let ours = merge.chunk_text(ix, Version::Ours);
        let theirs = merge.chunk_text(ix, Version::Theirs);
        match chunk.kind {
            Unchanged => {
                assert!(!chunk.base.is_empty(), "empty unchanged chunk {ix}");
                assert_eq!(base, ours);
                assert_eq!(base, theirs);
            }
            OursKind => {
                assert_ne!(ours, base, "chunk {ix}");
                assert_eq!(theirs, base, "chunk {ix}");
            }
            TheirsKind => {
                assert_ne!(theirs, base, "chunk {ix}");
                assert_eq!(ours, base, "chunk {ix}");
            }
            BothSame => assert_eq!(ours, theirs, "chunk {ix}"),
            Conflict => {}
        }
    }
    assert_eq!(cursor[0], merge.line_count(Version::Base));
    assert_eq!(cursor[1], merge.line_count(Version::Ours));
    assert_eq!(cursor[2], merge.line_count(Version::Theirs));
}

fn merge(base: &str, ours: &str, theirs: &str) -> Merge3 {
    let merge = merge3(base, ours, theirs);
    check_invariants(&merge);
    merge
}

// ---------------------------------------------------------------------------------------------
// Table tests
// ---------------------------------------------------------------------------------------------

#[test]
fn identical_sides_are_all_unchanged() {
    let text = "a\nb\nc\n";
    let m = merge(text, text, text);
    assert_eq!(m.chunks, vec![chunk(0..3, 0..3, 0..3, Unchanged)]);
    assert_eq!(m.conflict_count(), 0);
    assert_eq!(m.merged_text().as_deref(), Some(text));

    let empty = merge("", "", "");
    assert!(empty.chunks.is_empty());
    assert_eq!(empty.merged_text().as_deref(), Some(""));
}

#[test]
fn one_sided_changes_take_the_changed_side() {
    let base = "1\n2\n3\n4\n5\n6\n7\n";
    let ours = "1\nOURS\n3\n4\n5\n6\n7\n";
    let theirs = "1\n2\n3\n4\n5\nTHEIRS\n7\n";
    let m = merge(base, ours, theirs);
    assert_eq!(
        m.chunks,
        vec![
            chunk(0..1, 0..1, 0..1, Unchanged),
            chunk(1..2, 1..2, 1..2, OursKind),
            chunk(2..5, 2..5, 2..5, Unchanged),
            chunk(5..6, 5..6, 5..6, TheirsKind),
            chunk(6..7, 6..7, 6..7, Unchanged),
        ]
    );
    assert_eq!(
        m.merged_text().as_deref(),
        Some("1\nOURS\n3\n4\n5\nTHEIRS\n7\n")
    );
    let (text, result_chunks) = m.auto_merged_result();
    assert_eq!(text, "1\nOURS\n3\n4\n5\nTHEIRS\n7\n");
    assert_eq!(result_chunks.len(), m.chunks.len());
    assert!(
        result_chunks
            .iter()
            .all(|chunk| chunk.resolution == Resolution::Auto)
    );
}

#[test]
fn same_change_on_both_sides_is_both_same() {
    let base = "1\n2\n3\n4\n5\n";
    let changed = "1\n2\nNEW\nNEWER\n4\n5\n";
    let m = merge(base, changed, changed);
    assert_eq!(kinds(&m), vec![Unchanged, BothSame, Unchanged]);
    assert_eq!(m.chunks[1], chunk(2..3, 2..4, 2..4, BothSame));
    assert_eq!(m.merged_text().as_deref(), Some(changed));
}

#[test]
fn overlapping_different_changes_conflict_and_start_as_base() {
    let base = "1\n2\n3\n4\n5\n";
    let ours = "1\n2\nOURS\n4\n5\n";
    let theirs = "1\n2\nTHEIRS\n4\n5\n";
    let m = merge(base, ours, theirs);
    assert_eq!(
        m.chunks,
        vec![
            chunk(0..2, 0..2, 0..2, Unchanged),
            chunk(2..3, 2..3, 2..3, Conflict),
            chunk(3..5, 3..5, 3..5, Unchanged),
        ]
    );
    assert_eq!(m.conflict_count(), 1);
    assert!(m.has_conflicts());
    assert_eq!(m.merged_text(), None);
    let (text, result_chunks) = m.auto_merged_result();
    assert_eq!(text, base);
    assert_eq!(result_chunks[1].lines, 2..3);
    assert_eq!(result_chunks[1].resolution, Resolution::Unresolved);
}

#[test]
fn adjacent_edits_conflict_like_git() {
    // Ours edits line 2, theirs edits line 3: no shared line, but git's xdiff merge treats
    // touching hunks as one conflict.
    let base = "1\n2\n3\n4\n";
    let ours = "1\nA\n3\n4\n";
    let theirs = "1\n2\nB\n4\n";
    let m = merge(base, ours, theirs);
    assert_eq!(
        m.chunks,
        vec![
            chunk(0..1, 0..1, 0..1, Unchanged),
            chunk(1..3, 1..3, 1..3, Conflict),
            chunk(3..4, 3..4, 3..4, Unchanged),
        ]
    );
    // An insertion right after a line the other side changed touches it too.
    let m = merge("1\n2\n3\n", "1\nX\n3\n", "1\n2\nY\n3\n");
    assert_eq!(m.conflict_count(), 1);
    // With one unchanged line in between the edits merge cleanly.
    let m = merge("1\n2\n3\n4\n", "A\n2\n3\n4\n", "1\n2\nB\n4\n");
    assert_eq!(kinds(&m), vec![OursKind, Unchanged, TheirsKind, Unchanged]);
    assert_eq!(m.merged_text().as_deref(), Some("A\n2\nB\n4\n"));
}

#[test]
fn insertions_at_start_and_end() {
    let base = "1\n2\n";
    let m = merge(base, "top\n1\n2\n", "1\n2\nbottom\n");
    assert_eq!(
        m.chunks,
        vec![
            chunk(0..0, 0..1, 0..0, OursKind),
            chunk(0..2, 1..3, 0..2, Unchanged),
            chunk(2..2, 3..3, 2..3, TheirsKind),
        ]
    );
    assert_eq!(m.merged_text().as_deref(), Some("top\n1\n2\nbottom\n"));

    // Both sides insert different lines at the same point: a conflict.
    let m = merge(base, "A\n1\n2\n", "B\n1\n2\n");
    assert_eq!(m.chunks[0], chunk(0..0, 0..1, 0..1, Conflict));
    let m = merge(base, "1\n2\nA\n", "1\n2\nB\n");
    assert_eq!(m.chunks[1], chunk(2..2, 2..3, 2..3, Conflict));

    // Into an empty base.
    let m = merge("", "x\n", "");
    assert_eq!(m.chunks, vec![chunk(0..0, 0..1, 0..0, OursKind)]);
    assert_eq!(m.merged_text().as_deref(), Some("x\n"));
}

#[test]
fn deletions() {
    let base = "1\n2\n3\n4\n5\n";
    let m = merge(base, "1\n3\n4\n5\n", "1\n2\n3\n4\n");
    assert_eq!(
        m.chunks,
        vec![
            chunk(0..1, 0..1, 0..1, Unchanged),
            chunk(1..2, 1..1, 1..2, OursKind),
            chunk(2..4, 1..3, 2..4, Unchanged),
            chunk(4..5, 3..4, 4..4, TheirsKind),
        ]
    );
    assert_eq!(m.merged_text().as_deref(), Some("1\n3\n4\n"));

    // Deleting everything on one side.
    let m = merge(base, "", base);
    assert_eq!(m.merged_text().as_deref(), Some(""));

    // One side deletes a line the other side changed: a conflict.
    let m = merge(base, "1\n3\n4\n5\n", "1\nTWO\n3\n4\n5\n");
    assert_eq!(m.chunks[1], chunk(1..2, 1..1, 1..2, Conflict));
}

#[test]
fn crlf_is_preserved() {
    let base = "1\r\n2\r\n3\r\n4\r\n5\r\n";
    let ours = "1\r\nOURS\r\n3\r\n4\r\n5\r\n";
    let theirs = "1\r\n2\r\n3\r\n4\r\nTHEIRS\r\n";
    let m = merge(base, ours, theirs);
    assert_eq!(m.line_ending(), "\r\n");
    assert_eq!(
        m.merged_text().as_deref(),
        Some("1\r\nOURS\r\n3\r\n4\r\nTHEIRS\r\n")
    );
    assert_eq!(merge("a\n", "b\n", "a\n").line_ending(), "\n");
    // A line whose ending changed is a change.
    let m = merge("a\nb\n", "a\r\nb\n", "a\nb\n");
    assert_eq!(kinds(&m), vec![OursKind, Unchanged]);
    assert_eq!(m.merged_text().as_deref(), Some("a\r\nb\n"));
}

#[test]
fn trailing_newline_follows_the_merged_side() {
    // Theirs drops the final newline; ours edits elsewhere.
    let m = merge("1\n2\n3\n4\n", "X\n2\n3\n4\n", "1\n2\n3\n4");
    assert_eq!(m.merged_text().as_deref(), Some("X\n2\n3\n4"));
    // Ours adds a final newline to a file that had none.
    let m = merge("1\n2\n3\n4", "1\n2\n3\n4\n", "1\nY\n3\n4");
    assert_eq!(m.merged_text().as_deref(), Some("1\nY\n3\n4\n"));
    // Unchanged last line without a newline stays without one.
    let m = merge("1\n2\n3\n4", "A\n2\n3\n4", "1\n2\n3\n4");
    assert_eq!(m.merged_text().as_deref(), Some("A\n2\n3\n4"));
}

#[test]
fn refined_conflict_keeps_common_lines_out_of_the_conflict() {
    // Both sides replace the same block; the lines they agree on are not part of a conflict.
    // The two conflicts are separated by more than three lines, so git keeps them apart.
    let base = "0\nold\n9\n";
    let ours = "0\nA\nsame1\nsame2\nsame3\nsame4\nB\n9\n";
    let theirs = "0\nX\nsame1\nsame2\nsame3\nsame4\nY\n9\n";
    let m = merge(base, ours, theirs);
    assert_eq!(
        kinds(&m),
        vec![Unchanged, Conflict, BothSame, Conflict, Unchanged]
    );
    assert_eq!(m.chunks[1].ours, 1..2);
    assert_eq!(m.chunks[2].ours, 2..6);
    assert_eq!(m.chunks[3].ours, 6..7);
    assert_eq!(m.conflict_count(), 2);

    // With three or fewer lines between them, git folds them into one conflict.
    let ours = "0\nA\nsame1\nsame2\nB\n9\n";
    let theirs = "0\nX\nsame1\nsame2\nY\n9\n";
    let m = merge(base, ours, theirs);
    assert_eq!(kinds(&m), vec![Unchanged, Conflict, Unchanged]);
    assert_eq!(m.chunks[1], chunk(1..2, 1..5, 1..5, Conflict));

    // Lines without letters or digits never keep conflicts apart.
    let ours = "0\nA\n}\n}\n\n}\n}\nB\n9\n";
    let theirs = "0\nX\n}\n}\n\n}\n}\nY\n9\n";
    let m = merge(base, ours, theirs);
    assert_eq!(m.conflict_count(), 1);
}

// ---------------------------------------------------------------------------------------------
// Result buffer and per-chunk resolution
// ---------------------------------------------------------------------------------------------

#[test]
fn apply_choice_rewrites_one_chunk_and_shifts_the_rest() {
    let base = "1\n2\n3\n4\n5\n6\n7\n";
    let ours = "1\nO1\nO2\n4\n5\n6\n7\n";
    let theirs = "1\nT1\n3\n4\n5\n6\nT7\n";
    let m = merge(base, ours, theirs);
    assert_eq!(kinds(&m), vec![Unchanged, Conflict, Unchanged, TheirsKind]);
    let mut result = MergeResult::new(&m);
    assert_eq!(result.text(), "1\n2\n3\n4\n5\n6\nT7\n");
    assert_eq!(result.unresolved_count(), 1);
    assert!(!result.is_resolved());

    let edit = result.apply_choice(&m, 1, Side::Ours).expect("chunk");
    assert_eq!(edit.chunk, 1);
    assert_eq!(edit.old_lines, 1..3);
    assert_eq!(edit.old_bytes, 2..6);
    assert_eq!(edit.new_text, "O1\nO2\n");
    assert_eq!(edit.new_lines, 1..3);
    assert_eq!(result.text(), "1\nO1\nO2\n4\n5\n6\nT7\n");
    assert_eq!(result.chunks()[1].resolution, Resolution::Ours);
    assert!(result.is_resolved());

    // Git's conflict covers the union of both hunks, so theirs brings its unchanged "3" along.
    let edit = result.apply_choice(&m, 1, Side::Theirs).expect("chunk");
    assert_eq!(edit.new_text, "T1\n3\n");
    assert_eq!(edit.new_lines, 1..3);
    assert_eq!(result.text(), "1\nT1\n3\n4\n5\n6\nT7\n");
    assert_eq!(result.chunks()[2].lines, 3..6);
    assert_eq!(result.chunks()[3].lines, 6..7);

    let edit = result.apply_choice(&m, 1, Side::Both).expect("chunk");
    assert_eq!(edit.old_bytes, 2..7);
    assert_eq!(result.text(), "1\nO1\nO2\nT1\n3\n4\n5\n6\nT7\n");
    assert_eq!(result.chunks()[1].lines, 1..5);
    assert_eq!(result.chunks()[1].resolution, Resolution::Both);
    assert_eq!(result.chunks()[3].lines, 8..9);

    assert_eq!(result.apply_choice(&m, 9, Side::Ours), None);

    // Taking ours on a theirs-only change reverts it.
    result.apply_choice(&m, 3, Side::Ours).expect("chunk");
    assert_eq!(result.text(), "1\nO1\nO2\nT1\n3\n4\n5\n6\n7\n");

    // The result's text always equals what the edits produce when replayed.
    let mut replay = MergeResult::new(&m).text().to_string();
    let mut fresh = MergeResult::new(&m);
    for (chunk, side) in [(1, Side::Theirs), (3, Side::Both), (1, Side::Both)] {
        let edit = fresh.apply_choice(&m, chunk, side).expect("chunk");
        replay.replace_range(edit.old_bytes.clone(), &edit.new_text);
        assert_eq!(replay, fresh.text());
    }
}

#[test]
fn ignore_and_edit_resolve_without_changing_text() {
    let m = merge("1\n2\n3\n", "1\nA\n3\n", "1\nB\n3\n");
    let mut result = MergeResult::new(&m);
    result.ignore(1);
    assert_eq!(result.chunks()[1].resolution, Resolution::Ignored);
    assert_eq!(result.text(), "1\n2\n3\n");
    assert!(result.is_resolved());

    let mut result = MergeResult::new(&m);
    result.mark_edited(1);
    assert_eq!(result.chunks()[1].resolution, Resolution::Edited);
    assert_eq!(result.unresolved_count(), 0);
}

#[test]
fn taking_both_inserts_a_missing_newline() {
    // Both sides end without a newline: taking both must not glue two lines together.
    let m = merge("1\n2", "1\nA", "1\nB");
    assert_eq!(m.chunks[1].kind, Conflict);
    let mut result = MergeResult::new(&m);
    result.apply_choice(&m, 1, Side::Both).expect("chunk");
    assert_eq!(result.text(), "1\nA\nB");
    assert_eq!(result.chunks()[1].lines, 1..3);

    let m = merge("1\r\n2", "1\r\nA", "1\r\nB");
    let mut result = MergeResult::new(&m);
    result.apply_choice(&m, 1, Side::Both).expect("chunk");
    assert_eq!(result.text(), "1\r\nA\r\nB");
    assert_eq!(m.choice_text(1, Side::Both), "A\r\nB");
}

#[test]
fn line_and_byte_ranges_map_chunks_to_text() {
    let m = merge("a\nb\nc\n", "a\nB\nc\n", "a\nb\nc\n");
    assert_eq!(m.byte_range(Version::Ours, 1..2), 2..4);
    assert_eq!(m.lines_text(Version::Ours, 1..3), "B\nc\n");
    assert_eq!(m.chunk_text(1, Version::Base), "b\n");
    assert_eq!(m.text(Version::Theirs), "a\nb\nc\n");
    assert_eq!(m.line_count(Version::Base), 3);
    assert_eq!(merge("a\nb", "a\nb", "a\nb").line_count(Version::Ours), 2);
}

// ---------------------------------------------------------------------------------------------
// Equivalence with `git merge-file`
// ---------------------------------------------------------------------------------------------

/// Runs git in `dir` with git's defaults: no user or system config, so no
/// `merge.conflictStyle` or diff settings leak in.
fn run_git(dir: &Path, args: &[&str]) -> std::process::Output {
    let config = dir.join("gitconfig");
    if !config.exists() {
        std::fs::write(&config, "").expect("write config");
    }
    let env = HashMap::from([
        (
            "GIT_CONFIG_GLOBAL".to_string(),
            config.to_string_lossy().into_owned(),
        ),
        ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
    ]);
    smol::block_on(
        util::command::new_command("git")
            .current_dir(dir)
            .envs(&env)
            .args(args)
            .output(),
    )
    .expect("run git")
}

/// `git merge-file -p` on files `ours`, `base` and `theirs` written to `dir`: stdout and the
/// exit status, which is the number of conflicts.
fn git_merge_file(dir: &Path, base: &str, ours: &str, theirs: &str) -> (String, i32) {
    std::fs::write(dir.join("base"), base).expect("write base");
    std::fs::write(dir.join("ours"), ours).expect("write ours");
    std::fs::write(dir.join("theirs"), theirs).expect("write theirs");
    let output = run_git(
        dir,
        &[
            "merge-file",
            "-p",
            "-L",
            "ours",
            "-L",
            "base",
            "-L",
            "theirs",
            "ours",
            "base",
            "theirs",
        ],
    );
    let status = output.status.code().expect("git merge-file exit code");
    assert!(status >= 0, "git merge-file failed: {output:?}");
    (
        String::from_utf8(output.stdout).expect("utf-8 output"),
        status,
    )
}

/// What `git merge-file -p -L ours -L base -L theirs` prints for this merge (LF inputs).
fn render_like_git(merge: &Merge3) -> String {
    fn push_side(out: &mut String, text: &str) {
        out.push_str(text);
        if !text.is_empty() && !text.ends_with('\n') {
            out.push('\n');
        }
    }
    let mut out = String::new();
    for (ix, chunk) in merge.chunks.iter().enumerate() {
        match chunk.kind {
            Conflict => {
                out.push_str("<<<<<<< ours\n");
                push_side(&mut out, merge.chunk_text(ix, Version::Ours));
                out.push_str("=======\n");
                push_side(&mut out, merge.chunk_text(ix, Version::Theirs));
                out.push_str(">>>>>>> theirs\n");
            }
            TheirsKind => out.push_str(merge.chunk_text(ix, Version::Theirs)),
            _ => out.push_str(merge.chunk_text(ix, Version::Ours)),
        }
    }
    out
}

#[test]
fn table_cases_match_git_merge_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let cases: &[(&str, &str, &str)] = &[
        ("1\n2\n3\n4\n", "1\nA\n3\n4\n", "1\n2\nB\n4\n"),
        ("1\n2\n3\n", "1\nX\n3\n", "1\n2\nY\n3\n"),
        (
            "0\nold\n9\n",
            "0\nA\ns1\ns2\ns3\ns4\nB\n9\n",
            "0\nX\ns1\ns2\ns3\ns4\nY\n9\n",
        ),
        (
            "0\nold\n9\n",
            "0\nA\ns1\ns2\nB\n9\n",
            "0\nX\ns1\ns2\nY\n9\n",
        ),
        (
            "0\nold\n9\n",
            "0\nA\n}\n}\n\n}\n}\nB\n9\n",
            "0\nX\n}\n}\n\n}\n}\nY\n9\n",
        ),
        ("1\n2", "1\nA", "1\nB"),
        ("1\n2\n3\n4\n", "X\n2\n3\n4\n", "1\n2\n3\n4"),
        ("1\n2\n3\n4", "1\n2\n3\n4\n", "1\nY\n3\n4"),
        (
            "1\r\n2\r\n3\r\n4\r\n5\r\n",
            "1\r\nO\r\n3\r\n4\r\n5\r\n",
            "1\r\n2\r\n3\r\n4\r\nT\r\n",
        ),
        ("a\nb\nc\n", "a\nX\nY\nc\n", "a\nX\nY\nc\n"),
        ("1\n2\n3\n4\n5\n", "1\n3\n4\n5\n", "1\nTWO\n3\n4\n5\n"),
        ("", "x\n", "y\n"),
        ("a\n", "", "b\n"),
    ];
    for (base, ours, theirs) in cases {
        let (expected, status) = git_merge_file(dir.path(), base, ours, theirs);
        let m = merge(base, ours, theirs);
        assert_eq!(
            m.conflict_count() as i32,
            status,
            "conflicts for {base:?} {ours:?} {theirs:?}"
        );
        if status == 0 {
            assert_eq!(m.merged_text().as_deref(), Some(expected.as_str()));
        } else if !base.contains('\r') {
            assert_eq!(
                render_like_git(&m),
                expected,
                "{base:?} {ours:?} {theirs:?}"
            );
        }
    }
}

/// xorshift64*: deterministic, so a failure reproduces from the printed case number.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Small alphabet so that lines repeat and hunks touch, plus lines without letters (`}` and
/// blank) that git's conflict simplification treats specially.
fn random_line(rng: &mut Rng) -> String {
    const ALPHABET: [&str; 6] = ["a", "b", "c", "d", "}", ""];
    let weights = [4, 4, 3, 2, 1, 1];
    let mut pick = rng.below(weights.iter().sum());
    for (line, weight) in ALPHABET.iter().zip(weights) {
        if pick < weight {
            return (*line).to_string();
        }
        pick -= weight;
    }
    unreachable!()
}

fn join(lines: &[String], final_newline: bool) -> String {
    let mut text = lines.join("\n");
    if final_newline && !lines.is_empty() {
        text.push('\n');
    }
    text
}

fn mutate(rng: &mut Rng, base: &[String]) -> Vec<String> {
    let mut lines = base.to_vec();
    for _ in 0..rng.below(4) {
        let at = rng.below(lines.len() + 1);
        match rng.below(3) {
            0 => lines.insert(at, random_line(rng)),
            1 if at < lines.len() => {
                lines.remove(at);
            }
            _ if at < lines.len() => lines[at] = random_line(rng),
            _ => {}
        }
    }
    lines
}

fn random_case(case: u64) -> (String, String, String) {
    let mut rng = Rng::new(case + 1);
    let base: Vec<String> = (0..rng.below(MAX_RANDOM_LINES + 1))
        .map(|_| random_line(&mut rng))
        .collect();
    let ours = if rng.below(8) == 0 {
        base.clone()
    } else {
        mutate(&mut rng, &base)
    };
    let theirs = mutate(&mut rng, &base);
    // Mostly with a final newline; sometimes without, on any of the three.
    let mut newline = || rng.below(8) != 0;
    (
        join(&base, newline()),
        join(&ours, newline()),
        join(&theirs, newline()),
    )
}

/// The hunks `git diff` finds between two files of `dir` with the diff `git merge-file` uses:
/// Myers, no indent heuristic.
fn git_diff_hunks(dir: &Path, before: &str, after: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let output = run_git(
        dir,
        &[
            "diff",
            "--no-index",
            "--no-color",
            "--no-ext-diff",
            "-U0",
            "--diff-algorithm=myers",
            "--no-indent-heuristic",
            before,
            after,
        ],
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 diff");
    let parse = |field: &str| -> Range<usize> {
        let mut parts = field[1..].split(',');
        let start: usize = parts
            .next()
            .and_then(|n| n.parse().ok())
            .expect("hunk start");
        let len: usize = parts.next().map_or(1, |n| n.parse().expect("hunk length"));
        // `-U0` numbers an empty range by the line before it.
        let start = if len == 0 { start } else { start - 1 };
        start..start + len
    };
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("@@ "))
        .map(|header| {
            let mut fields = header.split(' ');
            let old = parse(fields.next().expect("old range"));
            let new = parse(fields.next().expect("new range"));
            (old, new)
        })
        .collect()
}

fn our_diff_hunks(before: &str, after: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let mut interner: Interner<&str> = Interner::new(64);
    let [before, after] = [before, after].map(|text| {
        imara_diff::sources::lines(text)
            .map(|line| interner.intern(line))
            .collect::<Vec<Token>>()
    });
    diff_lines(&before, &after, interner.num_tokens())
        .into_iter()
        .map(|hunk| (hunk.before, hunk.after))
        .collect()
}

/// Random cases have at most this many base lines.
const MAX_RANDOM_LINES: usize = 14;
const RANDOM_CASES: u64 = 400;

enum Outcome {
    Same,
    /// The two-way diffs differ from git's, so the merges may too (see below).
    DiffTieBreak,
    Different(String),
}

/// Compares one random merge with `git merge-file`, output and conflict count.
///
/// The restriction: imara-diff's Myers and git's xdiff Myers can pick different, equally
/// short, edit scripts when lines repeat (base `a d d } c` to `d c`: delete `a d` and `}`, or
/// `a` and `d }`). xdiff's slider compaction in `diff_lines` removes most such differences, not
/// all: when measured, 3 of 4000 merges of up to 10 lines and 38 of 2000 merges of up to 40
/// lines with up to 12 edits per side went a different way, all through a two-way diff (and
/// none otherwise). The merge on top of the two-way diffs is a port of xdiff's, so when merge3
/// differs from git we ask `git diff` for git's own two-way diffs and only fail if they equal
/// ours, which means the merge logic itself is wrong. Tie-break mismatches are capped at 2%.
fn compare_with_git(dir: &Path, case: u64) -> Outcome {
    let (base, ours, theirs) = random_case(case);
    let (expected, status) = git_merge_file(dir, &base, &ours, &theirs);
    let merge = merge3(&base, &ours, &theirs);
    check_invariants(&merge);
    let actual = match merge.merged_text() {
        Some(text) if status == 0 => text,
        _ => render_like_git(&merge),
    };
    if merge.conflict_count() as i32 == status && actual == expected {
        return Outcome::Same;
    }
    // `git_merge_file` left the three files in `dir`.
    if git_diff_hunks(dir, "base", "ours") != our_diff_hunks(&base, &ours)
        || git_diff_hunks(dir, "base", "theirs") != our_diff_hunks(&base, &theirs)
    {
        return Outcome::DiffTieBreak;
    }
    Outcome::Different(format!(
        "case {case}: base {base:?} ours {ours:?} theirs {theirs:?}\n\
         git ({status} conflicts):\n{expected}\n\
         merge3 ({} conflicts):\n{actual}",
        merge.conflict_count()
    ))
}

#[test]
fn random_merges_match_git_merge_file() {
    let threads = 8;
    let outcomes: Vec<Outcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|thread| {
                scope.spawn(move || {
                    let dir = tempfile::tempdir().expect("temp dir");
                    (thread..RANDOM_CASES)
                        .step_by(threads as usize)
                        .map(|case| compare_with_git(dir.path(), case))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("worker"))
            .collect()
    });
    let tie_breaks = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, Outcome::DiffTieBreak))
        .count();
    let failures: Vec<&String> = outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            Outcome::Different(report) => Some(report),
            _ => None,
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {RANDOM_CASES} merges differ from git merge-file although the two-way diffs \
         agree:\n{}",
        failures.len(),
        failures
            .iter()
            .map(|report| report.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        tie_breaks * 50 <= RANDOM_CASES as usize,
        "{tie_breaks} of {RANDOM_CASES} merges differ through diff tie-breaks"
    );
}

#[test]
fn random_cases_cover_conflicts_and_clean_merges() {
    let (mut clean, mut conflicted) = (0, 0);
    for case in 0..RANDOM_CASES {
        let (base, ours, theirs) = random_case(case);
        if merge3(&base, &ours, &theirs).has_conflicts() {
            conflicted += 1;
        } else {
            clean += 1;
        }
    }
    assert!(clean >= 100, "only {clean} clean merges");
    assert!(conflicted >= 100, "only {conflicted} conflicted merges");
}

#[test]
fn random_choices_keep_the_result_consistent() {
    for case in 0..RANDOM_CASES {
        let (base, ours, theirs) = random_case(case);
        let m = merge3(&base, &ours, &theirs);
        let mut result = MergeResult::new(&m);
        let mut replay = result.text().to_string();
        let mut rng = Rng::new(case + 10_000);
        for _ in 0..6 {
            if m.chunks.is_empty() {
                break;
            }
            let chunk = rng.below(m.chunks.len());
            let side = [Side::Ours, Side::Theirs, Side::Both][rng.below(3)];
            let edit = result.apply_choice(&m, chunk, side).expect("chunk");
            replay.replace_range(edit.old_bytes.clone(), &edit.new_text);
            assert_eq!(replay, result.text(), "case {case}");
            // The chunks tile the result's lines, and the edited chunk shows the chosen text.
            let mut at = 0;
            for result_chunk in result.chunks() {
                assert_eq!(result_chunk.lines.start, at, "case {case}");
                at = result_chunk.lines.end;
            }
            assert_eq!(at, line_count(result.text()), "case {case}");
            let starts = line_starts(result.text());
            let lines = &result.chunks()[chunk].lines;
            let shown = &result.text()[starts[lines.start]..starts[lines.end]];
            let chosen = m.choice_text(chunk, side);
            assert_eq!(
                shown.trim_end_matches(['\r', '\n']),
                chosen.trim_end_matches(['\r', '\n']),
                "case {case}"
            );
        }
    }
}

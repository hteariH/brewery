//! git's histogram diff (`xdiff/xhistogram.c`, which git's merges use), ported line for line,
//! so merge3's two-way diffs choose the same lines git does. imara-diff has a histogram diff
//! of its own, but it was reworked for speed and picks different common lines where several
//! are equally rare, which moves conflict boundaries away from the ones git marked.
//!
//! The original is Copyright (C) 2010, Google Inc. and other copyright owners as documented
//! in JGit's IP log, under the Eclipse Distribution License 1.0 (a BSD-style license). As
//! there, lines are numbered from 1 and 0 means none. Where git falls back to its Myers diff
//! (every common line occurs more than 64 times), imara-diff's Myers runs instead.

use imara_diff::{Algorithm, Diff, Token};
use std::collections::HashMap;

const MAX_CHAIN_LENGTH: u32 = 64;

/// Marks the lines of `before` and `after` that git's histogram diff reports as removed and
/// added. `removed` and `added` must be all `false` and as long as the inputs.
pub(super) fn diff(before: &[Token], after: &[Token], removed: &mut [bool], added: &mut [bool]) {
    let mut env = Env {
        a: before,
        b: after,
        removed,
        added,
    };
    // The whole files: `xdl_prepare_env` trims the common prefix and suffix only for Myers
    // (`xdl_optimize_ctxs`), and the histogram's choice of common lines can differ with them.
    env.histogram_diff(1, before.len(), 1, after.len());
}

/// `struct region`: the longest common run found, inclusive line numbers; 0 when none.
#[derive(Default)]
struct Region {
    begin1: usize,
    end1: usize,
    begin2: usize,
    end2: usize,
}

/// One distinct line of the `before` range (`struct record`): its first occurrence and how
/// often it occurs.
struct Record {
    ptr: usize,
    cnt: u32,
}

/// `struct histindex` for one `find_lcs` call.
struct Index {
    records: HashMap<Token, Record>,
    /// The next occurrence of the same line after each `before` line (`next_ptrs`), 0 at the
    /// last; indexed from `ptr_shift`.
    next_ptrs: Vec<usize>,
    ptr_shift: usize,
    cnt: u32,
    has_common: bool,
}

struct Env<'a> {
    a: &'a [Token],
    b: &'a [Token],
    removed: &'a mut [bool],
    added: &'a mut [bool],
}

impl Env<'_> {
    fn a(&self, line: usize) -> Token {
        self.a[line - 1]
    }

    fn b(&self, line: usize) -> Token {
        self.b[line - 1]
    }

    fn histogram_diff(
        &mut self,
        mut line1: usize,
        mut count1: usize,
        mut line2: usize,
        mut count2: usize,
    ) {
        loop {
            if count1 == 0 && count2 == 0 {
                return;
            }
            if count1 == 0 {
                self.added[line2 - 1..line2 - 1 + count2].fill(true);
                return;
            }
            if count2 == 0 {
                self.removed[line1 - 1..line1 - 1 + count1].fill(true);
                return;
            }
            let mut lcs = Region::default();
            match self.find_lcs(&mut lcs, line1, count1, line2, count2) {
                Lcs::FallBack => {
                    self.classic_diff(line1, count1, line2, count2);
                    return;
                }
                Lcs::Found if lcs.begin1 == 0 && lcs.begin2 == 0 => {
                    self.removed[line1 - 1..line1 - 1 + count1].fill(true);
                    self.added[line2 - 1..line2 - 1 + count2].fill(true);
                    return;
                }
                Lcs::Found => {
                    self.histogram_diff(line1, lcs.begin1 - line1, line2, lcs.begin2 - line2);
                    let end1 = line1 + count1 - 1;
                    let end2 = line2 + count2 - 1;
                    count1 = end1 - lcs.end1;
                    line1 = lcs.end1 + 1;
                    count2 = end2 - lcs.end2;
                    line2 = lcs.end2 + 1;
                }
            }
        }
    }

    /// `fall_back_to_classic_diff`: git's Myers on the range.
    fn classic_diff(&mut self, line1: usize, count1: usize, line2: usize, count2: usize) {
        let before = &self.a[line1 - 1..line1 - 1 + count1];
        let after = &self.b[line2 - 1..line2 - 1 + count2];
        let num_tokens = before
            .iter()
            .chain(after)
            .map(|token| token.0 + 1)
            .max()
            .unwrap_or(0);
        let mut diff = Diff::default();
        diff.compute_with(Algorithm::Myers, before, after, num_tokens);
        for ix in 0..count1 {
            self.removed[line1 - 1 + ix] = diff.is_removed(ix as u32);
        }
        for ix in 0..count2 {
            self.added[line2 - 1 + ix] = diff.is_added(ix as u32);
        }
    }

    fn find_lcs(
        &self,
        lcs: &mut Region,
        line1: usize,
        count1: usize,
        line2: usize,
        count2: usize,
    ) -> Lcs {
        let mut index = Index {
            records: HashMap::new(),
            next_ptrs: vec![0; count1],
            ptr_shift: line1,
            cnt: MAX_CHAIN_LENGTH + 1,
            has_common: false,
        };
        // `scanA`: from the last line up, so each line's first occurrence ends up in its
        // record and `next_ptrs` chains the occurrences in order.
        for ptr in (line1..line1 + count1).rev() {
            let token = self.a(ptr);
            match index.records.get_mut(&token) {
                Some(record) => {
                    index.next_ptrs[ptr - index.ptr_shift] = record.ptr;
                    record.ptr = ptr;
                    record.cnt = record.cnt.saturating_add(1);
                }
                None => {
                    index.records.insert(token, Record { ptr, cnt: 1 });
                }
            }
        }

        let mut b_ptr = line2;
        while b_ptr < line2 + count2 {
            b_ptr = self.try_lcs(&mut index, lcs, b_ptr, line1, count1, line2, count2);
        }
        if index.has_common && MAX_CHAIN_LENGTH < index.cnt {
            Lcs::FallBack
        } else {
            Lcs::Found
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn try_lcs(
        &self,
        index: &mut Index,
        lcs: &mut Region,
        b_ptr: usize,
        line1: usize,
        count1: usize,
        line2: usize,
        count2: usize,
    ) -> usize {
        let mut b_next = b_ptr + 1;
        let end1 = line1 + count1 - 1;
        let end2 = line2 + count2 - 1;
        let Some(record) = index.records.get(&self.b(b_ptr)) else {
            return b_next;
        };
        let (first, record_cnt) = (record.ptr, record.cnt);
        if record_cnt > index.cnt {
            index.has_common = true;
            return b_next;
        }
        index.has_common = true;
        let cnt_at = |index: &Index, ptr: usize| index.records[&self.a(ptr)].cnt;
        let mut a_start = first;
        loop {
            let mut np = index.next_ptrs[a_start - index.ptr_shift];
            let mut b_start = b_ptr;
            let mut a_end = a_start;
            let mut b_end = b_start;
            let mut rc = record_cnt;

            while line1 < a_start && line2 < b_start && self.a(a_start - 1) == self.b(b_start - 1) {
                a_start -= 1;
                b_start -= 1;
                if 1 < rc {
                    rc = rc.min(cnt_at(index, a_start));
                }
            }
            while a_end < end1 && b_end < end2 && self.a(a_end + 1) == self.b(b_end + 1) {
                a_end += 1;
                b_end += 1;
                if 1 < rc {
                    rc = rc.min(cnt_at(index, a_end));
                }
            }

            if b_next <= b_end {
                b_next = b_end + 1;
            }
            if lcs.end1 - lcs.begin1 < a_end - a_start || rc < index.cnt {
                lcs.begin1 = a_start;
                lcs.begin2 = b_start;
                lcs.end1 = a_end;
                lcs.end2 = b_end;
                index.cnt = rc;
            }

            if np == 0 {
                break;
            }
            while np <= a_end {
                np = index.next_ptrs[np - index.ptr_shift];
                if np == 0 {
                    return b_next;
                }
            }
            a_start = np;
        }
        b_next
    }
}

enum Lcs {
    /// The longest common run is in the region (none when it stayed empty).
    Found,
    /// Every common line is too frequent: git uses its Myers diff for the range.
    FallBack,
}

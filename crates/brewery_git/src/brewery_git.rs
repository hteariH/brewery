//! IntelliJ-style git for Brewery: the git operations Zed's git layer lacks (merge, rebase,
//! abort/continue, reflog, show at revision), run on each repository's own job queue.

pub mod cli;

pub use cli::{GitCli, GitOutput, MergeOutcome, Operation, Side};

use gpui::App;

/// Must run after `git_ui::init`: later Brewery git UI replaces hooks that `git_ui` installs.
pub fn init(_cx: &mut App) {}

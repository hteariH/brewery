//! IntelliJ-style git for Brewery: the git operations Zed's git layer lacks (merge, rebase,
//! abort/continue, reflog, show at revision), run on each repository's own job queue, the
//! Branches popup and the Log tool window.

pub mod branches_popup;
pub mod cli;
pub mod log_panel;

pub use branches_popup::BranchesPopup;
pub use cli::{GitCli, GitOutput, MergeOutcome, Operation, Side};
pub use log_panel::LogPanel;

use gpui::App;

/// Must run after `git_ui::init`: it replaces the branch picker builder that `git_ui` installs.
pub fn init(cx: &mut App) {
    branches_popup::init(cx);
    log_panel::init(cx);
}

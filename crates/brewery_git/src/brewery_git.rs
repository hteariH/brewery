//! IntelliJ-style git for Brewery: the git operations Zed's git layer lacks (merge, rebase,
//! abort/continue, reflog, show at revision), run on each repository's own job queue, the
//! Branches popup, the Log tool window, file history and comparing a file or the project with
//! a branch or revision.

pub mod branches_popup;
pub mod cli;
pub mod compare;
pub mod file_history;
pub mod log_filter;
pub mod log_filter_bar;
pub mod log_panel;
pub mod revision_diff_view;

pub use branches_popup::BranchesPopup;
pub use cli::{GitCli, GitOutput, MergeOutcome, Operation, Side};
pub use compare::{CompareTarget, CompareWithRevision};
pub use file_history::{FileHistoryView, ShowFileHistory};
pub use log_filter::LogFilter;
pub use log_filter_bar::LogFilterBar;
pub use log_panel::LogPanel;
pub use revision_diff_view::{NewSide, OldSide, ResolvedSide, RevisionDiffView};

use gpui::App;

/// Must run after `git_ui::init`: it replaces the branch picker builder and the file history
/// opener that `git_ui` installs.
pub fn init(cx: &mut App) {
    branches_popup::init(cx);
    log_panel::init(cx);
    file_history::init(cx);
    compare::init(cx);
}

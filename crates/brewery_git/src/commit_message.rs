//! IntelliJ's Commit shortcut (Ctrl+K / Cmd+K in the JetBrains keymap): one press opens the
//! Commit tool window with the commit message focused, from anywhere in the workspace.

use git_ui::git_panel::{FocusEditor, GitPanel};
use gpui::{App, actions};
use workspace::Workspace;

actions!(
    brewery,
    [
        /// Opens the Commit tool window (the git panel) and focuses the commit message.
        FocusCommitMessage,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &FocusCommitMessage, window, cx| {
            if let Some(panel) = workspace.focus_panel::<GitPanel>(window, cx) {
                panel.update(cx, |panel, cx| panel.focus_editor(&FocusEditor, window, cx));
            }
        });
    })
    .detach();
}

#[cfg(test)]
#[path = "commit_message_tests.rs"]
mod tests;

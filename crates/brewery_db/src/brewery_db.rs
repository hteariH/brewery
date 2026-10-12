//! The Database tool window: the project's data sources, a lazily loaded schema tree per data
//! source, and the dialog that adds or edits a data source. Everything goes through
//! `brewery-daemon` (`db/*` methods).

pub mod data_source_modal;
pub mod panel;
pub mod tree;

pub use data_source_modal::DataSourceModal;
pub use panel::DatabasePanel;

use gpui::{App, actions};
use workspace::Workspace;

actions!(
    brewery,
    [
        /// Shows or hides the Database tool window (data sources and their schemas).
        ToggleDatabasePanel,
        /// Opens an SQL console for the data source selected in the Database tool window.
        NewQueryConsole,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleDatabasePanel, window, cx| {
            workspace.toggle_panel_focus::<DatabasePanel>(window, cx);
        });
    })
    .detach();
}

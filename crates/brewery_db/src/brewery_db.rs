//! The Database tool window: the project's data sources, a lazily loaded schema tree per data
//! source, the dialog that adds or edits a data source, and query consoles (an SQL editor tab
//! with a paged result grid). Everything goes through `brewery-daemon` (`db/*` methods).

pub mod completion;
pub mod console;
pub mod console_store;
pub mod data_source_modal;
pub mod panel;
pub mod result_grid;
pub mod schema_cache;
pub mod sql_context;
pub mod statements;
pub mod tree;

pub use console::QueryConsole;
pub use data_source_modal::DataSourceModal;
pub use panel::DatabasePanel;

use gpui::{App, actions};
use workspace::notifications::NotificationId;
use workspace::{Toast, Workspace};

struct SelectDataSourceToast;

actions!(
    brewery,
    [
        /// Shows or hides the Database tool window (data sources and their schemas).
        ToggleDatabasePanel,
        /// Opens an SQL console for the data source selected in the Database tool window.
        NewQueryConsole,
        /// Runs the selection, or the statement at the cursor, in the query console.
        RunStatement,
        /// Cancels the statement running in the query console.
        CancelStatement,
        /// Copies the selected result cells: one value, or a range as tab-separated rows.
        CopyResultCell,
        /// Selects the result cell above.
        MoveCellUp,
        /// Selects the result cell below.
        MoveCellDown,
        /// Selects the result cell to the left.
        MoveCellLeft,
        /// Selects the result cell to the right.
        MoveCellRight,
        /// Extends the selected result cells up by a row.
        ExtendCellsUp,
        /// Extends the selected result cells down by a row.
        ExtendCellsDown,
        /// Extends the selected result cells left by a column.
        ExtendCellsLeft,
        /// Extends the selected result cells right by a column.
        ExtendCellsRight,
        /// Selects the first result cell of the row.
        MoveCellToRowStart,
        /// Selects the last result cell of the row.
        MoveCellToRowEnd,
        /// Selects the first result cell.
        MoveCellToFirst,
        /// Selects the last fetched result cell.
        MoveCellToLast,
        /// Selects every fetched result cell.
        SelectAllCells,
    ]
);

pub fn init(cx: &mut App) {
    workspace::register_serializable_item::<QueryConsole>(cx);
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleDatabasePanel, window, cx| {
            workspace.toggle_panel_focus::<DatabasePanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &NewQueryConsole, window, cx| {
            let target = workspace.panel::<DatabasePanel>(cx).and_then(|panel| {
                let panel = panel.read(cx);
                let sources = panel.tree().sources();
                let connection = panel
                    .selected_connection()
                    .or_else(|| sources.first().filter(|_| sources.len() == 1))?
                    .clone();
                Some((connection, panel.daemon().cloned()))
            });
            match target {
                Some((connection, daemon)) => {
                    console::open_console(workspace, connection, daemon, None, window, cx);
                }
                None => workspace.show_toast(
                    Toast::new(
                        NotificationId::unique::<SelectDataSourceToast>(),
                        "Select a data source in the Database tool window first",
                    ),
                    cx,
                ),
            }
        });
    })
    .detach();
}

#[cfg(test)]
mod test_support;

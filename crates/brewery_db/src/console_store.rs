//! Where query consoles keep their SQL and how open consoles come back after a restart.
//!
//! Every console is backed by a file under the app data directory,
//! `db_consoles/<data source id>/console.sql` (then `console_2.sql`, ...), written shortly after
//! each edit, as IntelliJ keeps its consoles. Closing a tab keeps the file, and the next console
//! of that data source opens it again. The open consoles of a workspace are stored with the
//! workspace (data source and file), so they are reopened with it.

use brewery_daemon_client::ConnectionInfo;
use collections::HashSet;
use db::{
    query,
    sqlez::{domain::Domain, thread_safe_connection::ThreadSafeConnection},
    sqlez_macros::sql,
};
use gpui::{App, Global};
use std::path::{Path, PathBuf};
use workspace::{ItemId, WorkspaceDb, WorkspaceId};

/// How long after the last edit a console writes its file.
pub const SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// The directory of the console files of data source `connection_id`.
pub fn consoles_dir(connection_id: &str) -> PathBuf {
    // Ids are UUIDs from the daemon; anything else is made safe for a file name.
    let name: String = connection_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    paths::data_dir().join("db_consoles").join(name)
}

/// The `index`th console file of a data source: `console.sql`, `console_2.sql`, ...
pub fn console_file(connection_id: &str, index: usize) -> PathBuf {
    let name = if index <= 1 {
        "console.sql".to_string()
    } else {
        format!("console_{index}.sql")
    };
    consoles_dir(connection_id).join(name)
}

/// The console files open in a tab right now, so two consoles never share one.
#[derive(Default)]
struct OpenConsoleFiles(HashSet<PathBuf>);

impl Global for OpenConsoleFiles {}

/// Takes the first console file of `connection_id` that no open console uses.
pub fn claim_free_file(connection_id: &str, cx: &mut App) -> PathBuf {
    let open = cx.default_global::<OpenConsoleFiles>();
    let file = (1..)
        .map(|index| console_file(connection_id, index))
        .find(|file| !open.0.contains(file))
        .expect("an unused console file name");
    open.0.insert(file.clone());
    file
}

/// Marks `file` as used by an open console (a restored one).
pub fn claim_file(file: &Path, cx: &mut App) {
    cx.default_global::<OpenConsoleFiles>()
        .0
        .insert(file.to_path_buf());
}

pub fn release_file(file: &Path, cx: &mut App) {
    cx.default_global::<OpenConsoleFiles>().0.remove(file);
}

/// What a workspace remembers of an open console.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedConsole {
    pub connection: ConnectionInfo,
    pub file: PathBuf,
}

pub struct QueryConsoleDb(ThreadSafeConnection);

impl Domain for QueryConsoleDb {
    const NAME: &str = stringify!(QueryConsoleDb);

    const MIGRATIONS: &[&str] = &[sql!(
        CREATE TABLE brewery_query_consoles (
            workspace_id INTEGER,
            item_id INTEGER UNIQUE,

            connection TEXT NOT NULL,
            file BLOB NOT NULL,

            PRIMARY KEY(workspace_id, item_id),
            FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
            ON DELETE CASCADE
        ) STRICT;
    )];
}

db::static_connection!(QueryConsoleDb, [WorkspaceDb]);

impl QueryConsoleDb {
    query! {
        async fn save_row(
            item_id: ItemId,
            workspace_id: WorkspaceId,
            connection: String,
            file: PathBuf
        ) -> Result<()> {
            INSERT OR REPLACE INTO brewery_query_consoles(item_id, workspace_id, connection, file)
            VALUES (?, ?, ?, ?)
        }
    }

    query! {
        fn get_row(item_id: ItemId, workspace_id: WorkspaceId) -> Result<Option<(String, PathBuf)>> {
            SELECT connection, file
            FROM brewery_query_consoles
            WHERE item_id = ? AND workspace_id = ?
        }
    }

    pub async fn save_console(
        &self,
        item_id: ItemId,
        workspace_id: WorkspaceId,
        saved: SavedConsole,
    ) -> anyhow::Result<()> {
        let connection = serde_json::to_string(&saved.connection)?;
        self.save_row(item_id, workspace_id, connection, saved.file)
            .await
    }

    pub fn get_console(
        &self,
        item_id: ItemId,
        workspace_id: WorkspaceId,
    ) -> anyhow::Result<Option<SavedConsole>> {
        let Some((connection, file)) = self.get_row(item_id, workspace_id)? else {
            return Ok(None);
        };
        Ok(Some(SavedConsole {
            connection: serde_json::from_str(&connection)?,
            file,
        }))
    }
}

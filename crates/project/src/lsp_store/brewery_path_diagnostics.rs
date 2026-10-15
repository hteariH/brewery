// brewery: read access to the diagnostics a local project keeps per worktree path.

use language::{DiagnosticEntry, PointUtf16, Unclipped};
use lsp::LanguageServerId;

use crate::ProjectPath;
use crate::lsp_store::LspStore;

impl LspStore {
    /// The diagnostics stored for `path`, whether or not a buffer is open for it.
    ///
    /// Language servers may publish diagnostics for paths that can never be opened as a
    /// buffer, such as a project directory (jdtls reports build path problems on the
    /// project folder). Views that only render buffer excerpts need this to show them.
    /// Empty for remote projects, which only receive summaries.
    pub fn worktree_diagnostics_for_path(
        &self,
        path: &ProjectPath,
    ) -> Vec<(LanguageServerId, DiagnosticEntry<Unclipped<PointUtf16>>)> {
        let Some(local) = self.as_local() else {
            return Vec::new();
        };
        local
            .diagnostics
            .get(&path.worktree_id)
            .and_then(|by_path| by_path.get(&path.path))
            .into_iter()
            .flatten()
            .flat_map(|(server_id, entries)| {
                entries.iter().map(|entry| (*server_id, entry.clone()))
            })
            .collect()
    }
}

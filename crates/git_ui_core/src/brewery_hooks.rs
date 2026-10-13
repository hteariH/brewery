//! Brewery: hooks that let a higher-level crate (`brewery_git`) take over git panel actions
//! without the git panel depending on it. Each hook is a global; while it is unset, Zed's own
//! behaviour is unchanged.

use std::rc::Rc;

use git::repository::RepoPath;
use git::status::FileStatus;
use gpui::{App, Entity, Global, WeakEntity, Window};
use project::git_store::Repository;
use workspace::Workspace;

type ConflictOpener =
    dyn Fn(WeakEntity<Workspace>, Entity<Repository>, RepoPath, &mut Window, &mut App);

struct ConflictOpenerGlobal(Rc<ConflictOpener>);

impl Global for ConflictOpenerGlobal {}

/// Installs what opening a conflicted file from the git panel does (Brewery's merge editor).
pub fn set_conflict_opener(
    opener: impl Fn(WeakEntity<Workspace>, Entity<Repository>, RepoPath, &mut Window, &mut App)
    + 'static,
    cx: &mut App,
) {
    cx.set_global(ConflictOpenerGlobal(Rc::new(opener)));
}

/// Called by `GitPanel::open_diff` with the selected entry. Returns `true` when the entry is
/// conflicted and an opener was installed and ran; `false` lets the panel open its diff as
/// usual.
pub fn open_conflict(
    (repo, path, status): (&Entity<Repository>, &RepoPath, FileStatus),
    workspace: &Entity<Workspace>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if !status.is_conflicted() {
        return false;
    }
    let Some(opener) = cx
        .try_global::<ConflictOpenerGlobal>()
        .map(|opener| opener.0.clone())
    else {
        return false;
    };
    opener(
        workspace.downgrade(),
        repo.clone(),
        path.clone(),
        window,
        cx,
    );
    true
}

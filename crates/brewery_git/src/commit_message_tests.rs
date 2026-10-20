use super::*;
use fs::FakeFs;
use git::status::StatusCode;
use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};
use project::Project;
use serde_json::json;
use std::time::Duration;
use util::path;
use workspace::dock::Panel as _;
use workspace::{AppState, MultiWorkspace};

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _ = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        release_channel::init(semver::Version::new(0, 0, 0), cx);
        editor::init(cx);
        language_model::init(cx);
        git_ui::init(cx);
        super::init(cx);
    });
}

struct Fixture {
    workspace: Entity<Workspace>,
    panel: Entity<GitPanel>,
}

async fn workspace_with_git_panel(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(path!("/project"), json!({ ".git": {}, "a.txt": "a\n" }))
        .await;
    // A change, so the panel's change list takes focus when the panel does.
    fs.set_status_for_repo(
        path!("/project/.git").as_ref(),
        &[("a.txt", StatusCode::Modified.worktree())],
    );
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
    let (weak, async_cx) = workspace.update_in(cx, |workspace, window, cx| {
        (workspace.weak_handle(), window.to_async(cx))
    });
    let panel = GitPanel::load(weak, async_cx).await.expect("git panel");
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.add_panel(panel.clone(), window, cx);
    });
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    (Fixture { workspace, panel }, cx)
}

fn message_focused(fixture: &Fixture, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| fixture.panel.read(cx).commit_editor_is_focused(window, cx))
}

fn panel_focused(fixture: &Fixture, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| {
        fixture
            .panel
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    })
}

fn dock_open(fixture: &Fixture, cx: &mut VisualTestContext) -> bool {
    let dock = fixture.workspace.update_in(cx, |workspace, window, cx| {
        let position = fixture.panel.read(cx).position(window, cx);
        workspace.dock_at_position(position).clone()
    });
    dock.read_with(cx, |dock, _| dock.is_open())
}

#[gpui::test]
async fn one_press_opens_the_panel_with_the_message_focused(cx: &mut TestAppContext) {
    init_test(cx);
    let (fixture, cx) = workspace_with_git_panel(cx).await;
    assert!(!dock_open(&fixture, cx));
    assert!(!message_focused(&fixture, cx));

    cx.dispatch_action(FocusCommitMessage);
    cx.run_until_parked();
    assert!(dock_open(&fixture, cx), "the Commit tool window opened");
    assert!(
        message_focused(&fixture, cx),
        "the commit message has focus"
    );

    // Pressing it again keeps the message focused instead of toggling the panel away.
    cx.dispatch_action(FocusCommitMessage);
    cx.run_until_parked();
    assert!(dock_open(&fixture, cx));
    assert!(message_focused(&fixture, cx));
}

#[gpui::test]
async fn from_the_change_list_it_moves_focus_to_the_message(cx: &mut TestAppContext) {
    init_test(cx);
    let (fixture, cx) = workspace_with_git_panel(cx).await;
    fixture.workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_panel::<GitPanel>(window, cx);
    });
    cx.run_until_parked();
    assert!(panel_focused(&fixture, cx));
    assert!(!message_focused(&fixture, cx));

    cx.dispatch_action(FocusCommitMessage);
    cx.run_until_parked();
    assert!(message_focused(&fixture, cx));
}

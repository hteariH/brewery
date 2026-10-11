use super::*;
use gpui::{TestAppContext, VisualTestContext};
use project::Project;
use serde_json::json;
use settings::{BaseKeymap, DEFAULT_KEYMAP_PATH, KeymapFile, SPECIFIC_OVERRIDES_KEYMAP_PATH};
use std::sync::Arc;
use util::path;
use workspace::{AppState, MultiWorkspace};

#[test]
fn test_search_tab_cycles() {
    assert_eq!(SearchTab::Files.next(), SearchTab::Symbols);
    assert_eq!(SearchTab::Symbols.next(), SearchTab::Actions);
    assert_eq!(SearchTab::Actions.next(), SearchTab::Files);

    assert_eq!(SearchTab::Files.previous(), SearchTab::Actions);
    assert_eq!(SearchTab::Actions.previous(), SearchTab::Symbols);
    assert_eq!(SearchTab::Symbols.previous(), SearchTab::Files);

    assert_eq!(SearchTab::Files.label(), "Files");
    assert_eq!(SearchTab::Symbols.label(), "Symbols");
    assert_eq!(SearchTab::Actions.label(), "Actions");
}

fn init_test(cx: &mut TestAppContext) -> Arc<AppState> {
    cx.update(|cx| {
        let state = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        editor::init(cx);
        file_finder::init(cx);
        project_symbols::init(cx);
        command_palette::init(cx);
        super::init(cx);
        state
    })
}

fn active_modal(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> Option<Entity<SearchEverywhereModal>> {
    workspace.update(cx, |workspace, cx| {
        workspace.active_modal::<SearchEverywhereModal>(cx)
    })
}

#[gpui::test]
async fn test_search_everywhere_switches_tabs(cx: &mut TestAppContext) {
    let app_state = init_test(cx);
    app_state
        .fs
        .as_fake()
        .insert_tree(path!("/root"), json!({ "a": { "banana.txt": "" } }))
        .await;
    let project = Project::test(app_state.fs.clone(), [path!("/root").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

    cx.dispatch_action(SearchEverywhere);
    cx.run_until_parked();
    let modal = active_modal(&workspace, cx).expect("Search Everywhere should be open");
    modal.read_with(cx, |modal, _| {
        assert_eq!(modal.active_tab(), SearchTab::Files);
        assert!(modal.child.is_some(), "the file finder should be hosted");
    });

    cx.simulate_input("ban");
    cx.run_until_parked();
    modal.read_with(cx, |modal, cx| assert_eq!(modal.query(cx), "ban"));

    cx.dispatch_action(NextSearchTab);
    cx.run_until_parked();
    let modal = active_modal(&workspace, cx).expect("still open after switching tabs");
    modal.read_with(cx, |modal, cx| {
        assert_eq!(modal.active_tab(), SearchTab::Symbols);
        assert_eq!(modal.query(cx), "ban", "query carried over");
    });

    cx.dispatch_action(NextSearchTab);
    cx.run_until_parked();
    let modal = active_modal(&workspace, cx).expect("still open on the Actions tab");
    modal.read_with(cx, |modal, cx| {
        assert_eq!(modal.active_tab(), SearchTab::Actions);
        assert_eq!(modal.query(cx), "ban");
    });

    cx.dispatch_action(NextSearchTab);
    cx.run_until_parked();
    let modal = active_modal(&workspace, cx).expect("still open back on Files");
    modal.read_with(cx, |modal, cx| {
        assert_eq!(modal.active_tab(), SearchTab::Files);
        assert!(modal.child.is_some());
        assert_eq!(modal.query(cx), "ban");
    });

    cx.dispatch_action(PreviousSearchTab);
    cx.run_until_parked();
    let modal = active_modal(&workspace, cx).expect("still open after going back");
    modal.read_with(cx, |modal, _| {
        assert_eq!(modal.active_tab(), SearchTab::Actions)
    });

    // Cancelling the hosted picker closes the whole popup.
    cx.dispatch_action(menu::Cancel);
    cx.run_until_parked();
    assert!(active_modal(&workspace, cx).is_none());
}

#[gpui::test]
async fn test_tab_keys_switch_tabs_with_jetbrains_keymap(cx: &mut TestAppContext) {
    let app_state = init_test(cx);
    cx.update(|cx| {
        // Other crates' actions are not linked into this test binary, so their
        // bindings fail to load; only the ones resolved here matter.
        let base = BaseKeymap::JetBrains
            .asset_path()
            .expect("JetBrains has a keymap file");
        for path in [DEFAULT_KEYMAP_PATH, base, SPECIFIC_OVERRIDES_KEYMAP_PATH] {
            cx.bind_keys(KeymapFile::load_asset_allow_partial_failure(path, cx).unwrap());
        }
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        assert!(
            keymap
                .bindings_for_action(&SearchEverywhere)
                .any(|binding| binding.keystrokes().len() == 2),
            "Shift Shift should open Search Everywhere"
        );
    });
    app_state
        .fs
        .as_fake()
        .insert_tree(path!("/root"), json!({ "a.txt": "" }))
        .await;
    let project = Project::test(app_state.fs.clone(), [path!("/root").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

    cx.dispatch_action(SearchEverywhere);
    cx.run_until_parked();
    cx.simulate_input("a");
    cx.run_until_parked();

    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    let modal = active_modal(&workspace, cx).expect("tab keeps the popup open");
    modal.read_with(cx, |modal, cx| {
        assert_eq!(modal.active_tab(), SearchTab::Symbols);
        assert_eq!(modal.query(cx), "a", "tab must not edit the query");
    });

    cx.simulate_keystrokes("shift-tab");
    cx.run_until_parked();
    let modal = active_modal(&workspace, cx).expect("shift-tab keeps the popup open");
    modal.read_with(cx, |modal, _| {
        assert_eq!(modal.active_tab(), SearchTab::Files)
    });
}

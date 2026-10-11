use super::*;
use brewery_daemon_client::BuildTask;
use gpui::{TestAppContext, VisualTestContext};
use project::Project;
use serde_json::json;
use std::cell::RefCell;
use std::sync::Arc;
use util::path;
use workspace::{AppState, MultiWorkspace};

fn model(build_tool: BuildTool, root: &Path) -> ProjectModel {
    ProjectModel {
        build_tool,
        root_dir: root.to_string_lossy().into_owned(),
        modules: Vec::new(),
    }
}

fn wrapper(root: &Path, name: &str) -> String {
    root.join(name).to_string_lossy().into_owned()
}

fn args(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

#[test]
fn gradle_without_wrapper_uses_gradle() {
    let dir = tempfile::tempdir().unwrap();
    let model = model(BuildTool::Gradle, dir.path());
    for os_windows in [false, true] {
        assert_eq!(
            task_command(&model, ":", "build", os_windows),
            ("gradle".to_string(), args(&["build"]))
        );
        assert_eq!(
            task_command(&model, ":app", "build", os_windows),
            ("gradle".to_string(), args(&[":app:build"]))
        );
    }
}

#[test]
fn gradle_wrapper_is_picked_per_os() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("gradlew"), "").unwrap();
    std::fs::write(dir.path().join("gradlew.bat"), "").unwrap();
    let model = model(BuildTool::Gradle, dir.path());
    assert_eq!(
        task_command(&model, ":", "build", false),
        (wrapper(dir.path(), "gradlew"), args(&["build"]))
    );
    assert_eq!(
        task_command(&model, ":lib:core", "test", true),
        (
            wrapper(dir.path(), "gradlew.bat"),
            args(&[":lib:core:test"])
        )
    );
}

#[test]
fn gradle_unix_wrapper_alone_is_not_used_on_windows() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("gradlew"), "").unwrap();
    let model = model(BuildTool::Gradle, dir.path());
    assert_eq!(
        task_command(&model, ":", "build", true),
        ("gradle".to_string(), args(&["build"]))
    );
}

#[test]
fn maven_without_wrapper_uses_mvn() {
    let dir = tempfile::tempdir().unwrap();
    let model = model(BuildTool::Maven, dir.path());
    for os_windows in [false, true] {
        assert_eq!(
            task_command(&model, ".", "package", os_windows),
            ("mvn".to_string(), args(&["package"]))
        );
        assert_eq!(
            task_command(&model, "core", "package", os_windows),
            ("mvn".to_string(), args(&["-pl", "core", "package"]))
        );
    }
}

#[test]
fn maven_wrapper_is_picked_per_os() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mvnw"), "").unwrap();
    std::fs::write(dir.path().join("mvnw.cmd"), "").unwrap();
    let model = model(BuildTool::Maven, dir.path());
    assert_eq!(
        task_command(&model, ".", "install", false),
        (wrapper(dir.path(), "mvnw"), args(&["install"]))
    );
    assert_eq!(
        task_command(&model, "core", "package", true),
        (
            wrapper(dir.path(), "mvnw.cmd"),
            args(&["-pl", "core", "package"])
        )
    );
}

#[test]
fn tasks_are_grouped_with_ungrouped_last() {
    let task = |name: &str, group: Option<&str>| BuildTask {
        name: name.to_string(),
        group: group.map(str::to_string),
        description: None,
    };
    let module = BuildModule {
        name: "app".to_string(),
        path: ":app".to_string(),
        tasks: vec![
            task("compileJava", None),
            task("test", Some("verification")),
            task("build", Some("build")),
            task("check", Some("verification")),
            task("assemble", Some("build")),
        ],
    };
    assert_eq!(
        grouped_tasks(&module),
        vec![
            (Some("build"), vec![2, 4]),
            (Some("verification"), vec![1, 3]),
            (None, vec![0]),
        ]
    );
}

fn init_test(cx: &mut TestAppContext) -> Arc<AppState> {
    cx.update(|cx| {
        let state = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        super::init(cx);
        state
    })
}

/// Stands in for the daemon: records the root it was asked for and fails, which is
/// what a machine without a JDK or daemon jar would do.
fn failing_daemon(started: Rc<RefCell<Vec<PathBuf>>>) -> StartDaemon {
    Rc::new(move |root, _cx| {
        started.borrow_mut().push(root);
        async { Err(anyhow::anyhow!("no daemon in tests")) }.boxed_local()
    })
}

async fn workspace_with_panel(
    cx: &mut TestAppContext,
) -> (
    Entity<Workspace>,
    Entity<BuildPanel>,
    Rc<RefCell<Vec<PathBuf>>>,
    &mut VisualTestContext,
) {
    let app_state = init_test(cx);
    app_state
        .fs
        .as_fake()
        .insert_tree(path!("/root"), json!({ "build.gradle": "" }))
        .await;
    let project = Project::test(app_state.fs.clone(), [path!("/root").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
    let started = Rc::new(RefCell::new(Vec::new()));
    let start_daemon = failing_daemon(started.clone());
    let panel = workspace.update_in(cx, |workspace, window, cx| {
        let weak = workspace.weak_handle();
        let project = workspace.project().clone();
        let panel = cx.new(|cx| BuildPanel::new(weak, project, start_daemon, window, cx));
        workspace.add_panel(panel.clone(), window, cx);
        panel
    });
    (workspace, panel, started, cx)
}

#[gpui::test]
async fn toggle_action_opens_panel_in_right_dock(cx: &mut TestAppContext) {
    let (workspace, panel, _, cx) = workspace_with_panel(cx).await;
    assert_eq!(BuildPanel::persistent_name(), "Build Tool Panel");
    cx.update(|window, cx| {
        assert_eq!(panel.read(cx).position(window, cx), DockPosition::Right);
    });
    workspace.read_with(cx, |workspace, cx| {
        assert!(!workspace.right_dock().read(cx).is_open());
    });

    cx.dispatch_action(ToggleBuildPanel);
    cx.run_until_parked();

    workspace.read_with(cx, |workspace, cx| {
        let dock = workspace.right_dock().read(cx);
        assert!(dock.is_open(), "the right dock should be open");
        let visible = dock.visible_panel().expect("a panel should be visible");
        assert_eq!(visible.panel_id(), panel.entity_id());
    });
}

// Regression: `set_active` runs inside `Workspace::toggle_panel_focus`, i.e. while
// the workspace is being updated, so connecting there must not read the workspace.
#[gpui::test]
async fn first_show_connects_for_worktree_root_without_panicking(cx: &mut TestAppContext) {
    let (_, panel, started, cx) = workspace_with_panel(cx).await;

    cx.dispatch_action(ToggleBuildPanel);
    cx.run_until_parked();

    assert_eq!(*started.borrow(), vec![PathBuf::from(path!("/root"))]);
    panel.read_with(cx, |panel, _| {
        let error = panel
            .error
            .as_ref()
            .expect("the failed start should be shown");
        assert_eq!(error.kind, ErrorKind::Connection);
        assert!(
            error.message.contains("no daemon in tests"),
            "{}",
            error.message
        );
        assert!(panel.busy.is_none());
    });

    // Hiding and showing again must not start a second daemon.
    cx.dispatch_action(ToggleBuildPanel);
    cx.dispatch_action(ToggleBuildPanel);
    cx.run_until_parked();
    assert_eq!(started.borrow().len(), 1);
}

#[test]
fn only_the_second_click_activates() {
    assert!(!is_activating_click(1));
    assert!(is_activating_click(2));
    assert!(!is_activating_click(3));
}

#[test]
fn reload_refetches_unless_the_daemon_is_unreachable() {
    assert_eq!(reload_action(true, None), ReloadAction::Refetch);
    assert_eq!(
        reload_action(true, Some(ErrorKind::Import)),
        ReloadAction::Refetch
    );
    assert_eq!(
        reload_action(true, Some(ErrorKind::Connection)),
        ReloadAction::Restart
    );
    assert_eq!(reload_action(false, None), ReloadAction::Restart);
    assert_eq!(
        reload_action(false, Some(ErrorKind::Import)),
        ReloadAction::Restart
    );
}

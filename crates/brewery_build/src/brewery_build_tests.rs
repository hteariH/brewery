use super::*;
use brewery_daemon_client::{
    BuildTask, Daemon, DaemonFactory, RunConfiguration, SpringModel, StartDaemon,
};
use futures::FutureExt as _;
use futures::future::LocalBoxFuture;
use gpui::{TestAppContext, VisualTestContext};
use project::Project;
use serde_json::json;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
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

fn add_panel(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<BuildPanel> {
    workspace.update_in(cx, |workspace, window, cx| {
        let weak = workspace.weak_handle();
        let project = workspace.project().clone();
        let panel = cx.new(|cx| BuildPanel::new(weak, project, window, cx));
        workspace.add_panel(panel.clone(), window, cx);
        panel
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
    cx.update(|_, cx| DaemonFactory::set(start_daemon, cx));
    let panel = add_panel(&workspace, cx);
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

#[gpui::test]
async fn the_panel_moves_to_the_left_dock_and_remembers_it(cx: &mut TestAppContext) {
    let (workspace, panel, _, cx) = workspace_with_panel(cx).await;
    panel.update_in(cx, |panel, window, cx| {
        assert!(panel.position_is_valid(DockPosition::Left));
        assert!(!panel.position_is_valid(DockPosition::Bottom));
        panel.set_position(DockPosition::Left, window, cx);
    });
    cx.run_until_parked();

    workspace.read_with(cx, |workspace, cx| {
        let right = workspace.right_dock().read(cx).panel::<BuildPanel>();
        assert!(right.is_none(), "the panel should have left the right dock");
        let left = workspace.left_dock().read(cx).panel::<BuildPanel>();
        assert_eq!(left.map(|left| left.entity_id()), Some(panel.entity_id()));
    });
    let fs = workspace.read_with(cx, |workspace, cx| {
        workspace.project().read(cx).fs().clone()
    });
    let saved = fs
        .load(paths::settings_file())
        .await
        .expect("the dock side is saved to the settings file");
    let saved: serde_json::Value = settings::parse_json_with_comments(&saved).unwrap();
    assert_eq!(saved["build_panel"]["dock"], json!("left"));
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
        assert_eq!(error.kind, DaemonErrorKind::Connection);
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
        reload_action(true, Some(DaemonErrorKind::Import)),
        ReloadAction::Refetch
    );
    assert_eq!(
        reload_action(true, Some(DaemonErrorKind::Connection)),
        ReloadAction::Restart
    );
    assert_eq!(reload_action(false, None), ReloadAction::Restart);
    assert_eq!(
        reload_action(false, Some(DaemonErrorKind::Import)),
        ReloadAction::Restart
    );
}

type ExitHandlers = Arc<std::sync::Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

/// A daemon that serves an empty model until the test makes it die.
struct FakeDaemon {
    exit_handlers: ExitHandlers,
}

impl Daemon for FakeDaemon {
    fn project_model(&self) -> LocalBoxFuture<'_, Result<ProjectModel>> {
        async { Ok(model(BuildTool::Gradle, Path::new("/root"))) }.boxed_local()
    }

    fn spring_model(&self) -> LocalBoxFuture<'_, Result<SpringModel>> {
        async { Ok(SpringModel::default()) }.boxed_local()
    }

    fn run_configurations(&self) -> LocalBoxFuture<'_, Result<Vec<RunConfiguration>>> {
        async { Ok(Vec::new()) }.boxed_local()
    }

    fn on_project_changed(&self, _: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        Box::new(())
    }

    fn on_spring_changed(&self, _: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        Box::new(())
    }

    fn on_exit(&self, f: Box<dyn FnOnce() + Send>) {
        self.exit_handlers.lock().unwrap().push(f);
    }
}

fn dying_daemon(started: Rc<RefCell<usize>>, exit_handlers: ExitHandlers) -> StartDaemon {
    Rc::new(move |_, _| {
        *started.borrow_mut() += 1;
        let daemon: Rc<dyn Daemon> = Rc::new(FakeDaemon {
            exit_handlers: exit_handlers.clone(),
        });
        async move { Ok(daemon) }.boxed_local()
    })
}

#[gpui::test]
async fn a_dead_daemon_is_restarted_a_few_times_then_reported(cx: &mut TestAppContext) {
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
    let started = Rc::new(RefCell::new(0));
    let exit_handlers: ExitHandlers = Default::default();
    let start_daemon = dying_daemon(started.clone(), exit_handlers.clone());
    cx.update(|_, cx| DaemonFactory::set(start_daemon, cx));
    let panel = add_panel(&workspace, cx);

    cx.dispatch_action(ToggleBuildPanel);
    cx.run_until_parked();
    assert_eq!(*started.borrow(), 1);
    panel.read_with(cx, |panel, _| {
        assert!(panel.model.is_some());
        assert!(panel.error.is_none());
    });

    let kill = |cx: &mut VisualTestContext| {
        let handlers = std::mem::take(&mut *exit_handlers.lock().unwrap());
        assert_eq!(handlers.len(), 1, "exactly the live daemon is watched");
        handlers.into_iter().for_each(|handler| handler());
        cx.run_until_parked();
    };

    for restarts in 1..=3 {
        kill(cx);
        assert_eq!(*started.borrow(), 1 + restarts);
        panel.read_with(cx, |panel, _| {
            assert!(panel.error.is_none());
            assert!(panel.model.is_some());
        });
    }

    kill(cx);
    assert_eq!(*started.borrow(), 4, "the fourth death is not restarted");
    panel.read_with(cx, |panel, _| {
        let error = panel.error.as_ref().expect("the failure should be shown");
        assert_eq!(error.kind, DaemonErrorKind::Connection);
        assert_eq!(error.message, DAEMON_KEEPS_STOPPING);
    });
    panel.read_with(cx, |panel, cx| {
        let daemon = panel.daemon.as_ref().expect("the panel keeps its daemon");
        assert!(!daemon.read(cx).is_ready());
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(
            workspace.notification_ids().len(),
            1,
            "giving up should also raise a notification"
        );
    });

    // Reload is the way out, and it gets a fresh allowance.
    panel.update(cx, |panel, cx| panel.reload(cx));
    cx.run_until_parked();
    assert_eq!(*started.borrow(), 5);
    panel.read_with(cx, |panel, _| assert!(panel.error.is_none()));
    kill(cx);
    assert_eq!(*started.borrow(), 6);
}

#[gpui::test]
async fn the_panel_joins_a_daemon_another_panel_started(cx: &mut TestAppContext) {
    let app_state = init_test(cx);
    app_state
        .fs
        .as_fake()
        .insert_tree(path!("/root"), json!({ "build.gradle": "" }))
        .await;
    let project = Project::test(app_state.fs.clone(), [path!("/root").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
    let started = Rc::new(RefCell::new(0));
    let exit_handlers: ExitHandlers = Default::default();
    cx.update(|_, cx| DaemonFactory::set(dying_daemon(started.clone(), exit_handlers), cx));

    // Stands in for the Services panel, which may be opened first.
    let other = cx.update(|_, cx| ProjectDaemon::for_project(&project, cx));
    cx.run_until_parked();
    assert!(other.read_with(cx, |daemon, _| daemon.is_ready()));

    let panel = add_panel(&workspace, cx);
    cx.dispatch_action(ToggleBuildPanel);
    cx.run_until_parked();

    assert_eq!(*started.borrow(), 1, "one daemon serves both");
    panel.read_with(cx, |panel, _| {
        assert_eq!(
            panel.daemon.as_ref().map(Entity::entity_id),
            Some(other.entity_id())
        );
        assert!(panel.model.is_some(), "a ready daemon is used right away");
        assert!(panel.error.is_none());
    });
}

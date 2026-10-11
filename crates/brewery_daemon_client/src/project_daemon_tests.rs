use super::*;
use crate::protocol::BuildTool;
use fs::FakeFs;
use gpui::TestAppContext;
use serde_json::json;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use util::path;

#[test]
fn restart_policy_allows_three_restarts_per_window() {
    let mut policy = RestartPolicy::default();
    let start = Instant::now();
    assert!(policy.allow(start));
    assert!(policy.allow(start + Duration::from_secs(10)));
    assert!(policy.allow(start + Duration::from_secs(20)));
    assert!(!policy.allow(start + Duration::from_secs(30)));
    // A refused restart is not recorded, so it does not extend the ban.
    assert!(!policy.allow(start + Duration::from_secs(299)));
    // The first restart leaves the window after five minutes.
    assert!(policy.allow(start + Duration::from_secs(300)));
    assert!(!policy.allow(start + Duration::from_secs(301)));
}

#[test]
fn restart_policy_reset_forgets_history() {
    let mut policy = RestartPolicy::default();
    let now = Instant::now();
    for _ in 0..3 {
        assert!(policy.allow(now));
    }
    assert!(!policy.allow(now));
    policy.reset();
    assert!(policy.allow(now));
}

#[test]
fn import_errors_are_told_apart() {
    let import = Error::new(lsp::ResponseError::new(
        lsp::ResponseErrorCode::Other(crate::protocol::BUILD_IMPORT_FAILED.into()),
        "broken build.gradle",
    ));
    assert_eq!(DaemonErrorKind::of(&import), DaemonErrorKind::Import);
    assert_eq!(
        DaemonErrorKind::of(&not_running()),
        DaemonErrorKind::Connection
    );
}

type Handlers = Arc<Mutex<Vec<Box<dyn Fn() + Send>>>>;
type ExitHandlers = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

/// Serves fixed models; the test fires its notifications and kills it by hand.
#[derive(Clone, Default)]
struct Fake {
    started: Rc<RefCell<Vec<PathBuf>>>,
    project_changed: Handlers,
    spring_changed: Handlers,
    exit: ExitHandlers,
}

struct FakeDaemon(Fake);

impl Daemon for FakeDaemon {
    fn project_model(&self) -> LocalBoxFuture<'_, Result<ProjectModel>> {
        async {
            Ok(ProjectModel {
                build_tool: BuildTool::Gradle,
                root_dir: path!("/root").to_string(),
                modules: Vec::new(),
            })
        }
        .boxed_local()
    }

    fn spring_model(&self) -> LocalBoxFuture<'_, Result<SpringModel>> {
        async { Ok(SpringModel::default()) }.boxed_local()
    }

    fn run_configurations(&self) -> LocalBoxFuture<'_, Result<Vec<RunConfiguration>>> {
        async { Ok(Vec::new()) }.boxed_local()
    }

    fn on_project_changed(&self, f: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        self.0.project_changed.lock().unwrap().push(f);
        Box::new(())
    }

    fn on_spring_changed(&self, f: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        self.0.spring_changed.lock().unwrap().push(f);
        Box::new(())
    }

    fn on_exit(&self, f: Box<dyn FnOnce() + Send>) {
        self.0.exit.lock().unwrap().push(f);
    }
}

impl Fake {
    fn factory(&self) -> StartDaemon {
        let fake = self.clone();
        Rc::new(move |root, _| {
            fake.started.borrow_mut().push(root);
            let daemon: Rc<dyn Daemon> = Rc::new(FakeDaemon(fake.clone()));
            async move { Ok(daemon) }.boxed_local()
        })
    }

    fn starts(&self) -> usize {
        self.started.borrow().len()
    }

    fn kill(&self, cx: &mut TestAppContext) {
        let handlers = std::mem::take(&mut *self.exit.lock().unwrap());
        assert_eq!(handlers.len(), 1, "exactly the live daemon is watched");
        handlers.into_iter().for_each(|handler| handler());
        cx.run_until_parked();
    }
}

async fn project(cx: &mut TestAppContext, fake: &Fake) -> Entity<Project> {
    cx.update(|cx| {
        let settings_store = settings::SettingsStore::test(cx);
        cx.set_global(settings_store);
        DaemonFactory::set(fake.factory(), cx);
    });
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(path!("/root"), json!({ "build.gradle": "" }))
        .await;
    Project::test(fs, [path!("/root").as_ref()], cx).await
}

fn record_events(
    daemon: &Entity<ProjectDaemon>,
    cx: &mut TestAppContext,
) -> (Rc<RefCell<Vec<ProjectDaemonEvent>>>, gpui::Subscription) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let subscription = cx.update(|cx| {
        cx.subscribe(daemon, move |_, event: &ProjectDaemonEvent, _| {
            sink.borrow_mut().push(*event)
        })
    });
    (events, subscription)
}

#[gpui::test]
async fn panels_of_one_project_share_one_daemon(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let project = project(cx, &fake).await;

    let first = cx.update(|cx| ProjectDaemon::for_project(&project, cx));
    let second = cx.update(|cx| ProjectDaemon::for_project(&project, cx));
    assert_eq!(first.entity_id(), second.entity_id());
    let (first_events, _first) = record_events(&first, cx);
    let (second_events, _second) = record_events(&second, cx);
    cx.run_until_parked();

    assert_eq!(*fake.started.borrow(), vec![PathBuf::from(path!("/root"))]);
    first.read_with(cx, |daemon, _| assert!(daemon.is_ready()));
    assert_eq!(*first_events.borrow(), vec![ProjectDaemonEvent::Ready]);

    // Both subscribers hear about source and build changes from the one process.
    for handler in fake.spring_changed.lock().unwrap().iter() {
        handler();
    }
    for handler in fake.project_changed.lock().unwrap().iter() {
        handler();
    }
    cx.run_until_parked();
    let expected = vec![
        ProjectDaemonEvent::Ready,
        ProjectDaemonEvent::SpringChanged,
        ProjectDaemonEvent::ProjectChanged,
    ];
    assert_eq!(*first_events.borrow(), expected);
    assert_eq!(*second_events.borrow(), expected);

    let model = first
        .read_with(cx, |daemon, cx| daemon.project_model(cx))
        .await
        .unwrap();
    assert_eq!(model.build_tool, BuildTool::Gradle);
    let spring = second
        .read_with(cx, |daemon, cx| daemon.spring_model(cx))
        .await
        .unwrap();
    assert_eq!(spring, SpringModel::default());
    let configurations = second
        .read_with(cx, |daemon, cx| daemon.run_configurations(cx))
        .await
        .unwrap();
    assert!(configurations.is_empty());
    assert_eq!(fake.starts(), 1);
}

#[gpui::test]
async fn the_daemon_stops_when_nobody_holds_it(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let project = project(cx, &fake).await;

    let daemon = cx.update(|cx| ProjectDaemon::for_project(&project, cx));
    cx.run_until_parked();
    let weak = daemon.downgrade();
    drop(daemon);
    cx.run_until_parked();
    assert!(
        weak.upgrade().is_none(),
        "the registry must not keep it alive"
    );

    let _daemon = cx.update(|cx| ProjectDaemon::for_project(&project, cx));
    cx.run_until_parked();
    assert_eq!(fake.starts(), 2);
}

#[gpui::test]
async fn a_dead_daemon_is_restarted_a_few_times_then_given_up(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let project = project(cx, &fake).await;
    let daemon = cx.update(|cx| ProjectDaemon::for_project(&project, cx));
    let (events, _subscription) = record_events(&daemon, cx);
    cx.run_until_parked();
    assert_eq!(fake.starts(), 1);

    for restarts in 1..=3 {
        fake.kill(cx);
        assert_eq!(fake.starts(), 1 + restarts);
        daemon.read_with(cx, |daemon, _| assert!(daemon.is_ready()));
    }

    events.borrow_mut().clear();
    fake.kill(cx);
    assert_eq!(fake.starts(), 4, "the fourth death is not restarted");
    assert_eq!(
        *events.borrow(),
        vec![ProjectDaemonEvent::Failed { gave_up: true }]
    );
    daemon.read_with(cx, |daemon, _| {
        assert_eq!(
            *daemon.state(),
            DaemonState::Failed {
                error: DAEMON_KEEPS_STOPPING.into(),
                kind: DaemonErrorKind::Connection,
            }
        );
    });
    let error = daemon
        .read_with(cx, |daemon, cx| daemon.project_model(cx))
        .await
        .unwrap_err();
    assert_eq!(DaemonErrorKind::of(&error), DaemonErrorKind::Connection);

    // Reload is the way out, and it gets a fresh allowance.
    daemon.update(cx, |daemon, cx| daemon.reload(cx));
    cx.run_until_parked();
    assert_eq!(fake.starts(), 5);
    daemon.read_with(cx, |daemon, _| assert!(daemon.is_ready()));
    fake.kill(cx);
    assert_eq!(fake.starts(), 6);
}

#[gpui::test]
async fn a_failed_start_is_reported(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let project = project(cx, &fake).await;
    cx.update(|cx| {
        DaemonFactory::set(
            Rc::new(|_, _| async { Err(anyhow!("no JDK")) }.boxed_local()),
            cx,
        )
    });
    let daemon = cx.update(|cx| ProjectDaemon::for_project(&project, cx));
    let (events, _subscription) = record_events(&daemon, cx);
    cx.run_until_parked();
    assert_eq!(
        *events.borrow(),
        vec![ProjectDaemonEvent::Failed { gave_up: false }]
    );
    daemon.read_with(cx, |daemon, _| {
        assert_eq!(
            *daemon.state(),
            DaemonState::Failed {
                error: "no JDK".into(),
                kind: DaemonErrorKind::Connection,
            }
        );
    });
}

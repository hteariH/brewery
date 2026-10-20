use super::*;
use crate::protocol::BuildTool;
use fs::FakeFs;
use gpui::TestAppContext;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
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
    db_calls: Rc<RefCell<Vec<(&'static str, serde_json::Value)>>>,
    db_replies: Rc<
        RefCell<HashMap<&'static str, std::result::Result<serde_json::Value, lsp::ResponseError>>>,
    >,
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

    fn db_request(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> LocalBoxFuture<'_, Result<serde_json::Value>> {
        self.0.db_calls.borrow_mut().push((method, params));
        let reply = self.0.db_replies.borrow().get(method).cloned();
        async move {
            match reply {
                Some(Ok(value)) => Ok(value),
                Some(Err(error)) => Err(Error::new(error).context(format!("{method} failed"))),
                None => Err(anyhow!("no canned reply for {method}")),
            }
        }
        .boxed_local()
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

fn fixture(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap()
}

#[gpui::test]
async fn database_calls_reach_the_daemon_as_json(cx: &mut TestAppContext) {
    use crate::protocol::*;
    let fake = Fake::default();
    {
        let mut replies = fake.db_replies.borrow_mut();
        replies.insert(
            DbDrivers::METHOD,
            Ok(fixture(include_str!(
                "../../../brewery-daemon/protocol/fixtures/db-drivers.result.json"
            ))),
        );
        replies.insert(
            DbConnections::METHOD,
            Ok(serde_json::json!([fixture(include_str!(
                "../../../brewery-daemon/protocol/fixtures/db-connection.json"
            ))])),
        );
        replies.insert(
            DbSaveConnection::METHOD,
            Ok(fixture(include_str!(
                "../../../brewery-daemon/protocol/fixtures/db-connection.json"
            ))),
        );
        replies.insert(
            DbTestConnection::METHOD,
            Ok(fixture(include_str!(
                "../../../brewery-daemon/protocol/fixtures/db-connections-test.result.json"
            ))),
        );
        replies.insert(
            DbIntrospect::METHOD,
            Ok(fixture(include_str!(
                "../../../brewery-daemon/protocol/fixtures/db-introspect.result.json"
            ))),
        );
        replies.insert(
            DbExecute::METHOD,
            Ok(fixture(include_str!(
                "../../../brewery-daemon/protocol/fixtures/db-execute.result.json"
            ))),
        );
        replies.insert(
            DbFetch::METHOD,
            Ok(fixture(include_str!(
                "../../../brewery-daemon/protocol/fixtures/db-fetch.result.json"
            ))),
        );
        replies.insert(
            DbAddDriver::METHOD,
            Ok(serde_json::json!({"id": "h2", "name": "H2", "bundled": false, "jarPaths": ["/h2.jar"]})),
        );
        for method in [
            DbDeleteConnection::METHOD,
            DbCancel::METHOD,
            DbClose::METHOD,
        ] {
            replies.insert(method, Ok(serde_json::Value::Null));
        }
    }
    let project = project(cx, &fake).await;
    let daemon = cx.update(|cx| ProjectDaemon::for_project(&project, cx));
    cx.run_until_parked();

    let drivers = daemon
        .read_with(cx, |d, cx| d.db_drivers(cx))
        .await
        .unwrap();
    assert_eq!(drivers.len(), 4);
    let added = daemon
        .read_with(cx, |d, cx| {
            d.db_add_driver(
                AddDriverParams {
                    driver_id: "h2".into(),
                    name: "H2".into(),
                    class_name: "org.h2.Driver".into(),
                    jar_paths: vec!["/h2.jar".into()],
                },
                cx,
            )
        })
        .await
        .unwrap();
    assert!(!added.bundled);
    let connections = daemon
        .read_with(cx, |d, cx| d.db_connections(cx))
        .await
        .unwrap();
    assert_eq!(connections[0].id, "c1");
    let saved = daemon
        .read_with(cx, |d, cx| {
            d.db_save_connection(
                SaveConnectionParams {
                    connection: connections[0].clone(),
                    password: Some("pw".into()),
                },
                cx,
            )
        })
        .await
        .unwrap();
    assert_eq!(saved.name, "Local Postgres");
    let tested = daemon
        .read_with(cx, |d, cx| {
            d.db_test_connection(
                TestConnectionParams {
                    connection: saved.clone(),
                    password: None,
                },
                cx,
            )
        })
        .await
        .unwrap();
    assert!(tested.ok);
    daemon
        .read_with(cx, |d, cx| d.db_delete_connection("c1".into(), cx))
        .await
        .unwrap();
    let nodes = daemon
        .read_with(cx, |d, cx| {
            d.db_introspect(
                IntrospectParams {
                    connection_id: "c1".into(),
                    path: vec!["public".into()],
                    include_system: None,
                },
                cx,
            )
        })
        .await
        .unwrap();
    assert_eq!(nodes.len(), 7);
    let executed = daemon
        .read_with(cx, |d, cx| {
            d.db_execute(
                ExecuteParams {
                    connection_id: "c1".into(),
                    sql: "select 1".into(),
                    session_id: None,
                },
                cx,
            )
        })
        .await
        .unwrap();
    let cursor_id = executed.cursor_id.unwrap();
    let page = daemon
        .read_with(cx, |d, cx| {
            d.db_fetch(
                FetchParams {
                    cursor_id: cursor_id.clone(),
                    offset: 0,
                    limit: 500,
                },
                cx,
            )
        })
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 4);
    daemon
        .read_with(cx, |d, cx| {
            d.db_cancel(
                CancelParams {
                    connection_id: Some("c1".into()),
                    cursor_id: None,
                    session_id: None,
                },
                cx,
            )
        })
        .await
        .unwrap();
    daemon
        .read_with(cx, |d, cx| d.db_close(cursor_id.clone(), cx))
        .await
        .unwrap();

    let calls = fake.db_calls.borrow();
    let methods: Vec<&str> = calls.iter().map(|(method, _)| *method).collect();
    assert_eq!(
        methods,
        vec![
            "db/drivers",
            "db/drivers/addJar",
            "db/connections/list",
            "db/connections/save",
            "db/connections/test",
            "db/connections/delete",
            "db/introspect",
            "db/execute",
            "db/fetch",
            "db/cancel",
            "db/close",
        ]
    );
    assert_eq!(calls[0].1, serde_json::json!({}));
    assert_eq!(calls[3].1["password"], serde_json::json!("pw"));
    assert_eq!(
        calls[3].1["connection"]["driverId"],
        serde_json::json!("postgresql")
    );
    assert_eq!(calls[5].1, serde_json::json!({"id": "c1"}));
    assert_eq!(
        calls[6].1,
        serde_json::json!({"connectionId": "c1", "path": ["public"]})
    );
    assert_eq!(
        calls[8].1,
        serde_json::json!({"cursorId": cursor_id, "offset": 0, "limit": 500})
    );
    assert_eq!(calls[10].1, serde_json::json!({"cursorId": cursor_id}));
}

#[gpui::test]
async fn the_schema_snapshot_reaches_the_daemon_as_json(cx: &mut TestAppContext) {
    use crate::protocol::*;
    let fake = Fake::default();
    fake.db_replies.borrow_mut().insert(
        DbGetSchema::METHOD,
        Ok(fixture(include_str!(
            "../../../brewery-daemon/protocol/fixtures/db-schema.result.json"
        ))),
    );
    let project = project(cx, &fake).await;
    let daemon = cx.update(|cx| ProjectDaemon::for_project(&project, cx));
    cx.run_until_parked();

    let schema = daemon
        .read_with(cx, |d, cx| {
            d.db_schema(
                SchemaParams {
                    connection_id: "c1".into(),
                    refresh: Some(true),
                },
                cx,
            )
        })
        .await
        .unwrap();
    assert_eq!(schema.schemas[0].tables.len(), 2);
    let calls = fake.db_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "db/schema");
    assert_eq!(
        calls[0].1,
        serde_json::json!({"connectionId": "c1", "refresh": true})
    );
}

#[gpui::test]
async fn a_database_error_keeps_the_daemon_code_and_message(cx: &mut TestAppContext) {
    use crate::protocol::*;
    let fake = Fake::default();
    fake.db_replies.borrow_mut().insert(
        DbExecute::METHOD,
        Err(lsp::ResponseError::new(
            lsp::ResponseErrorCode::Other(DB_QUERY_FAILED.into()),
            "syntax error at or near \"selec\" (SQLState 42601)",
        )),
    );
    let project = project(cx, &fake).await;
    let daemon = cx.update(|cx| ProjectDaemon::for_project(&project, cx));
    cx.run_until_parked();

    let error = daemon
        .read_with(cx, |d, cx| {
            d.db_execute(
                ExecuteParams {
                    connection_id: "c1".into(),
                    sql: "selec".into(),
                    session_id: None,
                },
                cx,
            )
        })
        .await
        .unwrap_err();
    assert_eq!(
        crate::daemon_error_code(&error),
        Some(DB_QUERY_FAILED.into())
    );
    assert_eq!(
        crate::daemon_error_message(&error),
        "syntax error at or near \"selec\" (SQLState 42601)"
    );
    assert!(!crate::is_import_error(&error));

    // No daemon answer at all: no code, the whole chain as the message.
    let transport = anyhow!("Server reset the connection").context("db/execute failed");
    assert_eq!(crate::daemon_error_code(&transport), None);
    assert_eq!(
        crate::daemon_error_message(&transport),
        "db/execute failed: Server reset the connection"
    );
}

#[gpui::test]
async fn database_calls_fail_while_the_daemon_is_not_running(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let project = project(cx, &fake).await;
    let daemon = cx.update(|cx| ProjectDaemon::for_project(&project, cx));
    // Not parked yet: the daemon is still connecting.
    let error = daemon
        .read_with(cx, |d, cx| d.db_drivers(cx))
        .await
        .unwrap_err();
    assert_eq!(crate::daemon_error_code(&error), None);
    assert!(error.to_string().contains("not running"), "{error}");
}

/// A daemon written before the database methods existed keeps the trait's default.
struct NoDbDaemon;

impl Daemon for NoDbDaemon {
    fn project_model(&self) -> LocalBoxFuture<'_, Result<ProjectModel>> {
        async { Err(not_running()) }.boxed_local()
    }
    fn spring_model(&self) -> LocalBoxFuture<'_, Result<SpringModel>> {
        async { Err(not_running()) }.boxed_local()
    }
    fn run_configurations(&self) -> LocalBoxFuture<'_, Result<Vec<RunConfiguration>>> {
        async { Err(not_running()) }.boxed_local()
    }
    fn on_project_changed(&self, _: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        Box::new(())
    }
    fn on_spring_changed(&self, _: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        Box::new(())
    }
    fn on_exit(&self, _: Box<dyn FnOnce() + Send>) {}
}

#[test]
fn the_default_db_request_fails() {
    let error =
        futures::executor::block_on(NoDbDaemon.db_request("db/drivers", serde_json::json!({})))
            .unwrap_err();
    assert!(error.to_string().contains("db/drivers"), "{error}");
}

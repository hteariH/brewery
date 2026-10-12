use super::*;
use crate::data_source_modal::TestState;
use brewery_daemon_client::{
    BuildTool, Daemon, DaemonFactory, ProjectModel, RunConfiguration, SpringModel, StartDaemon,
};
use futures::FutureExt as _;
use futures::future::LocalBoxFuture;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use util::path;
use workspace::{AppState, MultiWorkspace};

type Calls = Rc<RefCell<Vec<(String, Value)>>>;

/// Serves the `db/*` methods from an in-memory list of connections and records every call.
struct FakeDaemon {
    connections: Rc<RefCell<Vec<Value>>>,
    calls: Calls,
}

fn connection(id: &str, name: &str, driver: &str, has_password: bool) -> Value {
    json!({
        "id": id,
        "name": name,
        "driverId": driver,
        "url": format!("jdbc:{driver}://localhost/{id}"),
        "user": "app",
        "hasPassword": has_password,
        "options": {}
    })
}

impl FakeDaemon {
    fn answer(&self, method: &str, params: &Value) -> anyhow::Result<Value> {
        match method {
            "db/connections/list" => Ok(Value::Array(self.connections.borrow().clone())),
            "db/drivers" => Ok(json!([
                { "id": "postgresql", "name": "PostgreSQL", "bundled": true, "jarPaths": [] },
                { "id": "sqlite", "name": "SQLite", "bundled": true, "jarPaths": [] }
            ])),
            "db/connections/save" => {
                let mut saved = params["connection"].clone();
                if saved["id"] == "" {
                    saved["id"] = json!("c3");
                }
                let password = params["password"].as_str();
                if let Some(password) = password {
                    saved["hasPassword"] = json!(!password.is_empty());
                }
                let mut connections = self.connections.borrow_mut();
                connections.retain(|other| other["id"] != saved["id"]);
                connections.push(saved.clone());
                saved["warning"] = json!("No secret store: the password was not saved");
                Ok(saved)
            }
            "db/connections/delete" => {
                self.connections
                    .borrow_mut()
                    .retain(|other| other["id"] != params["id"]);
                Ok(Value::Null)
            }
            "db/connections/test" => Ok(json!({
                "ok": true,
                "message": "Connected",
                "serverVersion": "PostgreSQL 16.4"
            })),
            "db/introspect" => {
                if params["connectionId"] == "c2" {
                    anyhow::bail!("connection refused");
                }
                let path: Vec<&str> = params["path"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                Ok(match path.as_slice() {
                    [] => json!([
                        { "name": "public", "kind": "schema", "path": ["public"], "hasChildren": true }
                    ]),
                    ["public"] => json!([
                        { "name": "owners", "kind": "table", "path": ["public", "owners"], "hasChildren": true },
                        { "name": "owners_id_seq", "kind": "sequence", "path": ["public", "owners_id_seq"], "hasChildren": false, "detail": "integer" }
                    ]),
                    _ => json!([]),
                })
            }
            other => anyhow::bail!("unexpected {other}"),
        }
    }
}

impl Daemon for FakeDaemon {
    fn project_model(&self) -> LocalBoxFuture<'_, anyhow::Result<ProjectModel>> {
        async {
            Ok(ProjectModel {
                build_tool: BuildTool::Gradle,
                root_dir: "/root".to_string(),
                modules: Vec::new(),
            })
        }
        .boxed_local()
    }

    fn spring_model(&self) -> LocalBoxFuture<'_, anyhow::Result<SpringModel>> {
        async { Ok(SpringModel::default()) }.boxed_local()
    }

    fn run_configurations(&self) -> LocalBoxFuture<'_, anyhow::Result<Vec<RunConfiguration>>> {
        async { Ok(Vec::new()) }.boxed_local()
    }

    fn db_request(
        &self,
        method: &'static str,
        params: Value,
    ) -> LocalBoxFuture<'_, anyhow::Result<Value>> {
        self.calls
            .borrow_mut()
            .push((method.to_string(), params.clone()));
        let answer = self.answer(method, &params);
        async move { answer }.boxed_local()
    }

    fn on_project_changed(&self, _: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        Box::new(())
    }

    fn on_spring_changed(&self, _: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        Box::new(())
    }

    fn on_exit(&self, _: Box<dyn FnOnce() + Send>) {}
}

struct Fixture {
    workspace: Entity<Workspace>,
    panel: Entity<DatabasePanel>,
    calls: Calls,
}

impl Fixture {
    fn calls_of(&self, method: &str) -> Vec<Value> {
        self.calls
            .borrow()
            .iter()
            .filter(|(called, _)| called == method)
            .map(|(_, params)| params.clone())
            .collect()
    }

    fn source_names(&self, cx: &mut VisualTestContext) -> Vec<String> {
        self.panel.read_with(cx, |panel, _| {
            panel
                .tree()
                .sources()
                .iter()
                .map(|source| source.name.clone())
                .collect()
        })
    }

    fn modal(&self, cx: &mut VisualTestContext) -> Option<Entity<DataSourceModal>> {
        self.workspace.read_with(cx, |workspace, cx| {
            workspace.active_modal::<DataSourceModal>(cx)
        })
    }
}

async fn workspace_with_panel(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    let app_state = cx.update(|cx| {
        let state = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        editor::init(cx);
        crate::init(cx);
        state
    });
    app_state
        .fs
        .as_fake()
        .insert_tree(path!("/root"), json!({ "build.gradle": "" }))
        .await;
    let project = Project::test(app_state.fs.clone(), [path!("/root").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

    let calls: Calls = Default::default();
    let connections = Rc::new(RefCell::new(vec![
        connection("c1", "Local Postgres", "postgresql", true),
        connection("c2", "Reports", "sqlite", false),
    ]));
    let start_daemon: StartDaemon = {
        let calls = calls.clone();
        Rc::new(move |_, _| {
            let daemon: Rc<dyn Daemon> = Rc::new(FakeDaemon {
                connections: connections.clone(),
                calls: calls.clone(),
            });
            async move { Ok(daemon) }.boxed_local()
        })
    };
    cx.update(|_, cx| DaemonFactory::set(start_daemon, cx));

    let panel = workspace.update_in(cx, |workspace, window, cx| {
        let weak = workspace.weak_handle();
        let project = workspace.project().clone();
        let panel = cx.new(|cx| DatabasePanel::new(weak, project, window, cx));
        workspace.add_panel(panel.clone(), window, cx);
        panel
    });
    (
        Fixture {
            workspace,
            panel,
            calls,
        },
        cx,
    )
}

async fn shown_panel(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    let (fixture, cx) = workspace_with_panel(cx).await;
    cx.dispatch_action(ToggleDatabasePanel);
    cx.run_until_parked();
    (fixture, cx)
}

fn rendered_rows(panel: &Entity<DatabasePanel>, cx: &mut VisualTestContext) -> usize {
    panel.update_in(cx, |panel, _, cx| panel.render_rows(cx).len())
}

fn select_source(fixture: &Fixture, id: &str, cx: &mut VisualTestContext) {
    fixture.panel.update(cx, |panel, cx| {
        panel.select(Some(RowKey::data_source(id)), cx)
    });
}

fn set_field(field: &Entity<ui_input::InputField>, text: &str, cx: &mut VisualTestContext) {
    field.update_in(cx, |field, window, cx| field.set_text(text, window, cx));
}

#[gpui::test]
async fn the_panel_lists_the_data_sources_in_the_left_dock(cx: &mut TestAppContext) {
    let (fixture, cx) = workspace_with_panel(cx).await;
    assert_eq!(DatabasePanel::persistent_name(), "Database Panel");
    assert!(
        fixture.calls.borrow().is_empty(),
        "no daemon before the panel is shown"
    );

    cx.dispatch_action(ToggleDatabasePanel);
    cx.run_until_parked();

    fixture.workspace.read_with(cx, |workspace, cx| {
        let dock = workspace.left_dock().read(cx);
        assert!(dock.is_open(), "the left dock should be open");
        let visible = dock.visible_panel().expect("a panel should be visible");
        assert_eq!(visible.panel_id(), fixture.panel.entity_id());
    });
    assert_eq!(fixture.calls_of("db/connections/list").len(), 1);
    assert_eq!(fixture.source_names(cx), ["Local Postgres", "Reports"]);
    assert_eq!(rendered_rows(&fixture.panel, cx), 2);
    assert_eq!(driver_label("postgresql"), "PostgreSQL");
}

#[gpui::test]
async fn expanding_lists_the_schema_lazily(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    assert!(fixture.calls_of("db/introspect").is_empty());

    fixture
        .panel
        .update(cx, |panel, cx| panel.toggle(RowKey::data_source("c1"), cx));
    cx.run_until_parked();
    fixture.panel.update(cx, |panel, cx| {
        panel.toggle(RowKey::new("c1", vec!["public".to_string()]), cx)
    });
    cx.run_until_parked();

    assert_eq!(
        fixture.calls_of("db/introspect"),
        [
            json!({ "connectionId": "c1", "path": [] }),
            json!({ "connectionId": "c1", "path": ["public"] }),
        ]
    );
    let names = fixture.panel.read_with(cx, |panel, _| {
        panel
            .tree()
            .rows()
            .iter()
            .map(|row| match row {
                Row::DataSource { connection, .. } => connection.name.clone(),
                Row::Node { node, depth, .. } => format!("{}{}", "  ".repeat(*depth), node.name),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        names,
        [
            "Local Postgres",
            "  public",
            "    owners",
            "    owners_id_seq",
            "Reports"
        ]
    );
    assert_eq!(rendered_rows(&fixture.panel, cx), 5);

    // Collapsing and expanding again is served from the cache.
    fixture.panel.update(cx, |panel, cx| {
        panel.toggle(RowKey::data_source("c1"), cx);
        panel.toggle(RowKey::data_source("c1"), cx);
    });
    cx.run_until_parked();
    assert_eq!(fixture.calls_of("db/introspect").len(), 2);

    // Refresh drops the cache of the selected data source and lists what is open again.
    select_source(&fixture, "c1", cx);
    fixture.panel.update(cx, |panel, cx| panel.refresh(cx));
    cx.run_until_parked();
    assert_eq!(fixture.calls_of("db/introspect").len(), 4);
    assert_eq!(fixture.calls_of("db/connections/list").len(), 2);
    assert_eq!(rendered_rows(&fixture.panel, cx), 5);
}

#[gpui::test]
async fn a_failed_introspection_shows_an_error_row_and_badge(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture
        .panel
        .update(cx, |panel, cx| panel.toggle(RowKey::data_source("c2"), cx));
    cx.run_until_parked();
    fixture.panel.read_with(cx, |panel, _| {
        assert_eq!(
            panel.tree().root_error("c2").as_deref(),
            Some("connection refused")
        );
        assert!(panel.tree().rows().iter().any(|row| matches!(
            row,
            Row::Error { message, .. } if message == "connection refused"
        )));
    });
    assert_eq!(rendered_rows(&fixture.panel, cx), 3);
}

#[gpui::test]
async fn add_opens_the_dialog_and_save_stores_and_relists(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.add_data_source(window, cx));
    cx.run_until_parked();
    let modal = fixture.modal(cx).expect("the dialog should be open");
    assert_eq!(fixture.calls_of("db/drivers").len(), 1);

    modal.read_with(cx, |modal, cx| {
        assert!(!modal.is_editing());
        assert_eq!(modal.driver_id(), "postgresql");
        assert_eq!(
            modal.url_field().read(cx).text(cx),
            "jdbc:postgresql://localhost:5432/postgres"
        );
    });
    modal.update_in(cx, |modal, window, cx| {
        modal.select_driver("sqlite".to_string(), window, cx)
    });
    modal.read_with(cx, |modal, cx| {
        assert_eq!(
            modal.url_field().read(cx).text(cx),
            format!(
                "jdbc:sqlite:{}/db.sqlite",
                path!("/root").replace('\\', "/")
            ),
            "the untouched template follows the driver"
        );
    });
    modal.update_in(cx, |modal, window, cx| {
        modal.select_driver("postgresql".to_string(), window, cx)
    });

    let (name, url, user, password) = modal.read_with(cx, |modal, _| {
        (
            modal.name_field().clone(),
            modal.url_field().clone(),
            modal.user_field().clone(),
            modal.password_field().clone(),
        )
    });
    set_field(&name, "Billing", cx);
    set_field(&url, "jdbc:postgresql://db.internal:5432/billing", cx);
    set_field(&user, "billing", cx);
    set_field(&password, "s3cret", cx);

    modal.update(cx, |modal, cx| modal.test_connection(cx));
    cx.run_until_parked();
    modal.read_with(cx, |modal, _| {
        assert_eq!(
            modal.test_state(),
            &TestState::Succeeded("Connected (PostgreSQL 16.4)".into())
        );
    });

    modal.update(cx, |modal, cx| modal.save(cx));
    cx.run_until_parked();

    let expected_connection = json!({
        "id": "",
        "name": "Billing",
        "driverId": "postgresql",
        "url": "jdbc:postgresql://db.internal:5432/billing",
        "user": "billing",
        "hasPassword": false,
        "options": {}
    });
    assert_eq!(
        fixture.calls_of("db/connections/test"),
        [json!({ "connection": expected_connection, "password": "s3cret" })]
    );
    assert_eq!(
        fixture.calls_of("db/connections/save"),
        [json!({ "connection": expected_connection, "password": "s3cret" })]
    );
    assert_eq!(fixture.calls_of("db/connections/list").len(), 2);
    assert_eq!(
        fixture.source_names(cx),
        ["Local Postgres", "Reports", "Billing"]
    );
    fixture.panel.read_with(cx, |panel, _| {
        assert_eq!(panel.selected(), Some(&RowKey::data_source("c3")));
    });
    assert!(
        fixture.modal(cx).is_none(),
        "the dialog closes after saving"
    );
}

#[gpui::test]
async fn editing_keeps_the_id_and_can_forget_the_password(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    select_source(&fixture, "c1", cx);
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.edit_selected(window, cx));
    cx.run_until_parked();
    let modal = fixture.modal(cx).expect("the dialog should be open");
    modal.read_with(cx, |modal, cx| {
        assert!(modal.is_editing());
        assert_eq!(modal.name_field().read(cx).text(cx), "Local Postgres");
        assert_eq!(modal.user_field().read(cx).text(cx), "app");
        assert_eq!(modal.password_field().read(cx).text(cx), "", "never shown");
    });

    // Empty password keeps the stored one.
    modal.update(cx, |modal, cx| modal.save(cx));
    cx.run_until_parked();
    let saves = fixture.calls_of("db/connections/save");
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0]["connection"]["id"], "c1");
    assert_eq!(saves[0]["password"], Value::Null);

    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.edit_selected(window, cx));
    cx.run_until_parked();
    let modal = fixture.modal(cx).expect("the dialog should be open again");
    modal.update(cx, |modal, cx| {
        modal.set_forget_password(true, cx);
        modal.save(cx);
    });
    cx.run_until_parked();
    let saves = fixture.calls_of("db/connections/save");
    assert_eq!(saves.len(), 2);
    assert_eq!(saves[1]["password"], "");
}

#[gpui::test]
async fn save_without_a_name_is_refused_in_the_dialog(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.add_data_source(window, cx));
    cx.run_until_parked();
    let modal = fixture.modal(cx).expect("the dialog should be open");
    modal.update(cx, |modal, cx| modal.save(cx));
    cx.run_until_parked();
    assert!(fixture.calls_of("db/connections/save").is_empty());
    modal.read_with(cx, |modal, _| {
        assert_eq!(modal.error().map(|e| e.as_ref()), Some("Enter a name"));
    });
    assert!(fixture.modal(cx).is_some());
}

#[gpui::test]
async fn delete_asks_first_then_removes_and_relists(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    select_source(&fixture, "c2", cx);
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.delete_selected(window, cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "deleting asks for confirmation");
    assert!(fixture.calls_of("db/connections/delete").is_empty());

    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();

    assert_eq!(
        fixture.calls_of("db/connections/delete"),
        [json!({ "id": "c2" })]
    );
    assert_eq!(fixture.calls_of("db/connections/list").len(), 2);
    assert_eq!(fixture.source_names(cx), ["Local Postgres"]);
    fixture.panel.read_with(cx, |panel, _| {
        assert_eq!(panel.selected(), None);
    });
}

#[gpui::test]
async fn cancelling_the_delete_prompt_keeps_the_data_source(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    select_source(&fixture, "c2", cx);
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.delete_selected(window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(fixture.calls_of("db/connections/delete").is_empty());
    assert_eq!(fixture.source_names(cx), ["Local Postgres", "Reports"]);
}

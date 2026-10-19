//! The fake daemon and workspace fixture shared by the panel and console tests.

use crate::ToggleDatabasePanel;
use crate::data_source_modal::DataSourceModal;
use crate::panel::DatabasePanel;
use crate::tree::Row;
use brewery_daemon_client::{
    BuildTool, Daemon, DaemonFactory, ProjectModel, RunConfiguration, SpringModel, StartDaemon,
};
use futures::FutureExt as _;
use futures::channel::oneshot;
use futures::future::LocalBoxFuture;
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use project::Project;
use serde_json::{Value, json};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use util::path;
use workspace::{AppState, MultiWorkspace, Workspace};

/// What the fake daemon knows and what it was asked; shared between the fake and the test.
#[derive(Default)]
pub struct FakeState {
    pub connections: RefCell<Vec<Value>>,
    pub calls: RefCell<Vec<(String, Value)>>,
    /// Methods that fail with this message, as a daemon error with the code when one is given.
    pub failing: RefCell<HashMap<String, (Option<i32>, String)>>,
    /// Rows behind every cursor `db/execute` opens.
    pub result_rows: Cell<usize>,
    pub cursors_opened: Cell<u32>,
    /// Replaces the successful `db/connections/test` answer.
    pub test_reply: RefCell<Option<Value>>,
    /// Methods whose replies wait until the test releases them, oldest first.
    pub held_methods: RefCell<HashSet<String>>,
    pub held: RefCell<Vec<(String, oneshot::Sender<()>)>>,
    /// Replaces the `db/schema` answer of `schema()`.
    pub schema_reply: RefCell<Option<Value>>,
}

/// The `db/schema` snapshot of the fake: `public` with two tables and a view, `billing` with one
/// table.
pub fn schema() -> Value {
    json!({
        "schemas": [
            {
                "name": "public",
                "tables": [
                    {
                        "name": "owners",
                        "kind": "table",
                        "columns": [
                            { "name": "id", "type": "serial" },
                            { "name": "email", "type": "varchar(50)" },
                            { "name": "First Name", "type": "text" }
                        ]
                    },
                    {
                        "name": "owner_names",
                        "kind": "view",
                        "columns": [{ "name": "name", "type": "text" }]
                    },
                    {
                        "name": "pets",
                        "kind": "table",
                        "columns": [
                            { "name": "id", "type": "int4" },
                            { "name": "owner_id", "type": "int4" },
                            { "name": "name", "type": "text" }
                        ]
                    }
                ]
            },
            {
                "name": "billing",
                "tables": [
                    {
                        "name": "invoices",
                        "kind": "table",
                        "columns": [
                            { "name": "id", "type": "int8" },
                            { "name": "total", "type": "numeric(10,2)" }
                        ]
                    }
                ]
            }
        ],
        "truncated": false
    })
}

impl FakeState {
    pub fn fail(&self, method: &str, message: &str) {
        self.failing
            .borrow_mut()
            .insert(method.to_string(), (None, message.to_string()));
    }

    /// Fails `method` the way the daemon does: a JSON-RPC error with `code`.
    pub fn fail_with_code(&self, method: &str, code: i32, message: &str) {
        self.failing
            .borrow_mut()
            .insert(method.to_string(), (Some(code), message.to_string()));
    }

    pub fn succeed(&self, method: &str) {
        self.failing.borrow_mut().remove(method);
    }

    pub fn hold(&self, method: &str) {
        self.held_methods.borrow_mut().insert(method.to_string());
    }

    pub fn held_count(&self, method: &str) -> usize {
        self.held
            .borrow()
            .iter()
            .filter(|(held, _)| held == method)
            .count()
    }

    /// Lets the oldest held reply of `method` through.
    pub fn release(&self, method: &str) {
        let mut held = self.held.borrow_mut();
        let ix = held
            .iter()
            .position(|(held, _)| held == method)
            .expect("a reply of this method should be held");
        let (_, sender) = held.remove(ix);
        // The requester may be gone already (a dropped task); that is fine.
        sender.send(()).ok();
    }

    pub fn answer(&self, method: &str, params: &Value) -> anyhow::Result<Value> {
        if let Some((code, message)) = self.failing.borrow().get(method) {
            return Err(match code {
                Some(code) => anyhow::Error::new(lsp::ResponseError::new(
                    lsp::ResponseErrorCode::Other((*code).into()),
                    message.clone(),
                )),
                None => anyhow::anyhow!("{message}"),
            });
        }
        match method {
            "db/connections/list" => Ok(Value::Array(self.connections.borrow().clone())),
            "db/drivers" => Ok(json!([
                { "id": "postgresql", "name": "PostgreSQL", "bundled": true, "jarPaths": [] },
                { "id": "sqlite", "name": "SQLite", "bundled": true, "jarPaths": [] }
            ])),
            "db/drivers/addJar" => Ok(json!({
                "id": params["driverId"],
                "name": params["name"],
                "bundled": false,
                "jarPaths": params["jarPaths"]
            })),
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
            "db/connections/test" => Ok(self.test_reply.borrow().clone().unwrap_or_else(|| {
                json!({
                    "ok": true,
                    "message": "Connected",
                    "serverVersion": "PostgreSQL 16.4"
                })
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
            "db/execute" => {
                let sql = params["sql"].as_str().unwrap_or_default().to_lowercase();
                if sql.starts_with("update")
                    || sql.starts_with("insert")
                    || sql.starts_with("delete")
                {
                    // `/*more*/` stands for SQL the daemon found further results in.
                    return Ok(json!({
                        "cursorId": null, "columns": [], "updateCount": 3, "elapsedMs": 4,
                        "moreResults": sql.contains("/*more*/")
                    }));
                }
                if sql.starts_with("fail") {
                    anyhow::bail!("syntax error at or near \"fail\"");
                }
                let cursor = self.cursors_opened.get() + 1;
                self.cursors_opened.set(cursor);
                Ok(json!({
                    "cursorId": format!("k{cursor}"),
                    "columns": [{ "name": "id", "type": "int4" }, { "name": "name", "type": "text" }],
                    "updateCount": null,
                    "elapsedMs": 12
                }))
            }
            "db/fetch" => {
                let total = self.result_rows.get();
                let offset = params["offset"].as_u64().unwrap_or_default() as usize;
                let limit = params["limit"].as_u64().unwrap_or_default() as usize;
                let end = (offset + limit).min(total);
                let rows: Vec<Value> = (offset..end)
                    .map(|ix| {
                        let name = (ix % 10 != 0).then(|| format!("name {ix}"));
                        json!([ix.to_string(), name])
                    })
                    .collect();
                Ok(json!({ "rows": rows, "done": end >= total }))
            }
            "db/schema" => Ok(self.schema_reply.borrow().clone().unwrap_or_else(schema)),
            "db/cancel" | "db/close" => Ok(Value::Null),
            other => anyhow::bail!("unexpected {other}"),
        }
    }
}

/// Serves the `db/*` methods from `FakeState`.
pub struct FakeDaemon {
    pub state: Rc<FakeState>,
}

pub fn connection(id: &str, name: &str, driver: &str, has_password: bool) -> Value {
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
        let state = &self.state;
        state
            .calls
            .borrow_mut()
            .push((method.to_string(), params.clone()));
        let answer = state.answer(method, &params);
        let gate = state.held_methods.borrow().contains(method).then(|| {
            let (sender, receiver) = oneshot::channel();
            state.held.borrow_mut().push((method.to_string(), sender));
            receiver
        });
        async move {
            if let Some(gate) = gate {
                gate.await.ok();
            }
            answer
        }
        .boxed_local()
    }

    fn on_project_changed(&self, _: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        Box::new(())
    }

    fn on_spring_changed(&self, _: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        Box::new(())
    }

    fn on_exit(&self, _: Box<dyn FnOnce() + Send>) {}
}

pub struct Fixture {
    pub workspace: Entity<Workspace>,
    pub panel: Entity<DatabasePanel>,
    pub state: Rc<FakeState>,
}

impl Fixture {
    pub fn calls_of(&self, method: &str) -> Vec<Value> {
        self.state
            .calls
            .borrow()
            .iter()
            .filter(|(called, _)| called == method)
            .map(|(_, params)| params.clone())
            .collect()
    }

    pub fn source_names(&self, cx: &mut VisualTestContext) -> Vec<String> {
        self.panel.read_with(cx, |panel, _| {
            panel
                .tree()
                .sources()
                .iter()
                .map(|source| source.name.clone())
                .collect()
        })
    }

    pub fn modal(&self, cx: &mut VisualTestContext) -> Option<Entity<DataSourceModal>> {
        self.workspace.read_with(cx, |workspace, cx| {
            workspace.active_modal::<DataSourceModal>(cx)
        })
    }

    pub fn open_add_dialog(&self, cx: &mut VisualTestContext) -> Entity<DataSourceModal> {
        self.panel
            .update_in(cx, |panel, window, cx| panel.add_data_source(window, cx));
        cx.run_until_parked();
        self.modal(cx).expect("the dialog should be open")
    }

    pub fn panel_has_loading_row(&self, cx: &mut VisualTestContext) -> bool {
        self.panel.read_with(cx, |panel, _| {
            panel
                .tree()
                .rows()
                .iter()
                .any(|row| matches!(row, Row::Loading { .. }))
        })
    }
}

pub async fn workspace_with_panel(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
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

    let state = Rc::new(FakeState::default());
    state.result_rows.set(1200);
    *state.connections.borrow_mut() = vec![
        connection("c1", "Local Postgres", "postgresql", true),
        connection("c2", "Reports", "sqlite", false),
    ];
    let start_daemon: StartDaemon = {
        let state = state.clone();
        Rc::new(move |_, _| {
            let daemon: Rc<dyn Daemon> = Rc::new(FakeDaemon {
                state: state.clone(),
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
            state,
        },
        cx,
    )
}

pub async fn shown_panel(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    let (fixture, cx) = workspace_with_panel(cx).await;
    cx.dispatch_action(ToggleDatabasePanel);
    cx.run_until_parked();
    (fixture, cx)
}

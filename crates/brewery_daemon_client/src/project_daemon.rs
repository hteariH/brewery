//! One brewery-daemon per project, shared by every Brewery panel that needs it.
//!
//! A JVM per panel would import the same build twice and watch the same files twice, so panels
//! ask `ProjectDaemon::for_project` instead of starting their own. The daemon lives as long as
//! some panel holds the returned entity; the registry only keeps weak handles.

use crate::protocol::{
    AddDriverParams, CancelParams, CloseParams, ConnectionInfo, ConnectionsListParams, DbAddDriver,
    DbCancel, DbClose, DbConnections, DbDeleteConnection, DbDrivers, DbExecute, DbFetch,
    DbGetSchema, DbIntrospect, DbNode, DbSaveConnection, DbSchema, DbTestConnection,
    DeleteConnectionParams, DriverInfo, DriversParams, ExecuteParams, ExecuteResult, FetchParams,
    FetchResult, IntrospectParams, ProjectModel, RunConfiguration, SaveConnectionParams,
    SchemaParams, SpringModel, TestConnectionParams, TestConnectionResult,
};
use crate::{DaemonClient, is_import_error};
use anyhow::{Context as _, Error, Result, anyhow};
use futures::future::LocalBoxFuture;
use futures::{FutureExt as _, StreamExt as _};
use gpui::{
    App, AppContext as _, AsyncApp, Context, Entity, EntityId, EventEmitter, Global, SharedString,
    Task, WeakEntity,
};
use lsp::request::Request as _;
use project::Project;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// What a running daemon offers; tests substitute a fake that never starts a JVM.
pub trait Daemon {
    fn project_model(&self) -> LocalBoxFuture<'_, Result<ProjectModel>>;
    fn spring_model(&self) -> LocalBoxFuture<'_, Result<SpringModel>>;
    fn run_configurations(&self) -> LocalBoxFuture<'_, Result<Vec<RunConfiguration>>>;
    /// One `db/*` request as JSON: `method` is the wire name (`protocol::DbExecute::METHOD`, ...),
    /// `params` and the result are the serialised wire types. A single entry point keeps the seam
    /// small: a test fake matches on `method` and answers every database call with canned data.
    /// Daemons that know no databases keep the default, which fails.
    fn db_request(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> LocalBoxFuture<'_, Result<serde_json::Value>> {
        let _ = params;
        async move { Err(anyhow!("{method} is not supported by this daemon")) }.boxed_local()
    }
    /// The handler lives as long as the returned guard.
    fn on_project_changed(&self, f: Box<dyn Fn() + Send>) -> Box<dyn Any>;
    /// The handler lives as long as the returned guard.
    fn on_spring_changed(&self, f: Box<dyn Fn() + Send>) -> Box<dyn Any>;
    /// Calls `f` if the daemon dies on its own.
    fn on_exit(&self, f: Box<dyn FnOnce() + Send>);
}

impl Daemon for DaemonClient {
    fn project_model(&self) -> LocalBoxFuture<'_, Result<ProjectModel>> {
        DaemonClient::project_model(self).boxed_local()
    }

    fn spring_model(&self) -> LocalBoxFuture<'_, Result<SpringModel>> {
        DaemonClient::spring_model(self).boxed_local()
    }

    fn run_configurations(&self) -> LocalBoxFuture<'_, Result<Vec<RunConfiguration>>> {
        DaemonClient::run_configurations(self).boxed_local()
    }

    fn db_request(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> LocalBoxFuture<'_, Result<serde_json::Value>> {
        DaemonClient::db_request(self, method, params).boxed_local()
    }

    fn on_project_changed(&self, f: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        Box::new(DaemonClient::on_project_changed(self, f))
    }

    fn on_spring_changed(&self, f: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        Box::new(DaemonClient::on_spring_changed(self, f))
    }

    fn on_exit(&self, f: Box<dyn FnOnce() + Send>) {
        DaemonClient::on_exit(self, f)
    }
}

/// How a running daemon is obtained for a project root.
pub type StartDaemon =
    Rc<dyn Fn(PathBuf, AsyncApp) -> LocalBoxFuture<'static, Result<Rc<dyn Daemon>>>>;

fn start_real_daemon(
    root: PathBuf,
    mut cx: AsyncApp,
) -> LocalBoxFuture<'static, Result<Rc<dyn Daemon>>> {
    async move {
        let client: Rc<dyn Daemon> = Rc::new(DaemonClient::start(&root, &mut cx).await?);
        Ok(client)
    }
    .boxed_local()
}

/// Overrides how daemons are started. Without it the real JVM daemon is launched; tests set it
/// before any panel asks for a daemon.
#[derive(Clone)]
pub struct DaemonFactory(pub StartDaemon);

impl Global for DaemonFactory {}

impl DaemonFactory {
    pub fn set(start: StartDaemon, cx: &mut App) {
        cx.set_global(Self(start));
    }

    fn get(cx: &App) -> StartDaemon {
        match cx.try_global::<Self>() {
            Some(factory) => factory.0.clone(),
            None => Rc::new(start_real_daemon),
        }
    }
}

#[derive(Default)]
struct ProjectDaemons(HashMap<EntityId, WeakEntity<ProjectDaemon>>);

impl Global for ProjectDaemons {}

/// Why a daemon call failed. They are kept apart because the fix differs: a broken build is
/// retried against the running daemon, while a daemon that never started (or died) is restarted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonErrorKind {
    Connection,
    Import,
}

impl DaemonErrorKind {
    pub fn of(error: &Error) -> Self {
        if is_import_error(error) {
            Self::Import
        } else {
            Self::Connection
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DaemonState {
    Connecting,
    Ready,
    /// The daemon could not be started or kept running. `kind` is `Connection` for those; it is
    /// carried so that panels can treat it like the errors of their own calls.
    Failed {
        error: SharedString,
        kind: DaemonErrorKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectDaemonEvent {
    /// A (re)start began; calls fail until `Ready`.
    Connecting,
    /// The daemon is up; panels fetch what they show.
    Ready,
    /// Build files changed.
    ProjectChanged,
    /// Java sources changed.
    SpringChanged,
    /// See `ProjectDaemon::state` for the message. `gave_up` is set once when the daemon kept
    /// dying and is no longer restarted automatically; Reload is the way out.
    Failed { gave_up: bool },
}

const MAX_RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(5 * 60);

pub const DAEMON_KEEPS_STOPPING: &str =
    "The Brewery daemon stopped repeatedly. Open the log and press Reload.";

const NO_FOLDER: &str = "Open a folder to load its Java project.";

/// Allows a dead daemon to be restarted a few times, so that a daemon which dies right
/// after every start does not loop forever.
struct RestartPolicy {
    max: usize,
    window: Duration,
    history: VecDeque<Instant>,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max: MAX_RESTARTS,
            window: RESTART_WINDOW,
            history: VecDeque::new(),
        }
    }
}

impl RestartPolicy {
    /// Records a restart at `now` and returns whether it is allowed.
    fn allow(&mut self, now: Instant) -> bool {
        while self
            .history
            .front()
            .is_some_and(|&at| now.saturating_duration_since(at) >= self.window)
        {
            self.history.pop_front();
        }
        if self.history.len() >= self.max {
            return false;
        }
        self.history.push_back(now);
        true
    }

    fn reset(&mut self) {
        self.history.clear();
    }
}

enum Change {
    Project,
    Spring,
}

pub struct ProjectDaemon {
    project: WeakEntity<Project>,
    start_daemon: StartDaemon,
    state: DaemonState,
    /// Dropping the client stops the daemon process.
    client: Option<Rc<dyn Daemon>>,
    restart_policy: RestartPolicy,
    _notifications: Vec<Box<dyn Any>>,
    _watch_changes: Option<Task<()>>,
    _watch_exit: Option<Task<()>>,
    connect_task: Option<Task<()>>,
}

impl EventEmitter<ProjectDaemonEvent> for ProjectDaemon {}

impl ProjectDaemon {
    /// The project's daemon, started on first request. It stops once nobody holds the entity.
    ///
    /// Only touches globals and the project, so it is safe to call from `Panel` callbacks that
    /// run while the workspace is being updated.
    pub fn for_project(project: &Entity<Project>, cx: &mut App) -> Entity<Self> {
        let id = project.entity_id();
        let registry = cx.default_global::<ProjectDaemons>();
        registry.0.retain(|_, daemon| daemon.upgrade().is_some());
        if let Some(daemon) = registry.0.get(&id).and_then(WeakEntity::upgrade) {
            return daemon;
        }
        let start_daemon = DaemonFactory::get(cx);
        let project = project.downgrade();
        let daemon = cx.new(|cx| Self::new(project, start_daemon, cx));
        cx.default_global::<ProjectDaemons>()
            .0
            .insert(id, daemon.downgrade());
        daemon
    }

    fn new(
        project: WeakEntity<Project>,
        start_daemon: StartDaemon,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            project,
            start_daemon,
            state: DaemonState::Connecting,
            client: None,
            restart_policy: RestartPolicy::default(),
            _notifications: Vec::new(),
            _watch_changes: None,
            _watch_exit: None,
            connect_task: None,
        };
        this.connect(cx);
        this
    }

    pub fn state(&self) -> &DaemonState {
        &self.state
    }

    pub fn is_ready(&self) -> bool {
        self.state == DaemonState::Ready
    }

    /// Restarts the daemon with a fresh restart allowance: the JDK or the jar may have appeared
    /// since the last failure, and pressing Reload is the user's answer to "stopped repeatedly".
    /// Every subscriber sees `Connecting` and then `Ready` or `Failed`.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.restart_policy.reset();
        self.connect(cx);
    }

    pub fn project_model(&self, cx: &App) -> Task<Result<ProjectModel>> {
        let Some(client) = self.client.clone() else {
            return Task::ready(Err(not_running()));
        };
        cx.foreground_executor()
            .spawn(async move { client.project_model().await })
    }

    pub fn spring_model(&self, cx: &App) -> Task<Result<SpringModel>> {
        let Some(client) = self.client.clone() else {
            return Task::ready(Err(not_running()));
        };
        cx.foreground_executor()
            .spawn(async move { client.spring_model().await })
    }

    pub fn run_configurations(&self, cx: &App) -> Task<Result<Vec<RunConfiguration>>> {
        let Some(client) = self.client.clone() else {
            return Task::ready(Err(not_running()));
        };
        cx.foreground_executor()
            .spawn(async move { client.run_configurations().await })
    }

    /// Every `db/*` call goes through here, so a failed one carries the daemon's error (see
    /// `daemon_error_code` and `daemon_error_message`) when the daemon answered.
    fn db_call<P, R>(&self, method: &'static str, params: P, cx: &App) -> Task<Result<R>>
    where
        P: Serialize,
        R: DeserializeOwned + 'static,
    {
        let Some(client) = self.client.clone() else {
            return Task::ready(Err(not_running()));
        };
        let params = match serde_json::to_value(params) {
            Ok(params) => params,
            Err(error) => return Task::ready(Err(error.into())),
        };
        cx.foreground_executor().spawn(async move {
            let value = client.db_request(method, params).await?;
            serde_json::from_value(value).with_context(|| format!("unexpected {method} response"))
        })
    }

    pub fn db_drivers(&self, cx: &App) -> Task<Result<Vec<DriverInfo>>> {
        self.db_call(DbDrivers::METHOD, DriversParams {}, cx)
    }

    pub fn db_add_driver(&self, params: AddDriverParams, cx: &App) -> Task<Result<DriverInfo>> {
        self.db_call(DbAddDriver::METHOD, params, cx)
    }

    pub fn db_connections(&self, cx: &App) -> Task<Result<Vec<ConnectionInfo>>> {
        self.db_call(DbConnections::METHOD, ConnectionsListParams {}, cx)
    }

    pub fn db_save_connection(
        &self,
        params: SaveConnectionParams,
        cx: &App,
    ) -> Task<Result<ConnectionInfo>> {
        self.db_call(DbSaveConnection::METHOD, params, cx)
    }

    pub fn db_delete_connection(&self, id: String, cx: &App) -> Task<Result<()>> {
        self.db_call(
            DbDeleteConnection::METHOD,
            DeleteConnectionParams { id },
            cx,
        )
    }

    pub fn db_test_connection(
        &self,
        params: TestConnectionParams,
        cx: &App,
    ) -> Task<Result<TestConnectionResult>> {
        self.db_call(DbTestConnection::METHOD, params, cx)
    }

    pub fn db_introspect(&self, params: IntrospectParams, cx: &App) -> Task<Result<Vec<DbNode>>> {
        self.db_call(DbIntrospect::METHOD, params, cx)
    }

    pub fn db_schema(&self, params: SchemaParams, cx: &App) -> Task<Result<DbSchema>> {
        self.db_call(DbGetSchema::METHOD, params, cx)
    }

    pub fn db_execute(&self, params: ExecuteParams, cx: &App) -> Task<Result<ExecuteResult>> {
        self.db_call(DbExecute::METHOD, params, cx)
    }

    pub fn db_fetch(&self, params: FetchParams, cx: &App) -> Task<Result<FetchResult>> {
        self.db_call(DbFetch::METHOD, params, cx)
    }

    pub fn db_cancel(&self, params: CancelParams, cx: &App) -> Task<Result<()>> {
        self.db_call(DbCancel::METHOD, params, cx)
    }

    pub fn db_close(&self, cursor_id: String, cx: &App) -> Task<Result<()>> {
        self.db_call(DbClose::METHOD, CloseParams { cursor_id }, cx)
    }

    fn project_root(&self, cx: &App) -> Option<PathBuf> {
        let project = self.project.upgrade()?;
        let worktree = project.read(cx).visible_worktrees(cx).next()?;
        Some(worktree.read(cx).abs_path().to_path_buf())
    }

    fn disconnect(&mut self) {
        self.connect_task = None;
        self._watch_changes = None;
        self._watch_exit = None;
        self._notifications.clear();
        self.client = None;
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        self.disconnect();
        let Some(root) = self.project_root(cx) else {
            self.fail(NO_FOLDER.into(), false, cx);
            return;
        };
        self.state = DaemonState::Connecting;
        cx.emit(ProjectDaemonEvent::Connecting);
        cx.notify();
        let start = (self.start_daemon)(root, cx.to_async());
        self.connect_task = Some(cx.spawn(async move |this, cx| {
            let result = start.await;
            this.update(cx, |this, cx| match result {
                Ok(client) => this.attach(client, cx),
                Err(error) => this.fail(format!("{error:#}").into(), false, cx),
            })
            .ok();
        }));
    }

    fn attach(&mut self, client: Rc<dyn Daemon>, cx: &mut Context<Self>) {
        // The daemon notifies from its I/O thread; a channel brings that back to the
        // foreground, where events can be emitted.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Change>();
        let project_tx = tx.clone();
        self._notifications = vec![
            client.on_project_changed(Box::new(move || {
                project_tx.unbounded_send(Change::Project).ok();
            })),
            client.on_spring_changed(Box::new(move || {
                tx.unbounded_send(Change::Spring).ok();
            })),
        ];
        self._watch_changes = Some(cx.spawn(async move |this, cx| {
            while let Some(change) = rx.next().await {
                let event = match change {
                    Change::Project => ProjectDaemonEvent::ProjectChanged,
                    Change::Spring => ProjectDaemonEvent::SpringChanged,
                };
                if this.update(cx, |_, cx| cx.emit(event)).is_err() {
                    break;
                }
            }
        }));
        // Same hop for the exit notification, which comes from the watchdog thread.
        let (exit_tx, mut exit_rx) = futures::channel::mpsc::unbounded::<()>();
        client.on_exit(Box::new(move || {
            exit_tx.unbounded_send(()).ok();
        }));
        self._watch_exit = Some(cx.spawn(async move |this, cx| {
            if exit_rx.next().await.is_some() {
                this.update(cx, |this, cx| this.on_daemon_exit(cx)).ok();
            }
        }));
        self.client = Some(client);
        self.state = DaemonState::Ready;
        cx.emit(ProjectDaemonEvent::Ready);
        cx.notify();
    }

    /// Only reached when the process died by itself: a deliberate disconnect drops the
    /// watchdog first.
    fn on_daemon_exit(&mut self, cx: &mut Context<Self>) {
        if self.restart_policy.allow(Instant::now()) {
            self.connect(cx);
            return;
        }
        self.disconnect();
        self.fail(DAEMON_KEEPS_STOPPING.into(), true, cx);
    }

    fn fail(&mut self, error: SharedString, gave_up: bool, cx: &mut Context<Self>) {
        self.state = DaemonState::Failed {
            error,
            kind: DaemonErrorKind::Connection,
        };
        cx.emit(ProjectDaemonEvent::Failed { gave_up });
        cx.notify();
    }
}

fn not_running() -> Error {
    anyhow!("brewery-daemon is not running")
}

#[cfg(test)]
#[path = "project_daemon_tests.rs"]
mod tests;

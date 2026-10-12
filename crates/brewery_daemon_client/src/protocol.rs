//! Wire types of the brewery-daemon protocol. They mirror `Protocol.kt` in
//! `brewery-daemon/`; the JSON fixtures in `brewery-daemon/protocol/fixtures/`
//! pin both sides to the same format.

use lsp::notification::Notification;
use lsp::request::Request;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROTOCOL_VERSION: u32 = 1;

/// JSON-RPC error codes of the daemon (`ErrorCodes` in `Protocol.kt`).
/// The build files could not be imported (script error, missing JDK, ...).
pub const BUILD_IMPORT_FAILED: i32 = -32001;
/// `project/model` was requested before `project/open`.
pub const NOT_OPENED: i32 = -32002;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u32,
    pub daemon_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenProjectParams {
    pub root_dir: String,
}

/// Empty object: the daemon declares `project/model` with an (empty) params class.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectModelParams {}

/// Empty object, same reasoning as `ProjectModelParams`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectChangedParams {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildTool {
    Gradle,
    Maven,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildTask {
    pub name: String,
    pub group: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildModule {
    pub name: String,
    pub path: String,
    pub tasks: Vec<BuildTask>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModel {
    pub build_tool: BuildTool,
    pub root_dir: String,
    pub modules: Vec<BuildModule>,
}

/// Empty object, same reasoning as `ProjectModelParams`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpringModelParams {}

/// Empty object, same reasoning as `ProjectModelParams`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConfigurationsParams {}

/// Empty object, same reasoning as `ProjectModelParams`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpringChangedParams {}

/// What the daemon finds in the Java sources by syntax alone (annotations matched by name).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpringModel {
    pub applications: Vec<SpringApp>,
    pub beans: Vec<SpringBean>,
    pub endpoints: Vec<SpringEndpoint>,
}

/// A `@SpringBootApplication` class. `file` is absolute, `line` is 1-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpringApp {
    pub name: String,
    pub main_class: String,
    pub module_path: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpringBean {
    pub name: String,
    /// The declared type as written in the source (not resolved).
    #[serde(rename = "type")]
    pub type_name: String,
    pub kind: SpringBeanKind,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpringBeanKind {
    Component,
    Service,
    Repository,
    Controller,
    RestController,
    Configuration,
    /// A `@Bean` method.
    Bean,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpringEndpoint {
    pub method: HttpMethod,
    pub path: String,
    /// `SimpleClass#method`.
    pub handler: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    /// `@RequestMapping` without a `method`.
    Any,
}

/// A ready-to-run command line for an application found in the sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfiguration {
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub kind: RunConfigurationKind,
    pub main_class: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunConfigurationKind {
    SpringBoot,
    Application,
}

pub enum Initialize {}
impl Request for Initialize {
    type Params = InitializeParams;
    type Result = InitializeResult;
    const METHOD: &'static str = "initialize";
}

pub enum Shutdown {}
impl Request for Shutdown {
    type Params = ();
    type Result = ();
    const METHOD: &'static str = "shutdown";
}

pub enum OpenProject {}
impl Request for OpenProject {
    type Params = OpenProjectParams;
    type Result = ();
    const METHOD: &'static str = "project/open";
}

pub enum GetProjectModel {}
impl Request for GetProjectModel {
    type Params = ProjectModelParams;
    type Result = ProjectModel;
    const METHOD: &'static str = "project/model";
}

pub enum ProjectChanged {}
impl Notification for ProjectChanged {
    type Params = ProjectChangedParams;
    const METHOD: &'static str = "project/changed";
}

pub enum GetSpringModel {}
impl Request for GetSpringModel {
    type Params = SpringModelParams;
    type Result = SpringModel;
    const METHOD: &'static str = "spring/model";
}

pub enum GetRunConfigurations {}
impl Request for GetRunConfigurations {
    type Params = RunConfigurationsParams;
    type Result = Vec<RunConfiguration>;
    const METHOD: &'static str = "run/configurations";
}

/// Java sources changed; debounced on the daemon side like `project/changed`.
pub enum SpringChanged {}
impl Notification for SpringChanged {
    type Params = SpringChangedParams;
    const METHOD: &'static str = "spring/changed";
}

/// Database errors (`DbErrorCodes` in `db/DbProtocol.kt`).
pub const DB_CONNECTION_FAILED: i32 = -32010;
pub const DB_AUTH_FAILED: i32 = -32011;
pub const DB_QUERY_FAILED: i32 = -32012;
/// Unknown connection or cursor.
pub const DB_NOT_FOUND: i32 = -32013;
/// JSON-RPC invalid params, e.g. a fetch offset behind a forward-only cursor.
pub const DB_INVALID_PARAMS: i32 = -32602;

/// Largest page `db/fetch` serves; the daemon caps `limit` here.
pub const DB_FETCH_LIMIT_MAX: u32 = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverInfo {
    pub id: String,
    pub name: String,
    pub bundled: bool,
    pub jar_paths: Vec<String>,
}

/// A saved connection. `id` is empty when it is new; the daemon assigns one on save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub id: String,
    pub name: String,
    pub driver_id: String,
    pub url: String,
    #[serde(default)]
    pub user: Option<String>,
    pub has_password: bool,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    /// Only set in a `db/connections/save` result, when the password could not be stored in the
    /// OS secret store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// `password` is only sent when the user typed one; `None` keeps what is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveConnectionParams {
    pub connection: ConnectionInfo,
    pub password: Option<String>,
}

/// Same shape as saving, but nothing is stored.
pub type TestConnectionParams = SaveConnectionParams;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestConnectionResult {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub server_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConnectionParams {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddDriverParams {
    pub driver_id: String,
    pub name: String,
    pub class_name: String,
    pub jar_paths: Vec<String>,
}

/// Empty object, same reasoning as `ProjectModelParams`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriversParams {}

/// Empty object, same reasoning as `ProjectModelParams`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionsListParams {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DbNodeKind {
    Catalog,
    Schema,
    Table,
    View,
    Sequence,
    /// `columns`, `indexes` and `foreignKeys` under a table.
    Folder,
    Column,
    Index,
    ForeignKey,
    /// A kind a newer daemon added.
    #[serde(other)]
    Other,
}

/// One node of the lazy schema tree. `path` is what to send back to list its children.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbNode {
    pub name: String,
    pub kind: DbNodeKind,
    pub path: Vec<String>,
    pub has_children: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntrospectParams {
    pub connection_id: String,
    pub path: Vec<String>,
    /// Also list `pg_catalog`, `information_schema`, `sqlite_*` and the like.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_system: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteParams {
    pub connection_id: String,
    pub sql: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
}

/// `cursor_id` is `None` for statements without a result set; `update_count` is set then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteResult {
    pub cursor_id: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub update_count: Option<i64>,
    pub elapsed_ms: u64,
}

/// Rows `[offset, offset + limit)` of a cursor. Cursors are forward-only: an offset behind what
/// was fetched is rejected with `DB_INVALID_PARAMS`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchParams {
    pub cursor_id: String,
    pub offset: u32,
    pub limit: u32,
}

/// Values arrive as strings; `None` is SQL NULL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchResult {
    pub rows: Vec<Vec<Option<String>>>,
    pub done: bool,
}

/// Cancels the running statement of a connection's session; `cursor_id` may stand in for the
/// connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelParams {
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseParams {
    pub cursor_id: String,
}

pub enum DbDrivers {}
impl Request for DbDrivers {
    type Params = DriversParams;
    type Result = Vec<DriverInfo>;
    const METHOD: &'static str = "db/drivers";
}

pub enum DbAddDriver {}
impl Request for DbAddDriver {
    type Params = AddDriverParams;
    type Result = DriverInfo;
    const METHOD: &'static str = "db/drivers/addJar";
}

pub enum DbConnections {}
impl Request for DbConnections {
    type Params = ConnectionsListParams;
    type Result = Vec<ConnectionInfo>;
    const METHOD: &'static str = "db/connections/list";
}

pub enum DbSaveConnection {}
impl Request for DbSaveConnection {
    type Params = SaveConnectionParams;
    type Result = ConnectionInfo;
    const METHOD: &'static str = "db/connections/save";
}

pub enum DbDeleteConnection {}
impl Request for DbDeleteConnection {
    type Params = DeleteConnectionParams;
    type Result = ();
    const METHOD: &'static str = "db/connections/delete";
}

pub enum DbTestConnection {}
impl Request for DbTestConnection {
    type Params = TestConnectionParams;
    type Result = TestConnectionResult;
    const METHOD: &'static str = "db/connections/test";
}

pub enum DbIntrospect {}
impl Request for DbIntrospect {
    type Params = IntrospectParams;
    type Result = Vec<DbNode>;
    const METHOD: &'static str = "db/introspect";
}

pub enum DbExecute {}
impl Request for DbExecute {
    type Params = ExecuteParams;
    type Result = ExecuteResult;
    const METHOD: &'static str = "db/execute";
}

pub enum DbFetch {}
impl Request for DbFetch {
    type Params = FetchParams;
    type Result = FetchResult;
    const METHOD: &'static str = "db/fetch";
}

pub enum DbCancel {}
impl Request for DbCancel {
    type Params = CancelParams;
    type Result = ();
    const METHOD: &'static str = "db/cancel";
}

pub enum DbClose {}
impl Request for DbClose {
    type Params = CloseParams;
    type Result = ();
    const METHOD: &'static str = "db/close";
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB_CONNECTION: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/db-connection.json");
    const DB_SAVE_PARAMS: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/db-connections-save.params.json");
    const DB_TEST_RESULT: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/db-connections-test.result.json");
    const DB_DRIVERS: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/db-drivers.result.json");
    const DB_EXECUTE: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/db-execute.result.json");
    const DB_FETCH: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/db-fetch.result.json");
    const DB_INTROSPECT: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/db-introspect.result.json");

    #[test]
    fn db_error_codes_match_the_daemon() {
        assert_eq!(DB_CONNECTION_FAILED, -32010);
        assert_eq!(DB_AUTH_FAILED, -32011);
        assert_eq!(DB_QUERY_FAILED, -32012);
        assert_eq!(DB_NOT_FOUND, -32013);
        assert_eq!(DB_INVALID_PARAMS, -32602);
    }

    #[test]
    fn db_methods_use_the_daemon_names() {
        assert_eq!(DbDrivers::METHOD, "db/drivers");
        assert_eq!(DbAddDriver::METHOD, "db/drivers/addJar");
        assert_eq!(DbConnections::METHOD, "db/connections/list");
        assert_eq!(DbSaveConnection::METHOD, "db/connections/save");
        assert_eq!(DbDeleteConnection::METHOD, "db/connections/delete");
        assert_eq!(DbTestConnection::METHOD, "db/connections/test");
        assert_eq!(DbIntrospect::METHOD, "db/introspect");
        assert_eq!(DbExecute::METHOD, "db/execute");
        assert_eq!(DbFetch::METHOD, "db/fetch");
        assert_eq!(DbCancel::METHOD, "db/cancel");
        assert_eq!(DbClose::METHOD, "db/close");
    }

    #[test]
    fn db_connection_fixture_round_trips() {
        let connection: ConnectionInfo = round_trips(DB_CONNECTION);
        assert_eq!(connection.id, "c1");
        assert_eq!(connection.driver_id, "postgresql");
        assert_eq!(connection.user.as_deref(), Some("app"));
        assert!(connection.has_password);
        assert_eq!(
            connection.options.get("ssl").map(String::as_str),
            Some("true")
        );
        assert_eq!(connection.warning, None);
    }

    #[test]
    fn a_save_warning_is_read_and_a_missing_one_is_not_written() {
        let connection: ConnectionInfo = serde_json::from_str(
            r#"{"id":"c1","name":"n","driverId":"sqlite","url":"u","user":null,"hasPassword":false,"options":{},"warning":"no secret store"}"#,
        )
        .unwrap();
        assert_eq!(connection.warning.as_deref(), Some("no secret store"));
        assert_eq!(connection.user, None);
        let json = serde_json::to_value(ConnectionInfo {
            warning: None,
            ..connection
        })
        .unwrap();
        assert!(json.get("warning").is_none());
        // The daemon may leave out the user and the options.
        let bare: ConnectionInfo = serde_json::from_str(
            r#"{"id":"","name":"n","driverId":"sqlite","url":"u","hasPassword":false}"#,
        )
        .unwrap();
        assert!(bare.options.is_empty());
        assert_eq!(bare.user, None);
    }

    #[test]
    fn db_save_params_fixture_round_trips() {
        let params: SaveConnectionParams = round_trips(DB_SAVE_PARAMS);
        assert_eq!(params.connection.id, "");
        assert_eq!(params.connection.name, "Local Postgres");
        assert!(!params.connection.has_password);
        assert!(params.connection.options.is_empty());
        assert_eq!(params.password, None);
        let typed = SaveConnectionParams {
            password: Some("secret".into()),
            ..params
        };
        assert_eq!(
            serde_json::to_value(&typed).unwrap()["password"],
            serde_json::json!("secret")
        );
    }

    #[test]
    fn db_test_result_fixture_round_trips() {
        let result: TestConnectionResult = round_trips(DB_TEST_RESULT);
        assert!(result.ok);
        assert_eq!(result.message, "Connected");
        assert_eq!(result.server_version.as_deref(), Some("PostgreSQL 16.4"));
    }

    #[test]
    fn db_drivers_fixture_round_trips() {
        let drivers: Vec<DriverInfo> = round_trips(DB_DRIVERS);
        let ids: Vec<&str> = drivers.iter().map(|driver| driver.id.as_str()).collect();
        assert_eq!(ids, vec!["postgresql", "mysql", "mariadb", "sqlite"]);
        assert!(drivers.iter().all(|driver| driver.bundled));
        assert_eq!(drivers[0].name, "PostgreSQL");
        assert!(drivers[0].jar_paths.is_empty());
    }

    #[test]
    fn db_execute_fixture_round_trips() {
        let result: ExecuteResult = round_trips(DB_EXECUTE);
        assert_eq!(
            result.cursor_id.as_deref(),
            Some("6f1c2b9e-3d4a-4c55-9a0b-1f2e3d4c5b6a")
        );
        assert_eq!(result.columns.len(), 2);
        assert_eq!(result.columns[0].name, "id");
        assert_eq!(result.columns[1].type_name, "varchar");
        assert_eq!(result.update_count, None);
        assert_eq!(result.elapsed_ms, 12);

        let update: ExecuteResult =
            serde_json::from_str(r#"{"cursorId":null,"columns":[],"updateCount":3,"elapsedMs":1}"#)
                .unwrap();
        assert_eq!(update.cursor_id, None);
        assert_eq!(update.update_count, Some(3));
    }

    #[test]
    fn db_fetch_fixture_round_trips() {
        let result: FetchResult = round_trips(DB_FETCH);
        assert!(!result.done);
        assert_eq!(result.rows.len(), 4);
        assert_eq!(
            result.rows[0],
            vec![Some("1".to_string()), Some("Ada".to_string())]
        );
        assert_eq!(result.rows[1][1], None);
        assert_eq!(result.rows[2][1].as_deref(), Some("0x00ff10"));
        assert_eq!(result.rows[3][1].as_deref(), Some("2024-03-05T10:20:30"));
    }

    #[test]
    fn db_introspect_fixture_round_trips() {
        let nodes: Vec<DbNode> = round_trips(DB_INTROSPECT);
        let kinds: Vec<DbNodeKind> = nodes.iter().map(|node| node.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DbNodeKind::Schema,
                DbNodeKind::Table,
                DbNodeKind::Folder,
                DbNodeKind::Column,
                DbNodeKind::Index,
                DbNodeKind::ForeignKey,
                DbNodeKind::Sequence,
            ]
        );
        assert_eq!(nodes[1].path, vec!["public", "owners"]);
        assert!(nodes[1].has_children);
        assert_eq!(nodes[0].detail, None);
        assert_eq!(nodes[3].detail.as_deref(), Some("serial NOT NULL PK"));
        assert!(!nodes[3].has_children);
    }

    #[test]
    fn db_node_kinds_have_their_wire_names() {
        for (kind, name) in [
            (DbNodeKind::Catalog, "catalog"),
            (DbNodeKind::View, "view"),
            (DbNodeKind::ForeignKey, "foreignKey"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), serde_json::json!(name));
        }
        let unknown: DbNodeKind = serde_json::from_str("\"materializedView\"").unwrap();
        assert_eq!(unknown, DbNodeKind::Other);
    }

    #[test]
    fn db_request_params_use_camel_case() {
        let introspect = serde_json::to_value(IntrospectParams {
            connection_id: "c1".into(),
            path: vec!["public".into()],
            include_system: None,
        })
        .unwrap();
        assert_eq!(
            introspect,
            serde_json::json!({"connectionId": "c1", "path": ["public"]})
        );
        let with_system = serde_json::to_value(IntrospectParams {
            connection_id: "c1".into(),
            path: Vec::new(),
            include_system: Some(true),
        })
        .unwrap();
        assert_eq!(with_system["includeSystem"], serde_json::json!(true));

        assert_eq!(
            serde_json::to_value(FetchParams {
                cursor_id: "k".into(),
                offset: 500,
                limit: 500
            })
            .unwrap(),
            serde_json::json!({"cursorId": "k", "offset": 500, "limit": 500})
        );
        assert_eq!(
            serde_json::to_value(CancelParams {
                connection_id: Some("c1".into()),
                cursor_id: None
            })
            .unwrap(),
            serde_json::json!({"connectionId": "c1"})
        );
        assert_eq!(
            serde_json::to_value(CloseParams {
                cursor_id: "k".into()
            })
            .unwrap(),
            serde_json::json!({"cursorId": "k"})
        );
        assert_eq!(
            serde_json::to_value(AddDriverParams {
                driver_id: "d".into(),
                name: "D".into(),
                class_name: "org.D".into(),
                jar_paths: vec!["/a.jar".into()],
            })
            .unwrap(),
            serde_json::json!({"driverId": "d", "name": "D", "className": "org.D", "jarPaths": ["/a.jar"]})
        );
        assert_eq!(
            serde_json::to_value(ExecuteParams {
                connection_id: "c1".into(),
                sql: "select 1".into()
            })
            .unwrap(),
            serde_json::json!({"connectionId": "c1", "sql": "select 1"})
        );
        assert_eq!(
            serde_json::to_value(DriversParams {}).unwrap(),
            serde_json::json!({})
        );
        assert_eq!(
            serde_json::to_value(ConnectionsListParams {}).unwrap(),
            serde_json::json!({})
        );
    }

    const INITIALIZE_PARAMS: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/initialize.params.json");
    const INITIALIZE_RESULT: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/initialize.result.json");
    const GRADLE_MODEL: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/project-model.gradle.json");

    #[test]
    fn error_codes_match_the_daemon() {
        assert_eq!(BUILD_IMPORT_FAILED, -32001);
        assert_eq!(NOT_OPENED, -32002);
    }

    #[test]
    fn initialize_params_match_fixture() {
        let expected: serde_json::Value = serde_json::from_str(INITIALIZE_PARAMS).unwrap();
        let actual = serde_json::to_value(InitializeParams {
            protocol_version: PROTOCOL_VERSION,
        })
        .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn initialize_result_fixture_deserialises() {
        let result: InitializeResult = serde_json::from_str(INITIALIZE_RESULT).unwrap();
        assert_eq!(result.protocol_version, PROTOCOL_VERSION);
        assert_eq!(result.daemon_version, "0.1.0");
    }

    #[test]
    fn project_model_fixture_deserialises() {
        let model: ProjectModel = serde_json::from_str(GRADLE_MODEL).unwrap();
        assert_eq!(model.build_tool, BuildTool::Gradle);
        assert_eq!(model.modules.len(), 1);
        assert_eq!(model.modules[0].path, ":");
        let task = &model.modules[0].tasks[0];
        assert_eq!(task.name, "build");
        assert_eq!(task.group.as_deref(), Some("build"));
        assert_eq!(
            task.description.as_deref(),
            Some("Assembles and tests this project.")
        );
    }

    #[test]
    fn null_group_and_description_are_accepted() {
        let task: BuildTask =
            serde_json::from_str(r#"{"name":"clean","group":null,"description":null}"#).unwrap();
        assert_eq!(task.group, None);
        assert_eq!(task.description, None);
    }

    #[test]
    fn empty_params_serialise_as_objects() {
        assert_eq!(
            serde_json::to_value(ProjectModelParams {}).unwrap(),
            serde_json::json!({})
        );
    }

    const SPRING_MODEL: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/spring-model.json");
    const RUN_CONFIGURATIONS: &str =
        include_str!("../../../brewery-daemon/protocol/fixtures/run-configurations.json");

    /// The daemon writes these fixtures with a placeholder root (`/path/to/spring-petshop`) that
    /// its own tests swap for the real fixture directory; here it is just a string, so the
    /// fixture is compared as is.
    fn round_trips<T: serde::de::DeserializeOwned + Serialize>(fixture: &str) -> T {
        let expected: serde_json::Value = serde_json::from_str(fixture).unwrap();
        let parsed: T = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), expected);
        parsed
    }

    #[test]
    fn spring_model_fixture_round_trips() {
        let model: SpringModel = round_trips(SPRING_MODEL);
        assert_eq!(model.applications.len(), 1);
        let app = &model.applications[0];
        assert_eq!(app.name, "PetshopApplication");
        assert_eq!(app.main_class, "com.example.petshop.PetshopApplication");
        assert_eq!(app.module_path, ":");
        assert!(
            app.file.ends_with("PetshopApplication.java"),
            "{}",
            app.file
        );
        assert_eq!(app.line, 6);

        let kinds: Vec<(&str, SpringBeanKind)> = model
            .beans
            .iter()
            .map(|bean| (bean.name.as_str(), bean.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("appConfig", SpringBeanKind::Configuration),
                ("clock", SpringBeanKind::Bean),
                ("ownerController", SpringBeanKind::RestController),
                ("ownerRepository", SpringBeanKind::Repository),
                ("ownerService", SpringBeanKind::Service),
            ]
        );
        assert_eq!(model.beans[1].type_name, "Clock");

        let endpoints: Vec<(HttpMethod, &str, &str)> = model
            .endpoints
            .iter()
            .map(|endpoint| {
                (
                    endpoint.method,
                    endpoint.path.as_str(),
                    endpoint.handler.as_str(),
                )
            })
            .collect();
        assert_eq!(
            endpoints,
            vec![
                (HttpMethod::Get, "/owners", "OwnerController#list"),
                (HttpMethod::Get, "/owners/{id}", "OwnerController#get"),
                (HttpMethod::Post, "/owners", "OwnerController#create"),
                (HttpMethod::Any, "/owners/legacy", "OwnerController#legacy"),
            ]
        );
        assert_eq!(model.endpoints[0].line, 19);
    }

    #[test]
    fn every_bean_kind_and_method_has_its_wire_name() {
        let kinds = [
            (SpringBeanKind::Component, "component"),
            (SpringBeanKind::Service, "service"),
            (SpringBeanKind::Repository, "repository"),
            (SpringBeanKind::Controller, "controller"),
            (SpringBeanKind::RestController, "restController"),
            (SpringBeanKind::Configuration, "configuration"),
            (SpringBeanKind::Bean, "bean"),
        ];
        for (kind, name) in kinds {
            assert_eq!(serde_json::to_value(kind).unwrap(), serde_json::json!(name));
        }
        let methods = [
            (HttpMethod::Get, "GET"),
            (HttpMethod::Post, "POST"),
            (HttpMethod::Put, "PUT"),
            (HttpMethod::Delete, "DELETE"),
            (HttpMethod::Patch, "PATCH"),
            (HttpMethod::Any, "ANY"),
        ];
        for (method, name) in methods {
            assert_eq!(
                serde_json::to_value(method).unwrap(),
                serde_json::json!(name)
            );
        }
    }

    #[test]
    fn run_configurations_fixture_round_trips() {
        let configurations: Vec<RunConfiguration> = round_trips(RUN_CONFIGURATIONS);
        assert_eq!(configurations.len(), 2);
        let boot = &configurations[0];
        assert_eq!(boot.label, "PetshopApplication");
        assert_eq!(boot.command, "/path/to/spring-petshop/gradlew");
        assert_eq!(boot.args, vec!["bootRun".to_string()]);
        assert_eq!(boot.cwd, "/path/to/spring-petshop");
        assert_eq!(boot.kind, RunConfigurationKind::SpringBoot);
        let plain = &configurations[1];
        assert_eq!(plain.kind, RunConfigurationKind::Application);
        assert_eq!(
            plain.args,
            vec![
                ":reports:run".to_string(),
                "-PmainClass=com.example.reports.ReportsApplication".to_string()
            ]
        );
    }

    #[test]
    fn new_methods_use_the_daemon_names() {
        assert_eq!(GetSpringModel::METHOD, "spring/model");
        assert_eq!(GetRunConfigurations::METHOD, "run/configurations");
        assert_eq!(SpringChanged::METHOD, "spring/changed");
        assert_eq!(
            serde_json::to_value(SpringModelParams {}).unwrap(),
            serde_json::json!({})
        );
        assert_eq!(
            serde_json::to_value(RunConfigurationsParams {}).unwrap(),
            serde_json::json!({})
        );
    }
}

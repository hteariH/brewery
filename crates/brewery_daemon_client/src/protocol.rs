//! Wire types of the brewery-daemon protocol. They mirror `Protocol.kt` in
//! `brewery-daemon/`; the JSON fixtures in `brewery-daemon/protocol/fixtures/`
//! pin both sides to the same format.

use lsp::notification::Notification;
use lsp::request::Request;
use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}

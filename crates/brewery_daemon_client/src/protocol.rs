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

//! Brewery: turns the Java extension's `java-main` runnables into debug scenarios, so the
//! gutter menu offers "debug: ..." for a `main` class without a `.zed/debug.json`.
//!
//! The locator only sees the unresolved task template, while the class's package lives in
//! the runnable's task variables, which the debugger substitutes into the scenario config
//! when the session starts. Task variable substitution has defaults but no conditionals, so
//! a class in the default package would come out as `.Main`; `finalize_substituted_config`
//! runs right after that substitution and removes the dot.

use anyhow::{Result, bail};
use async_trait::async_trait;
use dap::{DapLocator, DebugRequest, adapters::DebugAdapterName};
use gpui::{BackgroundExecutor, SharedString};
use serde_json::{Value, json};
use task::{DebugScenario, SpawnInTerminal, TaskTemplate};

/// Name under which the Java extension registers its java-debug adapter.
const JAVA_ADAPTER: &str = "Java";
/// Tag the Java extension's runnables query sets on classes with a `main` method.
const JAVA_MAIN_TAG: &str = "java-main";
/// The `:-` default keeps an absent package (default package) from failing substitution,
/// which would otherwise leave the whole string unsubstituted.
const MAIN_CLASS_TEMPLATE: &str = "${ZED_CUSTOM_java_package_name:-}.$ZED_CUSTOM_java_class_name";

pub struct JavaLocator;

#[async_trait]
impl DapLocator for JavaLocator {
    fn name(&self) -> SharedString {
        SharedString::new_static("brewery-java-locator")
    }

    async fn create_scenario(
        &self,
        build_config: &TaskTemplate,
        resolved_label: &str,
        adapter: &DebugAdapterName,
    ) -> Option<DebugScenario> {
        if adapter.0.as_ref() != JAVA_ADAPTER
            || !build_config.tags.iter().any(|tag| tag == JAVA_MAIN_TAG)
        {
            return None;
        }
        // No build step: java-debug launches from the classes jdtls already compiles, and the
        // run task's shell script would start the program rather than just build it.
        Some(DebugScenario {
            adapter: adapter.0.clone(),
            label: resolved_label.to_owned().into(),
            build: None,
            config: json!({
                "request": "launch",
                // An explicit class matters: without it java-debug refuses to pick one as soon
                // as the project has more than one `main`.
                "mainClass": MAIN_CLASS_TEMPLATE,
                "cwd": "$ZED_WORKTREE_ROOT",
            }),
            tcp_connection: None,
        })
    }

    async fn run(&self, _: SpawnInTerminal, _executor: BackgroundExecutor) -> Result<DebugRequest> {
        bail!("Java locator should not require DapLocator::run to be ran");
    }
}

/// Called on a scenario's config after task variables were substituted into it. A leading
/// dot can only come from an empty package, and is never valid in a Java class name, so
/// stripping it also cannot break a hand-written `debug.json` entry.
pub fn finalize_substituted_config(adapter: &str, config: &mut Value) {
    if adapter != JAVA_ADAPTER {
        return;
    }
    if let Some(Value::String(main_class)) = config.get_mut("mainClass")
        && let Some(without_dot) = main_class.strip_prefix('.')
    {
        *main_class = without_dot.to_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use task::{TaskContext, TaskVariables, VariableName, substitute_variables_in_str};

    fn java_main_template() -> TaskTemplate {
        TaskTemplate {
            label: "Run $ZED_CUSTOM_java_class_name".to_owned(),
            command: "pkg=\"${ZED_CUSTOM_java_package_name:-}\"; java \"$pkg\"".to_owned(),
            tags: vec!["java-main".to_owned()],
            ..TaskTemplate::default()
        }
    }

    fn java_adapter() -> DebugAdapterName {
        DebugAdapterName("Java".into())
    }

    fn create(template: &TaskTemplate, adapter: &DebugAdapterName) -> Option<DebugScenario> {
        smol::block_on(JavaLocator.create_scenario(template, "Run App", adapter))
    }

    /// Mirrors what the debugger does when the scenario starts: substitute every config
    /// string with the runnable's task context, then let the locator finish the config.
    fn resolve(scenario: &DebugScenario, variables: &[(&'static str, &str)]) -> serde_json::Value {
        let context = TaskContext {
            task_variables: variables
                .iter()
                .map(|(name, value)| (VariableName::Custom((*name).into()), (*value).to_owned()))
                .collect::<TaskVariables>(),
            ..TaskContext::default()
        };
        let mut config = scenario.config.clone();
        if let Some(object) = config.as_object_mut() {
            for value in object.values_mut() {
                if let serde_json::Value::String(s) = value
                    && let Some(substituted) = substitute_variables_in_str(s, &context)
                {
                    *s = substituted;
                }
            }
        }
        finalize_substituted_config(&scenario.adapter, &mut config);
        config
    }

    #[test]
    fn ignores_other_adapters() {
        assert!(create(&java_main_template(), &DebugAdapterName("Debugpy".into())).is_none());
    }

    #[test]
    fn ignores_templates_without_java_main_tag() {
        let mut template = java_main_template();
        template.tags = vec!["java-test-class".to_owned()];
        assert!(create(&template, &java_adapter()).is_none());
        template.tags.clear();
        assert!(create(&template, &java_adapter()).is_none());
    }

    #[test]
    fn launches_java_main_without_build_step() {
        let scenario = create(&java_main_template(), &java_adapter()).expect("java-main scenario");
        assert_eq!(scenario.adapter.as_ref(), "Java");
        assert_eq!(scenario.label.as_ref(), "Run App");
        assert!(scenario.build.is_none());
        assert_eq!(scenario.config["request"], json!("launch"));
        assert_eq!(scenario.config["cwd"], json!("$ZED_WORKTREE_ROOT"));
    }

    #[test]
    fn main_class_is_fully_qualified_in_a_package() {
        let scenario = create(&java_main_template(), &java_adapter()).expect("java-main scenario");
        let config = resolve(
            &scenario,
            &[
                ("java_package_name", "com.example.app"),
                ("java_class_name", "App"),
            ],
        );
        assert_eq!(config["mainClass"], json!("com.example.app.App"));
    }

    #[test]
    fn main_class_has_no_leading_dot_in_the_default_package() {
        let scenario = create(&java_main_template(), &java_adapter()).expect("java-main scenario");
        let config = resolve(&scenario, &[("java_class_name", "App")]);
        assert_eq!(config["mainClass"], json!("App"));
    }

    #[test]
    fn finalize_leaves_other_adapters_alone() {
        let mut config = json!({ "mainClass": ".App" });
        finalize_substituted_config("Debugpy", &mut config);
        assert_eq!(config["mainClass"], json!(".App"));
    }
}

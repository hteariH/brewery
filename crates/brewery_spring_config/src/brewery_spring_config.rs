//! Spring Boot configuration support (`application.yml`, `application.properties`).
//!
//! The daemon jar doubles as a language server (`--spring-config-lsp`) that completes,
//! documents and checks configuration keys against the project's Spring metadata. Zed
//! starts it like any other language server, so it only runs once a YAML or Properties
//! buffer is open, and the server itself ignores files that are not Spring config.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use brewery_daemon_client::{find_daemon_jar, find_java};
use futures::{FutureExt as _, future::Shared, lock::OwnedMutexGuard};
use gpui::{App, AppContext as _, AsyncApp, Task};
use language::{
    DynLspInstaller, LanguageName, LanguageRegistry, LanguageServerBinaryLocations, LspAdapter,
    LspAdapterDelegate, Toolchain,
};
use lsp::{LanguageServerBinary, LanguageServerBinaryOptions, LanguageServerName};

pub const SERVER_NAME: &str = "brewery-spring-config";

/// `Properties` is the Java extension's name for `.properties` files.
const LANGUAGES: [&str; 2] = ["YAML", "Properties"];

/// The daemon jar is compiled for Java 21, the same floor `ProjectDaemon` uses.
const MIN_JAVA_MAJOR: u32 = 21;

/// Registers the Spring configuration server for YAML and Properties buffers.
pub fn init(languages: Arc<LanguageRegistry>, cx: &mut App) {
    register(
        &languages,
        || discover(|| find_java(MIN_JAVA_MAJOR), find_daemon_jar),
        cx,
    );
}

fn register(
    languages: &LanguageRegistry,
    discover: impl FnOnce() -> Option<LanguageServerBinary> + Send + 'static,
    cx: &mut App,
) {
    let adapter: Arc<dyn LspAdapter> = Arc::new(SpringConfigLspAdapter::new(discover, cx));
    for language in LANGUAGES {
        languages.register_lsp_adapter(LanguageName::new_static(language), adapter.clone());
    }
}

fn server_binary(java: PathBuf, jar: PathBuf) -> LanguageServerBinary {
    LanguageServerBinary {
        path: java,
        arguments: vec![
            OsString::from("-jar"),
            jar.into_os_string(),
            OsString::from("--spring-config-lsp"),
        ],
        env: None,
    }
}

/// Runs once per app, so a machine without a JDK or jar gets exactly one warning.
fn discover(
    find_java: impl FnOnce() -> Result<PathBuf>,
    find_jar: impl FnOnce() -> Result<PathBuf>,
) -> Option<LanguageServerBinary> {
    match find_java().and_then(|java| Ok((java, find_jar()?))) {
        Ok((java, jar)) => Some(server_binary(java, jar)),
        Err(error) => {
            log::warn!("Spring configuration completion is off: {error:#}");
            None
        }
    }
}

/// Uses the locally found JDK and daemon jar; nothing is ever downloaded.
pub struct SpringConfigLspAdapter {
    /// Set when discovery finishes, for the synchronous `is_opt_in_for`.
    resolved: Arc<OnceLock<Option<LanguageServerBinary>>>,
    discovery: Shared<Task<Option<LanguageServerBinary>>>,
}

impl SpringConfigLspAdapter {
    fn new(
        discover: impl FnOnce() -> Option<LanguageServerBinary> + Send + 'static,
        cx: &App,
    ) -> Self {
        let resolved = Arc::new(OnceLock::new());
        // Discovery may run `java -version`, so it stays off the main thread.
        let discovery = cx
            .background_spawn({
                let resolved = resolved.clone();
                async move {
                    let binary = discover();
                    resolved.get_or_init(|| binary.clone());
                    binary
                }
            })
            .shared();
        Self {
            resolved,
            discovery,
        }
    }

    async fn binary(&self) -> Result<LanguageServerBinary> {
        self.discovery.clone().await.ok_or_else(|| {
            anyhow!("no JDK {MIN_JAVA_MAJOR}+ or Brewery daemon jar found (see the log)")
        })
    }
}

#[async_trait(?Send)]
impl DynLspInstaller for SpringConfigLspAdapter {
    async fn try_fetch_server_binary(
        &self,
        _: &Arc<dyn LspAdapterDelegate>,
        _: PathBuf,
        _: bool,
        _: &mut AsyncApp,
    ) -> Result<LanguageServerBinary> {
        Err(anyhow!(
            "{SERVER_NAME} ships with Brewery and is never downloaded"
        ))
    }

    fn get_language_server_command(
        self: Arc<Self>,
        _: Arc<dyn LspAdapterDelegate>,
        _: Option<Toolchain>,
        _: LanguageServerBinaryOptions,
        _: OwnedMutexGuard<Option<(bool, LanguageServerBinary)>>,
        _: AsyncApp,
    ) -> LanguageServerBinaryLocations {
        async move { (self.binary().await, None) }.boxed_local()
    }
}

#[async_trait(?Send)]
impl LspAdapter for SpringConfigLspAdapter {
    fn name(&self) -> LanguageServerName {
        LanguageServerName::new_static(SERVER_NAME)
    }

    /// Once discovery has failed the server becomes opt-in, so the default server list
    /// skips it instead of failing (and flagging an error) for every buffer and
    /// worktree. Still pending counts as available, so buffers restored at startup
    /// get the server.
    fn is_opt_in_for(&self, _: &LanguageName) -> bool {
        matches!(self.resolved.get(), Some(None))
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::Arc;

    use anyhow::anyhow;
    use gpui::TestAppContext;
    use language::{LanguageName, LanguageRegistry};

    use super::*;

    fn fake_binary() -> lsp::LanguageServerBinary {
        server_binary(
            PathBuf::from("/jdk/bin/java"),
            PathBuf::from("/brewery/daemon.jar"),
        )
    }

    /// `LanguageServerBinary` has no `PartialEq`/`Debug`.
    fn command(binary: &lsp::LanguageServerBinary) -> (PathBuf, Vec<OsString>, bool) {
        (
            binary.path.clone(),
            binary.arguments.clone(),
            binary.env.is_some(),
        )
    }

    fn fake_command() -> Option<(PathBuf, Vec<OsString>, bool)> {
        Some(command(&fake_binary()))
    }

    #[test]
    fn binary_runs_the_daemon_jar_in_spring_config_mode() {
        let (path, arguments, has_env) = command(&fake_binary());
        assert_eq!(path, PathBuf::from("/jdk/bin/java"));
        assert_eq!(
            arguments,
            ["-jar", "/brewery/daemon.jar", "--spring-config-lsp"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
        assert!(!has_env);
    }

    #[test]
    fn discovery_needs_both_java_and_the_jar() {
        assert_eq!(
            discover(
                || Ok(PathBuf::from("/jdk/bin/java")),
                || Ok(PathBuf::from("/brewery/daemon.jar"))
            )
            .as_ref()
            .map(command),
            fake_command()
        );
        assert!(
            discover(
                || Err(anyhow!("no JDK 21")),
                || Ok(PathBuf::from("/brewery/daemon.jar"))
            )
            .is_none()
        );
        assert!(
            discover(
                || Ok(PathBuf::from("/jdk/bin/java")),
                || Err(anyhow!("no daemon jar"))
            )
            .is_none()
        );
    }

    #[gpui::test]
    async fn found_binary_is_served_to_every_worktree(cx: &mut TestAppContext) {
        let adapter = cx.update(|cx| SpringConfigLspAdapter::new(|| Some(fake_binary()), cx));
        cx.run_until_parked();
        assert!(!adapter.is_opt_in_for(&LanguageName::new_static("YAML")));
        assert_eq!(
            adapter.binary().await.ok().as_ref().map(command),
            fake_command()
        );
        assert_eq!(
            adapter.binary().await.ok().as_ref().map(command),
            fake_command()
        );
    }

    #[gpui::test]
    async fn missing_java_keeps_the_server_from_starting(cx: &mut TestAppContext) {
        let adapter = cx.update(|cx| SpringConfigLspAdapter::new(|| None, cx));
        cx.run_until_parked();
        // Opt-in means the default `...` server list skips it, so Zed never tries to
        // start it and never shows a failed-server indicator.
        assert!(adapter.is_opt_in_for(&LanguageName::new_static("YAML")));
        assert!(adapter.is_opt_in_for(&LanguageName::new_static("Properties")));
        assert!(adapter.binary().await.is_err());
    }

    #[gpui::test]
    async fn pending_discovery_does_not_hide_the_server(cx: &mut TestAppContext) {
        let adapter = cx.update(|cx| SpringConfigLspAdapter::new(|| Some(fake_binary()), cx));
        // Discovery has not run yet: a buffer opened now must still get the server.
        assert!(!adapter.is_opt_in_for(&LanguageName::new_static("YAML")));
        assert_eq!(
            adapter.binary().await.ok().as_ref().map(command),
            fake_command()
        );
    }

    #[gpui::test]
    fn registers_for_yaml_and_properties(cx: &mut TestAppContext) {
        let languages = Arc::new(LanguageRegistry::new(cx.executor()));
        cx.update(|cx| register(&languages, || Some(fake_binary()), cx));
        for language in ["YAML", "Properties"] {
            let names = languages
                .lsp_adapters(&LanguageName::new_static(language))
                .iter()
                .map(|adapter| adapter.name.0.to_string())
                .collect::<Vec<_>>();
            assert_eq!(names, vec![SERVER_NAME.to_string()], "{language}");
        }
    }
}

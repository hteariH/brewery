//! Client for `brewery-daemon`, the JVM process that reads the Gradle/Maven model.
//!
//! The process is driven through Zed's `lsp::LanguageServer` because it already does
//! what we need: spawn with stdio pipes, `Content-Length` framing, request/response
//! correlation, notifications. Only the LSP `initialize` handshake is skipped; the
//! daemon speaks its own `initialize` (see `protocol`).

mod jdk;
pub mod protocol;

pub use jdk::{JdkCandidate, find_daemon_jar, find_java, parse_release_version, pick_jdk};
pub use protocol::{BuildModule, BuildTask, BuildTool, PROTOCOL_VERSION, ProjectModel};

use anyhow::{Context as _, Result, bail};
use gpui::{AppContext as _, AsyncApp};
use parking_lot::Mutex;
use protocol::{
    GetProjectModel, Initialize, InitializeParams, OpenProject, OpenProjectParams, ProjectChanged,
    ProjectModelParams,
};
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The daemon only needs a JDK 21 toolchain to run the Gradle Tooling API.
const MIN_JAVA_MAJOR: u32 = 21;
/// JVM start-up plus handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);
/// The first Gradle import may download a distribution, so this is generous.
const IMPORT_TIMEOUT: Duration = Duration::from_secs(600);

/// Dropping the client also stops the daemon: `lsp::LanguageServer` shuts its process
/// down on drop. `shutdown` is for callers that want to wait for that to finish.
pub struct DaemonClient {
    server: Arc<lsp::LanguageServer>,
}

impl DaemonClient {
    /// Starts the daemon for `root`, checks the protocol version and opens the project.
    pub async fn start(root: &Path, cx: &mut AsyncApp) -> Result<Self> {
        // Discovery touches the file system and may run `java -version`.
        let (java, jar) = cx
            .background_spawn(async {
                Ok::<_, anyhow::Error>((find_java(MIN_JAVA_MAJOR)?, find_daemon_jar()?))
            })
            .await?;
        let binary = lsp::LanguageServerBinary {
            path: java,
            arguments: vec![OsString::from("-jar"), jar.into_os_string()],
            env: None,
        };
        let server = lsp::LanguageServer::new(
            Arc::new(Mutex::new(None)),
            lsp::LanguageServerId(0),
            lsp::LanguageServerName::new_static("brewery-daemon"),
            binary,
            root,
            None,
            None,
            cx,
        )
        .context("failed to start brewery-daemon")?;
        let client = Self {
            server: Arc::new(server),
        };

        match client.handshake(root).await {
            Ok(()) => Ok(client),
            Err(error) => {
                client.shutdown().await;
                Err(error)
            }
        }
    }

    async fn handshake(&self, root: &Path) -> Result<()> {
        let result = self
            .server
            .request::<Initialize>(
                InitializeParams {
                    protocol_version: PROTOCOL_VERSION,
                },
                HANDSHAKE_TIMEOUT,
            )
            .await
            .into_response()
            .context("brewery-daemon initialize failed")?;
        if result.protocol_version != PROTOCOL_VERSION {
            bail!(
                "brewery-daemon {} speaks protocol version {}, but this Brewery needs {}",
                result.daemon_version,
                result.protocol_version,
                PROTOCOL_VERSION
            );
        }
        self.server
            .request::<OpenProject>(
                OpenProjectParams {
                    root_dir: root.to_string_lossy().into_owned(),
                },
                HANDSHAKE_TIMEOUT,
            )
            .await
            .into_response()
            .context("brewery-daemon could not open the project")?;
        Ok(())
    }

    pub async fn project_model(&self) -> Result<ProjectModel> {
        self.server
            .request::<GetProjectModel>(ProjectModelParams {}, IMPORT_TIMEOUT)
            .await
            .into_response()
            .context("brewery-daemon could not read the project model")
    }

    /// Calls `f` whenever the daemon reports that build files changed. The handler is
    /// removed when the returned subscription is dropped.
    pub fn on_project_changed(&self, f: impl Fn() + Send + 'static) -> lsp::Subscription {
        self.server
            .on_notification::<ProjectChanged, _>(move |_, _| f())
    }

    pub async fn shutdown(self) {
        if let Some(shutdown) = self.server.shutdown() {
            shutdown.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    async fn daemon_reads_maven_model(cx: &mut TestAppContext) {
        let Some(jar) = std::env::var_os("BREWERY_DAEMON_JAR") else {
            eprintln!("skipping: BREWERY_DAEMON_JAR is not set");
            return;
        };
        assert!(
            Path::new(&jar).is_file(),
            "BREWERY_DAEMON_JAR is not a file"
        );
        // A real child process answers on its own schedule; the test executor must
        // not treat waiting for it as a deadlock.
        cx.executor().allow_parking();

        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../brewery-daemon/src/test/resources/projects/maven-multi")
            .canonicalize()
            .unwrap();
        let mut async_cx = cx.to_async();
        let client = DaemonClient::start(&root, &mut async_cx).await.unwrap();
        let model = client.project_model().await.unwrap();
        assert_eq!(model.build_tool, BuildTool::Maven);
        assert_eq!(model.modules.len(), 3);
        client.shutdown().await;
    }
}

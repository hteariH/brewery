//! Spring Boot configuration support (`application.yml`, `application.properties`).
//!
//! The daemon jar doubles as a language server (`--spring-config-lsp`) that completes,
//! documents and checks configuration keys against the project's Spring metadata. Zed
//! starts it like any other language server, so it only runs once a YAML or Properties
//! buffer is open, and the server itself ignores files that are not Spring config.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use brewery_daemon_client::{find_daemon_jar, find_java_with_env};
use futures::{FutureExt as _, lock::Mutex, lock::OwnedMutexGuard};
use gpui::{App, AsyncApp, BackgroundExecutor};
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

/// After a miss, discovery runs again at most this often.
const RETRY_WINDOW: Duration = Duration::from_secs(30);

/// A small, short-lived server: fast startup and a bounded heap matter more than peak
/// throughput.
const JVM_FLAGS: [&str; 3] = ["-XX:TieredStopAtLevel=1", "-XX:+UseSerialGC", "-Xmx512m"];

/// Looks for the JDK and daemon jar using the given environment.
type Discover = Arc<dyn Fn(&HashMap<String, String>) -> Result<LanguageServerBinary> + Send + Sync>;

/// Registers the Spring configuration server for YAML and Properties buffers.
pub fn init(languages: Arc<LanguageRegistry>, _cx: &mut App) {
    register(&languages, Arc::new(discover_installed));
}

fn register(languages: &LanguageRegistry, discover: Discover) {
    let adapter: Arc<dyn LspAdapter> =
        Arc::new(SpringConfigLspAdapter::new(discover, RETRY_WINDOW));
    for language in LANGUAGES {
        languages.register_lsp_adapter(LanguageName::new_static(language), adapter.clone());
    }
}

fn server_binary(java: PathBuf, jar: PathBuf) -> LanguageServerBinary {
    let mut arguments: Vec<OsString> = JVM_FLAGS.into_iter().map(OsString::from).collect();
    arguments.extend([
        OsString::from("-jar"),
        jar.into_os_string(),
        OsString::from("--spring-config-lsp"),
    ]);
    LanguageServerBinary {
        path: java,
        arguments,
        env: None,
    }
}

fn discover_installed(env: &HashMap<String, String>) -> Result<LanguageServerBinary> {
    discover(|| find_java_with_env(MIN_JAVA_MAJOR, env), find_daemon_jar)
}

fn discover(
    find_java: impl FnOnce() -> Result<PathBuf>,
    find_jar: impl FnOnce() -> Result<PathBuf>,
) -> Result<LanguageServerBinary> {
    let java = find_java()?;
    Ok(server_binary(java, find_jar()?))
}

/// Only successes are kept; a miss is looked up again after the retry window.
type Found = Option<LanguageServerBinary>;

/// Uses the locally found JDK and daemon jar; nothing is ever downloaded.
pub struct SpringConfigLspAdapter {
    discover: Discover,
    retry_window: Duration,
    /// Held across a lookup so concurrent worktrees share one attempt (and one warning).
    discovery: Mutex<Found>,
    /// When the last attempt missed, for the synchronous `is_opt_in_for`.
    last_miss: StdMutex<Option<Instant>>,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl SpringConfigLspAdapter {
    fn new(discover: Discover, retry_window: Duration) -> Self {
        Self::with_clock(discover, retry_window, Arc::new(Instant::now))
    }

    /// The clock is a seam so tests can move past the retry window.
    fn with_clock(
        discover: Discover,
        retry_window: Duration,
        now: Arc<dyn Fn() -> Instant + Send + Sync>,
    ) -> Self {
        Self {
            discover,
            retry_window,
            discovery: Mutex::new(None),
            last_miss: StdMutex::new(None),
            now,
        }
    }

    /// The last miss, if it is still inside the retry window.
    fn recent_miss(&self) -> bool {
        let last_miss = *self.last_miss.lock().unwrap_or_else(|e| e.into_inner());
        last_miss.is_some_and(|missed_at| {
            (self.now)().saturating_duration_since(missed_at) < self.retry_window
        })
    }

    fn set_miss(&self, miss: Option<Instant>) {
        *self.last_miss.lock().unwrap_or_else(|e| e.into_inner()) = miss;
    }

    /// Discovery runs when a server is about to start, with the worktree's shell
    /// environment: at app startup that environment may not be loaded yet, and
    /// SDKMAN/asdf only put `JAVA_HOME` and `PATH` there.
    async fn binary(
        &self,
        env: HashMap<String, String>,
        executor: &BackgroundExecutor,
    ) -> Result<LanguageServerBinary> {
        let mut discovery = self.discovery.lock().await;
        if let Some(binary) = &*discovery {
            return Ok(binary.clone());
        }
        if self.recent_miss() {
            return Err(Self::not_found());
        }
        // Discovery may run `java -version`, so it stays off the main thread.
        let discover = self.discover.clone();
        match executor.spawn(async move { discover(&env) }).await {
            Ok(binary) => {
                *discovery = Some(binary.clone());
                self.set_miss(None);
                Ok(binary)
            }
            Err(error) => {
                // One warning per miss window: attempts are rate limited above.
                log::warn!("Spring configuration completion is off: {error:#}");
                self.set_miss(Some((self.now)()));
                Err(Self::not_found())
            }
        }
    }

    fn not_found() -> anyhow::Error {
        anyhow!("no JDK {MIN_JAVA_MAJOR}+ or Brewery daemon jar found (see the log)")
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
        delegate: Arc<dyn LspAdapterDelegate>,
        _: Option<Toolchain>,
        _: LanguageServerBinaryOptions,
        _: OwnedMutexGuard<Option<(bool, LanguageServerBinary)>>,
        cx: AsyncApp,
    ) -> LanguageServerBinaryLocations {
        async move {
            let env = delegate.shell_env().await.into_iter().collect();
            (self.binary(env, cx.background_executor()).await, None)
        }
        .boxed_local()
    }
}

#[async_trait(?Send)]
impl LspAdapter for SpringConfigLspAdapter {
    fn name(&self) -> LanguageServerName {
        LanguageServerName::new_static(SERVER_NAME)
    }

    /// For the retry window after a miss the server is opt-in, so the default server
    /// list skips it instead of failing (and flagging an error) for every buffer and
    /// worktree. Not yet tried counts as available, so buffers restored at startup get
    /// the server; once the window passes it is listed again, so the next server-tree
    /// evaluation (another YAML/Properties buffer) retries discovery.
    fn is_opt_in_for(&self, _: &LanguageName) -> bool {
        self.recent_miss()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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

    fn yaml() -> LanguageName {
        LanguageName::new_static("YAML")
    }

    /// Fails `misses` times, then finds the binary; counts attempts.
    fn flaky(misses: usize) -> (Discover, Arc<AtomicUsize>) {
        let attempts = Arc::new(AtomicUsize::new(0));
        let discover: Discover = Arc::new({
            let attempts = attempts.clone();
            move |_| {
                if attempts.fetch_add(1, Ordering::SeqCst) < misses {
                    Err(anyhow!("no JDK 21"))
                } else {
                    Ok(fake_binary())
                }
            }
        });
        (discover, attempts)
    }

    #[test]
    fn binary_runs_the_daemon_jar_in_spring_config_mode() {
        let (path, arguments, has_env) = command(&fake_binary());
        assert_eq!(path, PathBuf::from("/jdk/bin/java"));
        assert_eq!(
            arguments,
            [
                "-XX:TieredStopAtLevel=1",
                "-XX:+UseSerialGC",
                "-Xmx512m",
                "-jar",
                "/brewery/daemon.jar",
                "--spring-config-lsp"
            ]
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
            .ok()
            .map(command),
            fake_command()
        );
        assert!(
            discover(
                || Err(anyhow!("no JDK 21")),
                || Ok(PathBuf::from("/brewery/daemon.jar"))
            )
            .is_err()
        );
        assert!(
            discover(
                || Ok(PathBuf::from("/jdk/bin/java")),
                || Err(anyhow!("no daemon jar"))
            )
            .is_err()
        );
    }

    #[test]
    fn discovery_reads_java_home_from_the_given_environment() {
        let root = std::env::temp_dir().join(format!("brewery-spring-jdk-{}", std::process::id()));
        let home = root.join("jdk-21");
        std::fs::create_dir_all(home.join("bin")).unwrap();
        let java = home
            .join("bin")
            .join(if cfg!(windows) { "java.exe" } else { "java" });
        std::fs::write(&java, b"").unwrap();
        std::fs::write(home.join("release"), "JAVA_VERSION=\"21.0.1\"\n").unwrap();
        let env = HashMap::from([("JAVA_HOME".to_string(), home.to_string_lossy().into_owned())]);
        let found = discover(
            || find_java_with_env(MIN_JAVA_MAJOR, &env),
            || Ok(PathBuf::from("/brewery/daemon.jar")),
        );
        assert_eq!(found.unwrap().path, java);
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test]
    async fn found_binary_is_served_to_every_worktree(cx: &mut TestAppContext) {
        let (discover, attempts) = flaky(0);
        let adapter = SpringConfigLspAdapter::new(discover, RETRY_WINDOW);
        let executor = cx.executor().clone();
        assert!(!adapter.is_opt_in_for(&yaml()));
        for _ in 0..2 {
            assert_eq!(
                adapter
                    .binary(HashMap::new(), &executor)
                    .await
                    .ok()
                    .as_ref()
                    .map(command),
                fake_command()
            );
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(!adapter.is_opt_in_for(&yaml()));
    }

    #[gpui::test]
    async fn a_miss_makes_the_server_opt_in_and_is_not_retried_within_the_window(
        cx: &mut TestAppContext,
    ) {
        let (discover, attempts) = flaky(usize::MAX);
        let adapter = SpringConfigLspAdapter::new(discover, RETRY_WINDOW);
        let executor = cx.executor().clone();
        assert!(!adapter.is_opt_in_for(&yaml()));
        assert!(adapter.binary(HashMap::new(), &executor).await.is_err());
        assert!(adapter.binary(HashMap::new(), &executor).await.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        // Opt-in means the default `...` server list skips it, so Zed never tries to
        // start it and never shows a failed-server indicator.
        assert!(adapter.is_opt_in_for(&yaml()));
        assert!(adapter.is_opt_in_for(&LanguageName::new_static("Properties")));
    }

    #[gpui::test]
    async fn a_success_after_the_retry_window_is_cached(cx: &mut TestAppContext) {
        let (discover, attempts) = flaky(1);
        let adapter = SpringConfigLspAdapter::new(discover, Duration::ZERO);
        let executor = cx.executor().clone();
        assert!(adapter.binary(HashMap::new(), &executor).await.is_err());
        assert_eq!(
            adapter
                .binary(HashMap::new(), &executor)
                .await
                .ok()
                .as_ref()
                .map(command),
            fake_command()
        );
        assert!(!adapter.is_opt_in_for(&yaml()));
        assert!(adapter.binary(HashMap::new(), &executor).await.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[gpui::test]
    async fn the_server_is_listed_again_once_the_retry_window_passes(cx: &mut TestAppContext) {
        let (discover, attempts) = flaky(1);
        let start = Instant::now();
        let offset_ms = Arc::new(AtomicU64::new(0));
        let clock = {
            let offset_ms = offset_ms.clone();
            Arc::new(move || start + Duration::from_millis(offset_ms.load(Ordering::SeqCst)))
        };
        let adapter = SpringConfigLspAdapter::with_clock(discover, RETRY_WINDOW, clock);
        let executor = cx.executor().clone();
        assert!(adapter.binary(HashMap::new(), &executor).await.is_err());
        assert!(adapter.is_opt_in_for(&yaml()));
        offset_ms.store(29_000, Ordering::SeqCst);
        assert!(adapter.is_opt_in_for(&yaml()));
        offset_ms.store(31_000, Ordering::SeqCst);
        assert!(!adapter.is_opt_in_for(&yaml()));
        assert!(adapter.binary(HashMap::new(), &executor).await.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[gpui::test]
    fn registers_for_yaml_and_properties(cx: &mut TestAppContext) {
        let languages = Arc::new(LanguageRegistry::new(cx.executor()));
        let (discover, _) = flaky(0);
        register(&languages, discover);
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

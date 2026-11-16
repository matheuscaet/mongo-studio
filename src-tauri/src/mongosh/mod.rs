//! The mongosh script engine: finds the MongoDB Shell on this machine (or
//! downloads MongoDB's official build into the app's data folder when the
//! user asks), and runs console scripts through it as a child process
//! instead of the built-in engine in `scripting`.

mod feed;
mod install;
mod script;

pub(crate) use script::describe_ms;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::assistant::detect::{display_path, find_in, is_executable, parse_version, search_dirs};
use crate::error::{AppError, AppResult};
use crate::models::ScriptResult;
use crate::state::{AppState, CancelToken};

/// Payload of the `mongosh-download` event.
pub const DOWNLOAD_EVENT: &str = "mongosh-download";

const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
/// The feed is only asked for the download size while nothing is
/// installed, so a slow network mustn't hold up the settings page.
const FEED_TIMEOUT: Duration = Duration::from_secs(5);
const FEED_TTL: Duration = Duration::from_secs(60 * 60);
const FEED_RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// Where the mongosh in use was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MongoshSource {
    /// The executable picked in settings.
    Custom,
    /// The copy Mongo Studio downloaded into its data folder.
    Downloaded,
    /// Found on the PATH (the login shell's, for a GUI launch).
    Path,
}

/// What `mongosh_detect` and `mongosh_download` report.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MongoshInfo {
    pub path: Option<String>,
    pub version: Option<String>,
    pub source: Option<MongoshSource>,
    /// Found but didn't run, or the download failed.
    pub error: Option<String>,
    /// Size in bytes of the archive a download would fetch, when known.
    pub download_size: Option<u64>,
}

/// How a script runs under mongosh, from the console settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MongoshRunOptions {
    /// The executable to run; `None` uses what `detect` finds.
    pub path: Option<String>,
    /// Extra command-line arguments, split like a shell would.
    #[serde(default)]
    pub extra_args: String,
    /// Load ~/.mongoshrc.js (off passes --norc).
    #[serde(default)]
    pub load_rc: bool,
}

/// The URI mongosh connects with for a live session: the session's saved
/// connection with its credentials, through the app's SSH tunnel when it
/// uses one, with its TLS files. Also returns the connection's name for log
/// lines.
pub async fn session_uri(state: &AppState, session_id: &str) -> AppResult<(String, String)> {
    let (profile_id, name, tunnel_port) = {
        let sessions = state.sessions.read().await;
        let active = sessions
            .get(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        (
            active.profile_id.clone(),
            active.name.clone(),
            active.tunnel.as_ref().map(|t| t.local_addr.port()),
        )
    };
    let profile = state.connection_store.get(&profile_id)?;
    let uri = crate::driver::shell_uri(&profile, state.secret_store.as_ref(), tunnel_port)?;
    Ok((uri, name))
}

/// The feed lookup `detect` reuses for a while.
struct CachedRelease {
    at: Instant,
    size: Option<u64>,
}

#[derive(Clone)]
pub struct Mongosh {
    /// Where downloaded builds are unpacked.
    data_dir: PathBuf,
    release_cache: Arc<Mutex<Option<CachedRelease>>>,
    /// One download at a time.
    downloading: Arc<Mutex<()>>,
}

impl Mongosh {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            release_cache: Arc::new(Mutex::new(None)),
            downloading: Arc::new(Mutex::new(())),
        }
    }

    /// The executable to use and where it came from: the custom path when
    /// it is an executable, else the downloaded copy, else the PATH.
    async fn locate(&self, custom_path: Option<&str>) -> Option<(PathBuf, MongoshSource)> {
        if let Some(custom) = custom_path.map(str::trim).filter(|p| !p.is_empty()) {
            let path = expand_home(custom);
            if is_executable(&path) {
                return Some((path, MongoshSource::Custom));
            }
        }
        if let Some(path) = install::installed_binary(&self.data_dir) {
            return Some((path, MongoshSource::Downloaded));
        }
        find_in(search_dirs().await, &[install::binary_name()]).map(|p| (p, MongoshSource::Path))
    }

    /// Looks for mongosh: the custom path first, then a downloaded copy,
    /// then the PATH.
    pub async fn detect(&self, custom_path: Option<String>) -> MongoshInfo {
        let Some((path, source)) = self.locate(custom_path.as_deref()).await else {
            return MongoshInfo {
                download_size: self.download_size().await,
                ..MongoshInfo::default()
            };
        };
        let (version, error) = match version_of(&path).await {
            Ok(version) => (Some(version), None),
            Err(error) => (None, Some(error)),
        };
        MongoshInfo {
            path: Some(display_path(&path)),
            version,
            source: Some(source),
            error,
            download_size: None,
        }
    }

    /// The size of the archive a download would fetch, looked up at most
    /// once an hour (every five minutes while the lookup fails).
    async fn download_size(&self) -> Option<u64> {
        let mut cache = self.release_cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            let ttl = if cached.size.is_some() {
                FEED_TTL
            } else {
                FEED_RETRY_AFTER
            };
            if cached.at.elapsed() < ttl {
                return cached.size;
            }
        }
        let size = async {
            let client = feed::http_client(Some(FEED_TIMEOUT)).ok()?;
            let release = feed::fetch_release(&client).await.ok()?;
            feed::archive_size(&client, &release.url).await
        }
        .await;
        *cache = Some(CachedRelease {
            at: Instant::now(),
            size,
        });
        size
    }

    /// Downloads MongoDB's build for this platform, checks its SHA-256,
    /// unpacks it into the data folder and reports it like `detect`.
    /// Progress goes out as `DOWNLOAD_EVENT` through `on_progress`.
    pub async fn download(
        &self,
        on_progress: impl Fn(serde_json::Value) + Send + Sync + 'static,
    ) -> AppResult<MongoshInfo> {
        let _one_at_a_time = self.downloading.lock().await;
        let client = feed::http_client(None)?;
        let release = feed::fetch_release(&client).await?;
        install::install(&self.data_dir, &client, &release, &on_progress).await?;
        Ok(self.detect(None).await)
    }

    /// Runs `script` against `database` through mongosh. `uri` connects
    /// with credentials and is handed over in the environment, never on the
    /// command line.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_script(
        &self,
        uri: String,
        database: String,
        script: String,
        options: MongoshRunOptions,
        timeout_ms: Option<u64>,
        cancel: CancelToken,
        on_log: impl Fn(String) + Send + Sync + 'static,
    ) -> AppResult<ScriptResult> {
        let (program, _) = self.locate(options.path.as_deref()).await.ok_or_else(|| {
            AppError::InvalidInput(
                "mongosh isn't installed: download it or pick its path in the console settings"
                    .to_string(),
            )
        })?;
        script::run(
            &program,
            uri,
            database,
            script,
            &options.extra_args,
            options.load_rc,
            timeout_ms,
            cancel,
            on_log,
        )
        .await
    }
}

/// `~/x` as a path in the home folder (`detect` shows paths that way).
fn expand_home(path: &str) -> PathBuf {
    let rest = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"));
    match (rest, crate::assistant::detect::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// `mongosh --version`, or why it didn't run.
async fn version_of(path: &Path) -> Result<String, String> {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW: no console window flashing up.
        command.creation_flags(0x0800_0000);
    }
    let output = tokio::time::timeout(VERSION_TIMEOUT, command.output())
        .await
        .map_err(|_| "`mongosh --version` didn't answer within 10 seconds".to_string())?
        .map_err(|e| format!("couldn't run it: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .chain(stdout.lines())
            .find(|l| !l.trim().is_empty());
        return Err(match detail {
            Some(line) => format!("`mongosh --version` failed: {}", line.trim()),
            None => format!("`mongosh --version` failed ({})", output.status),
        });
    }
    parse_version(&stdout).ok_or_else(|| "`mongosh --version` printed nothing".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_paths_are_expanded() {
        if let Some(home) = crate::assistant::detect::home_dir() {
            assert_eq!(expand_home("~/bin/mongosh"), home.join("bin/mongosh"));
        }
        assert_eq!(
            expand_home("/usr/bin/mongosh"),
            PathBuf::from("/usr/bin/mongosh")
        );
    }

    #[tokio::test]
    async fn a_custom_path_that_is_not_executable_is_passed_over() {
        let dir = std::env::temp_dir().join(format!(
            "mongo-studio-mongosh-locate-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mongosh = Mongosh::new(dir.clone());
        let missing = dir.join("nope").display().to_string();
        let found = mongosh.locate(Some(&missing)).await;
        assert!(found.is_none_or(|(_, source)| source != MongoshSource::Custom));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    mod shell_uri {
        use crate::driver::shell_uri;
        use crate::models::{
            AuthMechanism, ConnectionAdvancedOptions, ConnectionProfile, ConnectionSource,
            TlsOptions,
        };
        use crate::secrets::{InMemoryStore, SecretKind, SecretStore};

        fn profile(source: ConnectionSource) -> ConnectionProfile {
            ConnectionProfile {
                id: "p".to_string(),
                name: "p".to_string(),
                color: None,
                source,
                database: None,
                username: None,
                has_password: false,
                tls: TlsOptions::default(),
                ssh_tunnel: None,
                advanced: ConnectionAdvancedOptions::default(),
                created_at: String::new(),
                updated_at: String::new(),
            }
        }

        fn uri(uri: &str) -> ConnectionSource {
            ConnectionSource::Uri {
                uri: uri.to_string(),
            }
        }

        #[test]
        fn a_plain_uri_gets_the_app_name() {
            let p = profile(uri("mongodb://localhost:27017"));
            assert_eq!(
                shell_uri(&p, &InMemoryStore::new(), None).unwrap(),
                "mongodb://localhost:27017/?appName=mongo-studio"
            );
        }

        #[test]
        fn credentials_tls_and_advanced_options_go_in() {
            let secrets = InMemoryStore::new();
            secrets.set("p", SecretKind::Password, "p@ss w/rd").unwrap();
            secrets
                .set("p", SecretKind::TlsCertKeyPassphrase, "k&y")
                .unwrap();
            let mut p = profile(uri("mongodb://a:1,b:2/shop?retryWrites=false&appName=old"));
            p.username = Some("bob".to_string());
            p.has_password = true;
            p.tls = TlsOptions {
                enabled: true,
                ca_file: Some("/certs/ca file.pem".to_string()),
                cert_key_file: Some("C:\\certs\\me.pem".to_string()),
                cert_key_has_passphrase: true,
                allow_invalid_certificates: false,
                allow_invalid_hostnames: true,
            };
            p.advanced = ConnectionAdvancedOptions {
                app_name: Some("mine".to_string()),
                connect_timeout_ms: Some(5000),
                replica_set: Some("rs0".to_string()),
                auth_mechanism: Some(AuthMechanism::ScramSha256),
                auth_source: Some("admin".to_string()),
                ..Default::default()
            };
            assert_eq!(
                shell_uri(&p, &secrets, None).unwrap(),
                "mongodb://bob:p%40ss%20w%2Frd@a:1,b:2/shop?retryWrites=false&appName=mine\
                 &connectTimeoutMS=5000&replicaSet=rs0&authMechanism=SCRAM-SHA-256\
                 &authSource=admin&tls=true&tlsCAFile=%2Fcerts%2Fca%20file.pem\
                 &tlsCertificateKeyFile=C%3A%5Ccerts%5Cme.pem\
                 &tlsCertificateKeyFilePassword=k%26y&tlsAllowInvalidHostnames=true"
            );
        }

        #[test]
        fn a_missing_password_is_reported() {
            let mut p = profile(uri("mongodb://localhost"));
            p.username = Some("bob".to_string());
            p.has_password = true;
            assert!(shell_uri(&p, &InMemoryStore::new(), None).is_err());
        }

        #[test]
        fn a_tunnel_replaces_the_hosts_and_connects_directly() {
            let mut p = profile(ConnectionSource::Manual {
                host: "db.internal".to_string(),
                port: 27017,
                srv: false,
            });
            p.advanced.replica_set = Some("rs0".to_string());
            p.advanced.direct_connection = Some(false);
            assert_eq!(
                shell_uri(&p, &InMemoryStore::new(), Some(40123)).unwrap(),
                "mongodb://127.0.0.1:40123/?appName=mongo-studio&directConnection=true"
            );
        }

        #[test]
        fn an_srv_uri_through_a_tunnel_keeps_what_srv_implies() {
            let secrets = InMemoryStore::new();
            secrets.set("p", SecretKind::Password, "pw").unwrap();
            let mut p = profile(uri("mongodb+srv://cluster0.example.net/?retryWrites=true"));
            p.username = Some("bob".to_string());
            p.has_password = true;
            assert_eq!(
                shell_uri(&p, &secrets, Some(5000)).unwrap(),
                "mongodb://bob:pw@127.0.0.1:5000/?retryWrites=true&appName=mongo-studio\
                 &tls=true&authSource=admin&directConnection=true"
            );
            // without a tunnel the SRV URI stays as it is
            assert_eq!(
                shell_uri(&p, &secrets, None).unwrap(),
                "mongodb+srv://bob:pw@cluster0.example.net/?retryWrites=true&appName=mongo-studio"
            );
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::models::{ConnectionAdvancedOptions, ConnectionProfile, ConnectionSource};
    use crate::secrets::InMemoryStore;
    use std::sync::Mutex as StdMutex;

    /// Downloads mongosh into a temp folder and runs scripts through it
    /// against the server in `MONGO_STUDIO_TEST_URI` (database `shop` with
    /// an `orders` collection, as in the dev container):
    ///   MONGO_STUDIO_TEST_URI=mongodb://localhost:27017 \
    ///     cargo test --lib -- --ignored live_mongosh
    #[tokio::test]
    #[ignore]
    async fn live_mongosh_downloads_and_runs_scripts() {
        let base = std::env::var("MONGO_STUDIO_TEST_URI")
            .expect("set MONGO_STUDIO_TEST_URI to run this test");
        let dir = std::env::temp_dir().join(format!(
            "mongo-studio-mongosh-live-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mongosh = Mongosh::new(dir.clone());

        let before = mongosh.detect(None).await;
        println!("before the download: {before:?}");
        if before.source.is_none() {
            assert!(
                before.download_size.is_some_and(|n| n > 1_000_000),
                "{before:?}"
            );
        }

        let events = Arc::new(StdMutex::new(Vec::<serde_json::Value>::new()));
        let sink = events.clone();
        let info = mongosh
            .download(move |e| sink.lock().unwrap().push(e))
            .await
            .expect("download");
        println!("installed: {info:?}");
        assert_eq!(info.source, Some(MongoshSource::Downloaded));
        assert!(info.version.is_some(), "{info:?}");
        assert_eq!(info.error, None);
        let phases: Vec<String> = events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e["phase"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(phases.first().map(String::as_str), Some("download"));
        assert_eq!(phases.last().map(String::as_str), Some("done"));
        assert!(phases.contains(&"verify".to_string()) && phases.contains(&"unpack".to_string()));
        let files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(files.len(), 2, "a version folder and `current`: {files:?}");

        // the URI the app would build for this server, from a profile
        let profile = ConnectionProfile {
            id: "live-mongosh".to_string(),
            name: "live mongosh".to_string(),
            color: None,
            source: ConnectionSource::Uri { uri: base },
            database: None,
            username: None,
            has_password: false,
            tls: Default::default(),
            ssh_tunnel: None,
            advanced: ConnectionAdvancedOptions::default(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        let uri = crate::driver::shell_uri(&profile, &InMemoryStore::new(), None).unwrap();

        let run = |script: &str| {
            let logs = Arc::new(StdMutex::new(Vec::<String>::new()));
            let sink = logs.clone();
            let future = mongosh.run_script(
                uri.clone(),
                "shop".to_string(),
                script.to_string(),
                MongoshRunOptions::default(),
                Some(30_000),
                CancelToken::default(),
                move |line| sink.lock().unwrap().push(line),
            );
            async move { (future.await, logs.lock().unwrap().clone()) }
        };

        let (result, streamed) =
            run("print('hello', { a: 1 });\ndb.orders.find({}, { _id: 1 }).limit(3)").await;
        let result = result.expect("find");
        assert_eq!(result.database, "shop");
        assert_eq!(result.logs, vec![r#"hello {"a":1}"#.to_string()]);
        assert_eq!(streamed, result.logs);
        let docs = result.value.as_array().expect("documents");
        assert_eq!(docs.len(), 3);
        assert!(docs[0]["_id"]["$oid"].is_string(), "{:?}", result.value);

        let (result, _) = run("db.orders.countDocuments()").await;
        assert!(result.unwrap().value.is_u64());

        let (result, _) = run("use analytics\ndb.getName()").await;
        let result = result.unwrap();
        assert_eq!(result.value, serde_json::json!("analytics"));
        assert_eq!(result.database, "analytics");

        let (result, _) = run("show dbs").await;
        let dbs = result.unwrap().value;
        assert!(
            dbs.as_array().unwrap().iter().any(|d| d["name"] == "shop"),
            "{dbs}"
        );

        let (result, _) = run("const x = 1").await;
        assert_eq!(result.unwrap().value, serde_json::Value::Null);

        let (result, _) = run("await db.orders.findOne()").await;
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("invalid input: {}", script::AWAIT_HINT)
        );

        let (result, _) = run("throw new Error('boom')").await;
        assert_eq!(result.unwrap_err().to_string(), "invalid input: boom");

        let (result, _) = run("db.orders.find({ a: { $bad: 1 } }).toArray()").await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("MongoServerError[BadValue]"), "{err}");

        // the timeout and Cancel both stop the child
        let started = Instant::now();
        let err = mongosh
            .run_script(
                uri.clone(),
                "shop".to_string(),
                "while (true) {}".to_string(),
                MongoshRunOptions::default(),
                Some(3_000),
                CancelToken::default(),
                |_| {},
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("didn't finish"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));

        let cancel = CancelToken::default();
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            canceller.cancel();
        });
        let err = mongosh
            .run_script(
                uri.clone(),
                "shop".to_string(),
                "sleep(20000)".to_string(),
                MongoshRunOptions::default(),
                None,
                cancel,
                |_| {},
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Cancelled), "{err}");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

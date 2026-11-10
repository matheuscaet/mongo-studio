//! The MCP server for AI clients on this machine (Claude Code, Cursor,
//! Codex, VS Code...): streamable HTTP on 127.0.0.1 at a fixed port, one
//! bearer token kept in the secret store, and tools that browse and change
//! the user's saved connections within the grants set in Settings. Off until
//! the user turns it on. Separate from the Assistant's own read-only server
//! in `assistant::mcp`, which serves only the app's Assistant sessions.
//!
//! Pieces:
//! - `server` listens, checks Host/Origin and the token, keeps the client
//!   sessions and answers JSON-RPC.
//! - `policy` decides what a call may do (Off / Ask / Allow per capability,
//!   None / Read / Write per connection) and asks the user in the app.
//! - `tools` defines and runs the tools.
//! - `connections` keeps the server's own Mongo clients, one per saved
//!   connection, opened on demand and closed when idle.
//! - `clients` adds the server to the AI clients' config files.

mod clients;
mod connections;
mod policy;
mod server;
mod tools;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::relations::RelationsStore;
use crate::saved_scripts::SavedScriptsStore;
use crate::secrets::SecretKind;
use crate::state::AppState;

pub const REQUEST_EVENT: &str = "mcp-request";
pub const REQUEST_CLOSED_EVENT: &str = "mcp-request-closed";
pub const ACTIVITY_EVENT: &str = "mcp-activity";
pub const STATUS_EVENT: &str = "mcp-status";
/// The user chose "always" on a request: `{ connectionId, capability }`, to
/// be saved into `McpConfig.always` so the next config keeps it.
pub const ALWAYS_EVENT: &str = "mcp-always";
/// A client created, changed or deleted a saved connection:
/// `{ change, connectionId, name, folderId, level }`.
pub const CONNECTION_EVENT: &str = "mcp-connection";

/// The port used until the user picks another.
pub const DEFAULT_PORT: u16 = 47321;
/// The name the server goes by in the clients' configs and `serverInfo`.
pub(crate) const SERVER_NAME: &str = "mongo-studio";
/// The "connection id" the token is kept under in the secret store. Saved
/// connections have UUID ids, so this can't collide with one.
const TOKEN_SECRET_ID: &str = "mongo-studio-mcp-server";
/// How long a request waits for the user before it's refused.
const ASK_TIMEOUT: Duration = Duration::from_secs(120);

/// Off, ask the user each time, or allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    #[default]
    Off,
    Ask,
    Allow,
}

/// What a client may do on one saved connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    #[default]
    None,
    Read,
    Write,
}

/// How the app calls the user back when a request set to Ask waits
/// (Settings › MCP server). Both on unless the frontend says otherwise.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAlerts {
    /// A sound: the app's chime, or the desktop notification's.
    pub sound: bool,
    /// Bring the window forward, or ask for it with a notification.
    pub focus: bool,
}

impl Default for McpAlerts {
    fn default() -> Self {
        Self {
            sound: true,
            focus: true,
        }
    }
}

/// The settings the frontend pushes: whether the server runs, where, and
/// what clients may do. Capability ids: browse, read, write, ddl, script,
/// connList, connEdit, connDelete, saveScript, relRead, relEdit.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    pub enabled: bool,
    pub port: u16,
    #[serde(default)]
    pub caps: HashMap<String, Policy>,
    /// Per saved connection id; a connection missing here is `None`.
    #[serde(default)]
    pub connections: HashMap<String, Level>,
    /// Capabilities the user allowed for good on a connection by answering
    /// "always", keyed by connection id; `"*"` holds the ones that aren't
    /// about a connection (creating connections, saving scripts...).
    #[serde(default)]
    pub always: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub alerts: McpAlerts,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpClient {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub connected_at: u64,
    pub last_seen: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub running: bool,
    pub port: u16,
    pub url: String,
    /// Why it isn't running when it should (e.g. the port is taken).
    pub error: Option<String>,
    pub clients: Vec<McpClient>,
}

/// An AI client the server can be added to with one click.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTarget {
    /// claude | cursor | codex | vscode | windsurf
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub registered: bool,
    /// The file the entry lives in, shown to the user.
    pub config_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestChoice {
    Once,
    Always,
    Deny,
}

/// What the server needs from the app: a way to emit events to the windows
/// and the app's stores. A trait rather than a bare `AppHandle` so tests
/// can run the server without a Tauri app.
pub(crate) trait Host: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: Value);
    fn app_state(&self) -> &AppState;
    fn saved_scripts(&self) -> &SavedScriptsStore;
    /// The saved relations between collections.
    fn relations(&self) -> &RelationsStore;
    /// The sidebar's folder tree (the frontend's own JSON), or null.
    fn sidebar_layout(&self) -> Value;
    /// A request waits for the user: bring the window forward, or show a
    /// desktop notification that does. Done here rather than in the
    /// webview, which a minimized window may keep suspended.
    fn call_attention(&self, _alerts: McpAlerts, _title: &str, _body: &str) {}
    /// Nothing waits any more: take the notification away.
    fn dismiss_attention(&self) {}
}

impl Host for tauri::AppHandle {
    fn emit(&self, event: &str, payload: Value) {
        let _ = tauri::Emitter::emit(self, event, payload);
    }

    fn call_attention(&self, alerts: McpAlerts, title: &str, body: &str) {
        crate::attention::call_attention(self, alerts.focus, alerts.sound, title, body);
    }

    fn dismiss_attention(&self) {
        crate::attention::dismiss_attention();
    }

    fn app_state(&self) -> &AppState {
        use tauri::Manager;
        self.state::<AppState>().inner()
    }

    fn saved_scripts(&self) -> &SavedScriptsStore {
        use tauri::Manager;
        self.state::<SavedScriptsStore>().inner()
    }

    fn relations(&self) -> &RelationsStore {
        use tauri::Manager;
        self.state::<RelationsStore>().inner()
    }

    fn sidebar_layout(&self) -> Value {
        use tauri::Manager;
        self.try_state::<crate::sidebar_layout::SidebarLayoutStore>()
            .and_then(|s| s.get().ok())
            .unwrap_or(Value::Null)
    }
}

/// A client that initialized a session.
#[derive(Debug, Clone)]
pub(crate) struct ClientSession {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub connected_at: u64,
    pub last_seen: u64,
}

pub(crate) struct Inner {
    host: Arc<dyn Host>,
    config: Mutex<McpConfig>,
    /// Starts and stops happen one at a time.
    lifecycle: tokio::sync::Mutex<()>,
    listener: Mutex<Option<server::Listener>>,
    /// Why the listener isn't running although it's enabled.
    error: Mutex<Option<String>>,
    token: Mutex<Option<String>>,
    sessions: Mutex<HashMap<String, ClientSession>>,
    approvals: Mutex<HashMap<String, policy::Pending>>,
    pool: connections::Pool,
    ask_timeout: Duration,
    client_paths: clients::Paths,
}

impl Inner {
    fn new(host: Arc<dyn Host>, ask_timeout: Duration, client_paths: clients::Paths) -> Self {
        Self {
            host,
            config: Mutex::new(McpConfig::default()),
            lifecycle: tokio::sync::Mutex::new(()),
            listener: Mutex::new(None),
            error: Mutex::new(None),
            token: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            pool: connections::Pool::default(),
            ask_timeout,
            client_paths,
        }
    }

    fn emit<T: Serialize>(&self, event: &str, payload: &T) {
        if let Ok(value) = serde_json::to_value(payload) {
            self.host.emit(event, value);
        }
    }

    /// Calls the user back for a waiting request, as the settings say.
    fn call_attention(&self, title: &str, body: &str) {
        let alerts = self.config().alerts;
        if alerts.focus {
            self.host.call_attention(alerts, title, body);
        }
    }

    fn dismiss_attention(&self) {
        if self.approvals.lock().unwrap().is_empty() {
            self.host.dismiss_attention();
        }
    }

    fn config(&self) -> McpConfig {
        self.config.lock().unwrap().clone()
    }

    fn state(&self) -> &AppState {
        self.host.app_state()
    }

    /// The port clients reach: the listener's, else the configured one.
    fn port(&self) -> u16 {
        if let Some(listener) = self.listener.lock().unwrap().as_ref() {
            return listener.port;
        }
        match self.config.lock().unwrap().port {
            0 => DEFAULT_PORT,
            port => port,
        }
    }

    fn url(&self) -> String {
        server::url(self.port())
    }

    fn status(&self) -> McpStatus {
        let running = self.listener.lock().unwrap().is_some();
        let mut clients: Vec<McpClient> = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .map(|s| McpClient {
                id: s.id.clone(),
                name: s.name.clone(),
                version: s.version.clone(),
                connected_at: s.connected_at,
                last_seen: s.last_seen,
            })
            .collect();
        clients.sort_by_key(|c| c.connected_at);
        McpStatus {
            running,
            port: self.port(),
            url: self.url(),
            error: self.error.lock().unwrap().clone(),
            clients,
        }
    }

    fn emit_status(&self) {
        self.emit(STATUS_EVENT, &self.status());
    }

    /// The token, created and saved on first use.
    fn token(&self) -> AppResult<String> {
        let mut cached = self.token.lock().unwrap();
        if let Some(token) = cached.as_ref() {
            return Ok(token.clone());
        }
        let secrets = self.state().secret_store.as_ref();
        let token = match secrets
            .get(TOKEN_SECRET_ID, SecretKind::Password)
            .map_err(AppError::Secret)?
        {
            Some(token) if !token.is_empty() => token,
            _ => {
                let token = new_token();
                secrets
                    .set(TOKEN_SECRET_ID, SecretKind::Password, &token)
                    .map_err(AppError::Secret)?;
                log::info!("MCP server: created its access token");
                token
            }
        };
        *cached = Some(token.clone());
        Ok(token)
    }

    /// Replaces the token and forgets every client session.
    fn regenerate_token(&self) -> AppResult<String> {
        let token = new_token();
        self.state()
            .secret_store
            .set(TOKEN_SECRET_ID, SecretKind::Password, &token)
            .map_err(AppError::Secret)?;
        *self.token.lock().unwrap() = Some(token.clone());
        log::info!("MCP server: replaced its access token; every client was disconnected");
        let ids: Vec<String> = self.sessions.lock().unwrap().keys().cloned().collect();
        for id in &ids {
            self.forget_session(id);
        }
        self.emit_status();
        Ok(token)
    }

    /// Whether `presented` is the token, compared in constant time.
    fn token_matches(&self, presented: &str) -> bool {
        match self.token() {
            Ok(token) => constant_time_eq(token.as_bytes(), presented.as_bytes()),
            Err(e) => {
                log::warn!("MCP server: can't read its access token: {e}");
                false
            }
        }
    }

    /// Drops a client session and refuses its pending requests.
    fn forget_session(&self, id: &str) -> bool {
        let removed = self.sessions.lock().unwrap().remove(id).is_some();
        policy::close_client_requests(self, id);
        removed
    }
}

/// 32 random bytes, hex encoded.
fn new_token() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares without stopping at the first difference, so the time taken
/// doesn't tell how much of a guess was right.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Clone)]
pub struct McpServer {
    inner: Arc<Inner>,
}

impl McpServer {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self {
            inner: Arc::new(Inner::new(
                Arc::new(app),
                ASK_TIMEOUT,
                clients::Paths::from_env(),
            )),
        }
    }

    /// Applies new settings: starts, stops or restarts the listener.
    pub async fn set_config(&self, config: McpConfig) -> AppResult<McpStatus> {
        // Clients added at the old port would point nowhere after a port
        // change: they're rewritten with the new URL.
        let old_port = self.inner.config.lock().unwrap().port;
        let moved = config.enabled && old_port != 0 && config.port != old_port;
        let registered: Vec<String> = if moved {
            clients::targets(&self.inner)
                .await
                .into_iter()
                .filter(|t| t.registered)
                .map(|t| t.id)
                .collect()
        } else {
            Vec::new()
        };
        server::apply_config(&self.inner, config).await;
        for id in registered {
            if let Err(e) = clients::register(&self.inner, &id).await {
                log::warn!("MCP server: couldn't move {id} to the new port: {e}");
            }
        }
        Ok(self.inner.status())
    }

    pub fn status(&self) -> McpStatus {
        self.inner.status()
    }

    pub fn token(&self) -> AppResult<String> {
        self.inner.token()
    }

    pub fn regenerate_token(&self) -> AppResult<String> {
        let token = self.inner.regenerate_token()?;
        // The registered clients get the new token, so they keep working.
        let inner = self.inner.clone();
        tauri::async_runtime::spawn(async move {
            clients::reregister_all(&inner).await;
        });
        Ok(token)
    }

    pub fn answer(&self, request_id: &str, choice: RequestChoice) -> AppResult<()> {
        policy::answer(&self.inner, request_id, choice)
    }

    pub fn disconnect_client(&self, client_id: &str) -> AppResult<()> {
        if self.inner.forget_session(client_id) {
            log::info!("MCP server: disconnected client {client_id}");
            self.inner.emit_status();
        }
        Ok(())
    }

    pub async fn client_targets(&self) -> Vec<ClientTarget> {
        clients::targets(&self.inner).await
    }

    pub async fn register_client(&self, id: &str) -> AppResult<ClientTarget> {
        clients::register(&self.inner, id).await
    }

    pub async fn unregister_client(&self, id: &str) -> AppResult<ClientTarget> {
        clients::unregister(&self.inner, id).await
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::path::PathBuf;

    use crate::connection::ConnectionStore;
    use crate::models::SecretBackendKind;
    use crate::secrets::InMemoryStore;
    use crate::ssh_tunnel::KnownHosts;

    /// A host over stores in a temp folder, recording the events.
    pub struct TestHost {
        pub dir: PathBuf,
        pub events: Mutex<Vec<(String, Value)>>,
        pub state: AppState,
        pub scripts: SavedScriptsStore,
        pub relations: RelationsStore,
        pub layout: Mutex<Value>,
    }

    impl TestHost {
        pub fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("mongo-studio-mcp-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let state = AppState {
                connection_store: ConnectionStore::load(&dir).unwrap(),
                secret_store: Box::new(InMemoryStore::new()),
                secret_backend: SecretBackendKind::Keyring,
                known_hosts: Arc::new(KnownHosts::load(&dir).unwrap()),
                sessions: tokio::sync::RwLock::new(HashMap::new()),
                running_tasks: Mutex::new(HashMap::new()),
            };
            let scripts = SavedScriptsStore::load(&dir, &dir.join("home")).unwrap();
            let relations = RelationsStore::load(&dir).unwrap();
            Self {
                dir,
                events: Mutex::new(Vec::new()),
                state,
                scripts,
                relations,
                layout: Mutex::new(Value::Null),
            }
        }

        pub fn events_named(&self, name: &str) -> Vec<Value> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .collect()
        }
    }

    impl Drop for TestHost {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    impl Host for TestHost {
        fn emit(&self, event: &str, payload: Value) {
            self.events
                .lock()
                .unwrap()
                .push((event.to_string(), payload));
        }

        fn app_state(&self) -> &AppState {
            &self.state
        }

        fn saved_scripts(&self) -> &SavedScriptsStore {
            &self.scripts
        }

        fn relations(&self) -> &RelationsStore {
            &self.relations
        }

        fn sidebar_layout(&self) -> Value {
            self.layout.lock().unwrap().clone()
        }
    }

    /// An `Inner` over a fresh `TestHost`, with client configs under the
    /// host's folder and no Claude CLI.
    pub fn inner(ask_timeout: Duration) -> (Arc<TestHost>, Arc<Inner>) {
        let host = Arc::new(TestHost::new());
        let paths = clients::Paths::for_tests(&host.dir.join("home"));
        let inner = Arc::new(Inner::new(host.clone(), ask_timeout, paths));
        (host, inner)
    }

    /// Sets the config without starting a listener.
    pub fn set_policy(inner: &Inner, config: McpConfig) {
        *inner.config.lock().unwrap() = config;
    }

    /// Registers a client session, as `initialize` does.
    pub fn add_session(inner: &Inner, id: &str, name: &str) -> ClientSession {
        let session = ClientSession {
            id: id.to_string(),
            name: name.to_string(),
            version: Some("1.0".to_string()),
            connected_at: now_ms(),
            last_seen: now_ms(),
        };
        inner
            .sessions
            .lock()
            .unwrap()
            .insert(id.to_string(), session.clone());
        session
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::inner;
    use super::*;

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[tokio::test]
    async fn a_port_change_moves_the_registered_clients() {
        let (host, inner) = inner(ASK_TIMEOUT);
        let cursor = host.dir.join("home/.cursor");
        std::fs::create_dir_all(&cursor).unwrap();
        let server = McpServer { inner };
        let config = |port| McpConfig {
            enabled: true,
            port,
            ..McpConfig::default()
        };
        let (first, second) = (free_port(), free_port());
        server.set_config(config(first)).await.unwrap();
        server.register_client("cursor").await.unwrap();
        server.set_config(config(second)).await.unwrap();
        let file = std::fs::read_to_string(cursor.join("mcp.json")).unwrap();
        assert!(file.contains(&format!("127.0.0.1:{second}/mcp")), "{file}");
        let target = server.client_targets().await;
        assert!(target.iter().any(|t| t.id == "cursor" && t.registered));
        server.set_config(McpConfig::default()).await.unwrap();
    }

    #[test]
    fn the_token_is_created_once_and_kept_in_the_secret_store() {
        let (host, inner) = inner(ASK_TIMEOUT);
        let first = inner.token().unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(inner.token().unwrap(), first);
        assert_eq!(
            host.state
                .secret_store
                .get(TOKEN_SECRET_ID, SecretKind::Password)
                .unwrap()
                .as_deref(),
            Some(first.as_str())
        );
        // A fresh server over the same store reads the same token.
        *inner.token.lock().unwrap() = None;
        assert_eq!(inner.token().unwrap(), first);
        assert!(inner.token_matches(&first));
        assert!(!inner.token_matches("nope"));
    }

    #[test]
    fn regenerating_replaces_the_token_and_drops_the_clients() {
        let (host, inner) = inner(ASK_TIMEOUT);
        let first = inner.token().unwrap();
        test_support::add_session(&inner, "s1", "Claude Code");
        let second = inner.regenerate_token().unwrap();
        assert_ne!(first, second);
        assert!(!inner.token_matches(&first));
        assert!(inner.token_matches(&second));
        assert!(inner.status().clients.is_empty());
        assert_eq!(
            host.state
                .secret_store
                .get(TOKEN_SECRET_ID, SecretKind::Password)
                .unwrap(),
            Some(second)
        );
        assert!(!host.events_named(STATUS_EVENT).is_empty());
    }

    #[test]
    fn constant_time_compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn levels_are_ordered_and_config_parses_camel_case() {
        assert!(Level::None < Level::Read && Level::Read < Level::Write);
        let config: McpConfig = serde_json::from_value(serde_json::json!({
            "enabled": true, "port": 47321,
            "caps": { "read": "ask", "connEdit": "allow" },
            "connections": { "c1": "write" },
            "always": { "c1": ["write"] }
        }))
        .unwrap();
        assert_eq!(config.caps["connEdit"], Policy::Allow);
        assert_eq!(config.connections["c1"], Level::Write);
        assert_eq!(config.always["c1"], vec!["write".to_string()]);
    }
}

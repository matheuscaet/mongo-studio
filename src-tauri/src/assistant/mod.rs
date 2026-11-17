//! The Assistant: drives an AI coding agent CLI the user already has
//! installed and signed in (Claude Code or Codex) in headless mode, and gives
//! it read-only access to one MongoDB database through a small MCP server
//! this app runs on 127.0.0.1. The agent only ever proposes filters,
//! pipelines and console scripts as text; nothing it does can write, and
//! Mongo Studio never holds an API key - the CLI brings its own sign-in.
//!
//! Pieces:
//! - `detect` finds the CLIs (GUI apps don't inherit the login shell's PATH).
//! - `agents` spawns them, feeds them turns and turns their JSONL output into
//!   `assistant-event`s.
//! - `mcp`/`http` serve the tools over streamable HTTP, one bearer token per
//!   session, and `tools`/`schema` implement them against the session's
//!   database.
//! - `prompt` builds the system prompt.

mod agents;
pub(crate) mod detect;
pub(crate) mod http;
mod mcp;
mod models;
mod prompt;
mod schema;
mod tools;

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use mongodb::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub use detect::DetectedAgent;
// Shared with the MCP server for external clients (`crate::mcp_server`).
pub(crate) use schema::{infer as infer_schema, render as render_schema};
pub(crate) use tools::{
    bool_arg, doc_arg, int_arg, opt_doc_arg, opt_int_arg, opt_json_arg, pipeline_writes, plural,
    str_arg, summarize_explain, thousands, truncate,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
}

impl AgentKind {
    pub const ALL: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];

    pub fn display_name(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
        }
    }

    /// Who receives what the tools return, named in the approval prompt so
    /// the user knows where document values would go.
    pub fn provider(self) -> &'static str {
        match self {
            AgentKind::Claude => "Anthropic",
            AgentKind::Codex => "OpenAI",
        }
    }
}

/// What the agent may read, set by the frontend from its settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantPolicy {
    /// Index definitions and explain plans.
    pub indexes: bool,
    /// Document values on every allowed connection.
    pub values: bool,
    /// Connections with "Always allow values".
    #[serde(default)]
    pub values_connections: Vec<String>,
    /// Connections the Assistant may be used on at all.
    #[serde(default)]
    pub allowed_connections: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    /// The chat panel: prose plus proposals.
    Chat,
    /// An editor's inline prompt: exactly one fenced block.
    Inline,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantStartInput {
    pub agent: AgentKind,
    pub purpose: Purpose,
    pub connection_id: String,
    pub connection_name: String,
    /// The app's Mongo session id, a key of `AppState.sessions`.
    pub session_id: String,
    pub database: String,
    pub collection: Option<String>,
    pub server_version: Option<String>,
    /// The CLI's own session/thread id, to continue an earlier conversation.
    pub resume_id: Option<String>,
    /// The model to run; `None` uses the one set up in the CLI itself.
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning effort; `None` uses the CLI's configured or default one.
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantStarted {
    pub agent_session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalChoice {
    Once,
    Always,
    Deny,
}

/// Payload of the `assistant-event` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum AssistantEvent {
    /// The CLI reported its session (Claude) or thread (Codex) id.
    CliSession {
        agent_session_id: String,
        cli_session_id: String,
    },
    /// `text` is the whole block so far, not a delta; upserted by `block_id`.
    Text {
        agent_session_id: String,
        block_id: String,
        text: String,
        done: bool,
    },
    TurnEnd {
        agent_session_id: String,
        error: Option<String>,
        stopped: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepState {
    Run,
    Ok,
    Error,
    Denied,
}

/// Payload of the `assistant-step` event: one tool call, emitted when it
/// starts and again (same `step_id`) when it ends.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepEvent {
    pub agent_session_id: String,
    pub step_id: String,
    pub tool: String,
    pub state: StepState,
    pub label: String,
    pub meta: String,
    pub call: String,
    pub out: String,
}

/// Payload of the `assistant-approval` event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalEvent {
    pub agent_session_id: String,
    pub request_id: String,
    pub tool: String,
    pub database: String,
    pub collection: Option<String>,
    pub limit: Option<i64>,
    pub provider: String,
}

pub const EVENT: &str = "assistant-event";
pub const STEP_EVENT: &str = "assistant-step";
pub const APPROVAL_EVENT: &str = "assistant-approval";
pub const APPROVAL_CLOSED_EVENT: &str = "assistant-approval-closed";

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// What the Assistant needs from the app: a way to emit events to the
/// windows, and the Mongo client behind an app session. A trait rather than
/// a bare `AppHandle` so tests can drive the server and runners without a
/// Tauri app.
pub trait Host: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: Value);
    fn client(&self, mongo_session_id: String) -> BoxFuture<Option<Client>>;
}

impl Host for tauri::AppHandle {
    fn emit(&self, event: &str, payload: Value) {
        let _ = tauri::Emitter::emit(self, event, payload);
    }

    fn client(&self, mongo_session_id: String) -> BoxFuture<Option<Client>> {
        let app = self.clone();
        Box::pin(async move {
            use tauri::Manager;
            let state = app.state::<crate::state::AppState>();
            let sessions = state.sessions.read().await;
            sessions.get(&mongo_session_id).map(|a| a.client.clone())
        })
    }
}

/// One conversation with an agent, bound to one connection and database.
pub(crate) struct AgentSession {
    pub id: String,
    /// Bearer token the agent presents to the MCP server; it is what binds a
    /// tool call to this session's connection and database.
    pub token: String,
    pub agent: AgentKind,
    pub purpose: Purpose,
    pub connection_id: String,
    pub connection_name: String,
    pub mongo_session_id: String,
    pub database: String,
    pub collection: Option<String>,
    pub server_version: Option<String>,
    /// The CLI's session/thread id once known; the next process resumes it.
    pub cli_session_id: Mutex<Option<String>>,
    /// Passed to the CLI explicitly: its own configuration isn't loaded.
    pub model: Option<String>,
    pub effort: Option<String>,
    pub runner: tokio::sync::Mutex<agents::Runner>,
}

impl AgentSession {
    pub(crate) fn model_choice(&self) -> agents::ModelChoice<'_> {
        agents::ModelChoice {
            model: self.model.as_deref(),
            effort: self.effort.as_deref(),
        }
    }
}

struct PendingApproval {
    agent_session_id: String,
    tx: oneshot::Sender<ApprovalChoice>,
}

pub(crate) struct Inner {
    host: Arc<dyn Host>,
    workdir: PathBuf,
    policy: Mutex<AssistantPolicy>,
    sessions: Mutex<HashMap<String, Arc<AgentSession>>>,
    approvals: Mutex<HashMap<String, PendingApproval>>,
    /// Port of the MCP server once it's listening.
    server: tokio::sync::Mutex<Option<u16>>,
    /// Binaries found by the last detection, reused to spawn.
    binaries: Mutex<HashMap<AgentKind, PathBuf>>,
}

impl Inner {
    fn emit<T: Serialize>(&self, event: &str, payload: &T) {
        if let Ok(value) = serde_json::to_value(payload) {
            self.host.emit(event, value);
        }
    }

    fn policy(&self) -> AssistantPolicy {
        self.policy.lock().unwrap().clone()
    }

    fn session(&self, id: &str) -> AppResult<Arc<AgentSession>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::Assistant("This Assistant session has ended.".to_string()))
    }

    /// The live session a bearer token belongs to.
    fn session_by_token(&self, token: &str) -> Option<Arc<AgentSession>> {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .find(|s| constant_time_eq(s.token.as_bytes(), token.as_bytes()))
            .cloned()
    }

    fn is_live(&self, session: &AgentSession) -> bool {
        self.sessions.lock().unwrap().contains_key(&session.id)
    }

    /// Resolves every approval the session is waiting on as a denial and
    /// tells the windows to close their prompts.
    fn close_approvals(&self, agent_session_id: &str) {
        let closed: Vec<(String, PendingApproval)> = {
            let mut approvals = self.approvals.lock().unwrap();
            let ids: Vec<String> = approvals
                .iter()
                .filter(|(_, p)| p.agent_session_id == agent_session_id)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| approvals.remove(&id).map(|p| (id, p)))
                .collect()
        };
        for (request_id, pending) in closed {
            let _ = pending.tx.send(ApprovalChoice::Deny);
            self.emit(
                APPROVAL_CLOSED_EVENT,
                &serde_json::json!({ "requestId": request_id }),
            );
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Managed Tauri state for the Assistant commands.
#[derive(Clone)]
pub struct Assistant {
    inner: Arc<Inner>,
}

impl Assistant {
    /// `workdir` is where the agents run: an empty folder of the app's own,
    /// so they see none of the user's files.
    pub fn new(host: Arc<dyn Host>, workdir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                host,
                workdir,
                policy: Mutex::new(AssistantPolicy::default()),
                sessions: Mutex::new(HashMap::new()),
                approvals: Mutex::new(HashMap::new()),
                server: tokio::sync::Mutex::new(None),
                binaries: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn workdir(&self) -> AppResult<String> {
        std::fs::create_dir_all(&self.inner.workdir)?;
        Ok(self.inner.workdir.to_string_lossy().into_owned())
    }

    pub async fn detect(&self) -> Vec<DetectedAgent> {
        let detected = detect::detect_agents().await;
        for (_, agent, _) in &detected {
            match (&agent.path, &agent.version, &agent.error) {
                (None, _, _) => log::info!("{} not found", agent.name),
                (Some(path), _, Some(error)) => {
                    log::warn!("{} found at {path} but didn't run: {error}", agent.name)
                }
                (Some(path), version, None) => log::info!(
                    "{} {} found at {path}",
                    agent.name,
                    version.as_deref().unwrap_or("(version unknown)")
                ),
            }
        }
        let mut binaries = self.inner.binaries.lock().unwrap();
        for (kind, _, path) in &detected {
            match path {
                Some(path) => binaries.insert(*kind, path.clone()),
                None => binaries.remove(kind),
            };
        }
        detected.into_iter().map(|(_, agent, _)| agent).collect()
    }

    pub fn set_policy(&self, policy: AssistantPolicy) {
        *self.inner.policy.lock().unwrap() = policy;
    }

    pub async fn start(&self, input: AssistantStartInput) -> AppResult<AssistantStarted> {
        let inner = &self.inner;
        if !inner
            .policy()
            .allowed_connections
            .contains(&input.connection_id)
        {
            return Err(AppError::Assistant(
                "This connection isn't shared with the Assistant.".to_string(),
            ));
        }
        if input.database.is_empty() {
            return Err(AppError::InvalidInput("database is required".to_string()));
        }
        if inner.host.client(input.session_id.clone()).await.is_none() {
            return Err(AppError::SessionNotFound(input.session_id));
        }
        let configured = models::models_for(input.agent);
        let model = input
            .model
            .filter(|m| !m.is_empty())
            .or(configured.default_model);
        let effort = input
            .effort
            .filter(|e| !e.is_empty())
            .or(configured.default_effort);
        if model.as_deref().is_some_and(|m| !models::valid_model(m)) {
            return Err(AppError::InvalidInput(
                "that model name isn't valid".to_string(),
            ));
        }
        if effort.as_deref().is_some_and(|e| !models::valid_effort(e)) {
            return Err(AppError::InvalidInput(
                "that effort level isn't valid".to_string(),
            ));
        }
        std::fs::create_dir_all(&inner.workdir)?;
        mcp::ensure_server(inner).await?;

        let session = Arc::new(AgentSession {
            id: Uuid::new_v4().to_string(),
            // 244 random bits from two v4 UUIDs.
            token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            agent: input.agent,
            purpose: input.purpose,
            connection_id: input.connection_id,
            connection_name: input.connection_name,
            mongo_session_id: input.session_id,
            database: input.database,
            collection: input.collection.filter(|c| !c.is_empty()),
            server_version: input.server_version,
            cli_session_id: Mutex::new(input.resume_id.filter(|id| !id.is_empty())),
            model,
            effort,
            runner: tokio::sync::Mutex::new(agents::Runner::default()),
        });
        let agent_session_id = session.id.clone();
        log::info!(
            "Assistant conversation {agent_session_id} opened with {} on \"{}\" ({}), database {}",
            session.agent.display_name(),
            session.connection_name,
            session.connection_id,
            session.database
        );
        inner
            .sessions
            .lock()
            .unwrap()
            .insert(agent_session_id.clone(), session);
        Ok(AssistantStarted { agent_session_id })
    }

    pub async fn send(&self, agent_session_id: &str, text: String) -> AppResult<()> {
        let session = self.inner.session(agent_session_id)?;
        agents::send(&self.inner, &session, text).await
    }

    pub async fn stop(&self, agent_session_id: &str) -> AppResult<()> {
        let session = self.inner.session(agent_session_id)?;
        agents::stop(&self.inner, &session, false).await;
        Ok(())
    }

    pub async fn close(&self, agent_session_id: &str) -> AppResult<()> {
        let session = self.inner.sessions.lock().unwrap().remove(agent_session_id);
        if let Some(session) = session {
            agents::stop(&self.inner, &session, true).await;
            log::info!("Assistant conversation {agent_session_id} closed");
        }
        Ok(())
    }

    pub fn answer(&self, request_id: &str, choice: ApprovalChoice) -> AppResult<()> {
        let pending = self
            .inner
            .approvals
            .lock()
            .unwrap()
            .remove(request_id)
            .ok_or_else(|| AppError::Assistant("That request was already answered.".to_string()))?;
        if choice == ApprovalChoice::Always {
            let connection_id = self
                .inner
                .sessions
                .lock()
                .unwrap()
                .get(&pending.agent_session_id)
                .map(|s| s.connection_id.clone());
            if let Some(connection_id) = connection_id {
                let mut policy = self.inner.policy.lock().unwrap();
                if !policy.values_connections.contains(&connection_id) {
                    policy.values_connections.push(connection_id);
                }
            }
        }
        let _ = pending.tx.send(choice);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// A host that records events and hands out one fixed client.
    #[derive(Default)]
    pub struct RecordingHost {
        pub events: Mutex<Vec<(String, Value)>>,
        pub client: Option<Client>,
    }

    impl RecordingHost {
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

    impl Host for RecordingHost {
        fn emit(&self, event: &str, payload: Value) {
            self.events
                .lock()
                .unwrap()
                .push((event.to_string(), payload));
        }

        fn client(&self, _mongo_session_id: String) -> BoxFuture<Option<Client>> {
            let client = self.client.clone();
            Box::pin(async move { client })
        }
    }

    pub fn session(database: &str, connection_id: &str) -> Arc<AgentSession> {
        Arc::new(AgentSession {
            id: Uuid::new_v4().to_string(),
            token: Uuid::new_v4().simple().to_string(),
            agent: AgentKind::Claude,
            purpose: Purpose::Chat,
            connection_id: connection_id.to_string(),
            connection_name: "Local".to_string(),
            mongo_session_id: "mongo".to_string(),
            database: database.to_string(),
            collection: None,
            server_version: Some("8.0.19".to_string()),
            cli_session_id: Mutex::new(None),
            model: None,
            effort: None,
            runner: tokio::sync::Mutex::new(agents::Runner::default()),
        })
    }

    /// An Assistant around `host` with `session` registered.
    pub fn inner_with(
        host: Arc<RecordingHost>,
        session: &Arc<AgentSession>,
        policy: AssistantPolicy,
    ) -> Arc<Inner> {
        let assistant = Assistant::new(host, std::env::temp_dir().join("mongo-studio-assistant"));
        assistant.set_policy(policy);
        assistant
            .inner
            .sessions
            .lock()
            .unwrap()
            .insert(session.id.clone(), session.clone());
        assistant.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_to_the_frontend_shapes() {
        let event = AssistantEvent::CliSession {
            agent_session_id: "a".into(),
            cli_session_id: "c".into(),
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({ "type": "cli-session", "agentSessionId": "a", "cliSessionId": "c" })
        );
        let event = AssistantEvent::TurnEnd {
            agent_session_id: "a".into(),
            error: None,
            stopped: true,
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({ "type": "turn-end", "agentSessionId": "a", "error": null, "stopped": true })
        );
        let event = AssistantEvent::Text {
            agent_session_id: "a".into(),
            block_id: "b".into(),
            text: "hi".into(),
            done: false,
        };
        assert_eq!(
            serde_json::to_value(event).unwrap()["blockId"],
            serde_json::json!("b")
        );
    }

    #[test]
    fn policy_and_start_input_read_camel_case() {
        let policy: AssistantPolicy = serde_json::from_value(serde_json::json!({
            "indexes": true, "values": false,
            "valuesConnections": ["c1"], "allowedConnections": ["c1", "c2"]
        }))
        .unwrap();
        assert_eq!(policy.allowed_connections, ["c1", "c2"]);
        let input: AssistantStartInput = serde_json::from_value(serde_json::json!({
            "agent": "codex", "purpose": "inline", "connectionId": "c1",
            "connectionName": "Local", "sessionId": "s", "database": "shop",
            "collection": null, "serverVersion": "8.0.19", "resumeId": null
        }))
        .unwrap();
        assert_eq!(input.agent, AgentKind::Codex);
        assert_eq!(input.purpose, Purpose::Inline);
    }

    #[tokio::test]
    async fn answering_always_remembers_the_connection() {
        let host = Arc::new(test_support::RecordingHost::default());
        let session = test_support::session("shop", "c1");
        let inner = test_support::inner_with(host, &session, AssistantPolicy::default());
        let assistant = Assistant {
            inner: inner.clone(),
        };
        let (tx, rx) = oneshot::channel();
        inner.approvals.lock().unwrap().insert(
            "r1".into(),
            PendingApproval {
                agent_session_id: session.id.clone(),
                tx,
            },
        );
        assistant.answer("r1", ApprovalChoice::Always).unwrap();
        assert_eq!(rx.await.unwrap(), ApprovalChoice::Always);
        assert_eq!(inner.policy().values_connections, ["c1"]);
        assert!(assistant.answer("r1", ApprovalChoice::Once).is_err());
    }

    #[tokio::test]
    async fn closing_approvals_denies_them_and_tells_the_windows() {
        let host = Arc::new(test_support::RecordingHost::default());
        let session = test_support::session("shop", "c1");
        let inner = test_support::inner_with(host.clone(), &session, AssistantPolicy::default());
        let (tx, rx) = oneshot::channel();
        inner.approvals.lock().unwrap().insert(
            "r1".into(),
            PendingApproval {
                agent_session_id: session.id.clone(),
                tx,
            },
        );
        inner.close_approvals(&session.id);
        assert_eq!(rx.await.unwrap(), ApprovalChoice::Deny);
        assert_eq!(
            host.events_named(APPROVAL_CLOSED_EVENT),
            [serde_json::json!({ "requestId": "r1" })]
        );
    }
}

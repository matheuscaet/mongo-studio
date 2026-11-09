//! What a tool call may do, and asking the user when the answer is "Ask".
//!
//! Every call has a capability (browse, read, write...) the user set to
//! Off, Ask or Allow, and most are about one saved connection, which the
//! user set to None, Read or Read & write. A call goes through only when
//! both allow it; "Ask" shows a card in the app and waits for the answer.

use serde::Serialize;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::{
    now_ms, ClientSession, Inner, Level, McpConfig, Policy, RequestChoice, ALWAYS_EVENT,
    REQUEST_CLOSED_EVENT, REQUEST_EVENT,
};
use crate::error::{AppError, AppResult};

/// The `always` key of capabilities that aren't about one connection.
pub(crate) const ANY_CONNECTION: &str = "*";

pub(crate) const REFUSED_OFF: &str = "This is turned off in Mongo Studio's settings.";
pub(crate) const REFUSED_NO_ACCESS: &str = "No access to this connection in Mongo Studio.";
pub(crate) const DENIED: &str = "The user denied this in Mongo Studio.";
pub(crate) const NOT_ANSWERED: &str = "Nobody answered in Mongo Studio, so this wasn't done.";
pub(crate) const CLOSED: &str = "The request was closed in Mongo Studio, so this wasn't done.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cap {
    Browse,
    Read,
    Write,
    Ddl,
    Script,
    ConnList,
    ConnEdit,
    ConnDelete,
    SaveScript,
    /// List, read and join through the saved relations.
    RelRead,
    /// Propose, change and delete draft relations, and sample collections
    /// for link suggestions.
    RelEdit,
}

impl Cap {
    /// The id the frontend uses in `McpConfig.caps`.
    pub fn id(self) -> &'static str {
        match self {
            Cap::Browse => "browse",
            Cap::Read => "read",
            Cap::Write => "write",
            Cap::Ddl => "ddl",
            Cap::Script => "script",
            Cap::ConnList => "connList",
            Cap::ConnEdit => "connEdit",
            Cap::ConnDelete => "connDelete",
            Cap::SaveScript => "saveScript",
            Cap::RelRead => "relRead",
            Cap::RelEdit => "relEdit",
        }
    }
}

/// The policy set for a capability; browsing is always allowed.
pub(crate) fn cap_policy(config: &McpConfig, cap: Cap) -> Policy {
    match cap {
        Cap::Browse => Policy::Allow,
        _ => config.caps.get(cap.id()).copied().unwrap_or(Policy::Off),
    }
}

pub(crate) fn level_of(config: &McpConfig, connection_id: &str) -> Level {
    config
        .connections
        .get(connection_id)
        .copied()
        .unwrap_or(Level::None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    /// The capability is turned off.
    Off,
    /// The connection's level is below what the call needs.
    NoAccess,
    Ask,
    Allow,
    /// Allowed because the user answered "always" earlier.
    AllowAlways,
}

/// What a call with capability `cap` on `connection` (needing `need` on it)
/// may do under `config`.
pub(crate) fn decide(
    config: &McpConfig,
    cap: Cap,
    connection: Option<&str>,
    need: Level,
) -> Decision {
    let policy = cap_policy(config, cap);
    if policy == Policy::Off {
        return Decision::Off;
    }
    if let Some(id) = connection {
        if level_of(config, id) < need {
            return Decision::NoAccess;
        }
    }
    if policy == Policy::Allow {
        return Decision::Allow;
    }
    let key = connection.unwrap_or(ANY_CONNECTION);
    let always = config
        .always
        .get(key)
        .is_some_and(|caps| caps.iter().any(|c| c == cap.id()));
    if always {
        Decision::AllowAlways
    } else {
        Decision::Ask
    }
}

/// A question waiting for the user.
pub(crate) struct Pending {
    client_id: String,
    tx: oneshot::Sender<RequestChoice>,
}

/// What the approval card shows.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub tool: &'static str,
    pub capability: Cap,
    pub connection_id: Option<String>,
    pub connection_name: Option<String>,
    pub database: Option<String>,
    pub collection: Option<String>,
    pub title: String,
    pub code: String,
    pub dry_run: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestEvent<'a> {
    request_id: &'a str,
    client_id: &'a str,
    client_name: &'a str,
    tool: &'a str,
    capability: &'a str,
    connection_id: Option<&'a str>,
    connection_name: Option<&'a str>,
    database: Option<&'a str>,
    collection: Option<&'a str>,
    title: &'a str,
    code: &'a str,
    dry_run: Option<&'a str>,
    at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    Once,
    Always,
    Deny,
    Timeout,
    /// The client was disconnected or the server stopped meanwhile.
    Closed,
}

fn emit_closed(inner: &Inner, request_id: &str, reason: &str) {
    inner.emit(
        REQUEST_CLOSED_EVENT,
        &serde_json::json!({ "requestId": request_id, "reason": reason }),
    );
}

/// Shows the request in the app and waits for the answer, up to the
/// server's timeout. "Always" is also remembered in the config right away.
pub(crate) async fn ask(inner: &Inner, client: &ClientSession, request: &Request) -> Answer {
    let request_id = Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    inner.approvals.lock().unwrap().insert(
        request_id.clone(),
        Pending {
            client_id: client.id.clone(),
            tx,
        },
    );
    inner.emit(
        REQUEST_EVENT,
        &RequestEvent {
            request_id: &request_id,
            client_id: &client.id,
            client_name: &client.name,
            tool: request.tool,
            capability: request.capability.id(),
            connection_id: request.connection_id.as_deref(),
            connection_name: request.connection_name.as_deref(),
            database: request.database.as_deref(),
            collection: request.collection.as_deref(),
            title: &request.title,
            code: &request.code,
            dry_run: request.dry_run.as_deref(),
            at: now_ms(),
        },
    );
    let target = match (&request.connection_name, &request.database) {
        (Some(conn), _) => format!(" on {conn}"),
        _ => String::new(),
    };
    inner.call_attention(
        &format!("{} is waiting for you", client.name),
        &format!(
            "{}{target}. Allow or deny it in Mongo Studio.",
            request.title
        ),
    );
    let answer = wait_for_answer(inner, request, &request_id, rx).await;
    inner.dismiss_attention();
    answer
}

async fn wait_for_answer(
    inner: &Inner,
    request: &Request,
    request_id: &str,
    rx: oneshot::Receiver<RequestChoice>,
) -> Answer {
    match tokio::time::timeout(inner.ask_timeout, rx).await {
        Ok(Ok(RequestChoice::Once)) => Answer::Once,
        Ok(Ok(RequestChoice::Always)) => {
            let key = request
                .connection_id
                .clone()
                .unwrap_or_else(|| ANY_CONNECTION.to_string());
            remember_always(inner, &key, request.capability);
            Answer::Always
        }
        Ok(Ok(RequestChoice::Deny)) => Answer::Deny,
        Ok(Err(_)) => Answer::Closed,
        Err(_) => {
            // Gone already when an answer raced the timeout.
            if inner.approvals.lock().unwrap().remove(request_id).is_some() {
                emit_closed(inner, request_id, "timeout");
            }
            Answer::Timeout
        }
    }
}

/// Allows `cap` on `key` from now on, and tells the frontend to save it.
fn remember_always(inner: &Inner, key: &str, cap: Cap) {
    {
        let mut config = inner.config.lock().unwrap();
        let caps = config.always.entry(key.to_string()).or_default();
        if !caps.iter().any(|c| c == cap.id()) {
            caps.push(cap.id().to_string());
        }
    }
    inner.emit(
        ALWAYS_EVENT,
        &serde_json::json!({ "connectionId": key, "capability": cap.id() }),
    );
}

/// The user's answer to a request.
pub(crate) fn answer(inner: &Inner, request_id: &str, choice: RequestChoice) -> AppResult<()> {
    let pending = inner.approvals.lock().unwrap().remove(request_id);
    let Some(pending) = pending else {
        return Err(AppError::NotFound(
            "this request was already answered or has expired".to_string(),
        ));
    };
    emit_closed(inner, request_id, "answered");
    // The waiting call may have just ended; nothing then waits for this.
    let _ = pending.tx.send(choice);
    Ok(())
}

/// Refuses the pending requests of a client that went away.
pub(crate) fn close_client_requests(inner: &Inner, client_id: &str) {
    let closed: Vec<String> = {
        let mut approvals = inner.approvals.lock().unwrap();
        let ids: Vec<String> = approvals
            .iter()
            .filter(|(_, p)| p.client_id == client_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            approvals.remove(id);
        }
        ids
    };
    for id in closed {
        emit_closed(inner, &id, "disconnected");
    }
}

/// Refuses every pending request, when the server stops.
pub(crate) fn close_all_requests(inner: &Inner) {
    let closed: Vec<String> = inner
        .approvals
        .lock()
        .unwrap()
        .drain()
        .map(|(id, _)| id)
        .collect();
    for id in closed {
        emit_closed(inner, &id, "disconnected");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::Value;

    use super::super::test_support::{add_session, inner};
    use super::*;

    pub(crate) fn request_id_of(event: &Value) -> String {
        event["requestId"].as_str().unwrap_or_default().to_string()
    }

    fn config(caps: &[(&str, Policy)], connections: &[(&str, Level)]) -> McpConfig {
        McpConfig {
            enabled: true,
            port: 0,
            caps: caps.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            connections: connections
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
            always: HashMap::new(),
            alerts: Default::default(),
        }
    }

    #[test]
    fn decisions() {
        let mut c = config(
            &[
                ("read", Policy::Allow),
                ("write", Policy::Ask),
                ("ddl", Policy::Off),
            ],
            &[("c1", Level::Write), ("c2", Level::Read)],
        );
        // Off wins over everything.
        assert_eq!(
            decide(&c, Cap::Ddl, Some("c1"), Level::Write),
            Decision::Off
        );
        // A capability missing from the config is off, except browse.
        assert_eq!(
            decide(&c, Cap::Script, Some("c1"), Level::Write),
            Decision::Off
        );
        assert_eq!(
            decide(&c, Cap::Browse, Some("c2"), Level::Read),
            Decision::Allow
        );
        // The connection's level.
        assert_eq!(
            decide(&c, Cap::Browse, Some("c3"), Level::Read),
            Decision::NoAccess
        );
        assert_eq!(
            decide(&c, Cap::Write, Some("c2"), Level::Write),
            Decision::NoAccess
        );
        assert_eq!(
            decide(&c, Cap::Read, Some("c2"), Level::Read),
            Decision::Allow
        );
        assert_eq!(
            decide(&c, Cap::Write, Some("c1"), Level::Write),
            Decision::Ask
        );
        // "Always" is per connection.
        c.always.insert("c1".into(), vec!["write".into()]);
        assert_eq!(
            decide(&c, Cap::Write, Some("c1"), Level::Write),
            Decision::AllowAlways
        );
        c.connections.insert("c3".into(), Level::Write);
        assert_eq!(
            decide(&c, Cap::Write, Some("c3"), Level::Write),
            Decision::Ask
        );
        // Connection-less capabilities use the "*" key.
        c.caps.insert("saveScript".into(), Policy::Ask);
        assert_eq!(
            decide(&c, Cap::SaveScript, None, Level::None),
            Decision::Ask
        );
        c.always.insert("*".into(), vec!["saveScript".into()]);
        assert_eq!(
            decide(&c, Cap::SaveScript, None, Level::None),
            Decision::AllowAlways
        );
    }

    fn request() -> Request {
        Request {
            tool: "update_documents",
            capability: Cap::Write,
            connection_id: Some("c1".into()),
            connection_name: Some("Local".into()),
            database: Some("shop".into()),
            collection: Some("users".into()),
            title: "updateMany on shop.users".into(),
            code: "db.getCollection(\"users\").updateMany({}, {})".into(),
            dry_run: Some("12 documents would change".into()),
        }
    }

    async fn wait_for_request(host: &super::super::test_support::TestHost) -> Value {
        for _ in 0..200 {
            if let Some(event) = host.events_named(REQUEST_EVENT).pop() {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("no request event");
    }

    #[tokio::test]
    async fn an_unanswered_request_times_out() {
        let (host, inner) = inner(Duration::from_millis(50));
        let client = add_session(&inner, "s1", "Cursor");
        let answer = ask(&inner, &client, &request()).await;
        assert_eq!(answer, Answer::Timeout);
        let event = host.events_named(REQUEST_EVENT).pop().unwrap();
        assert_eq!(event["clientName"], "Cursor");
        assert_eq!(event["capability"], "write");
        assert_eq!(event["dryRun"], "12 documents would change");
        let closed = host.events_named(REQUEST_CLOSED_EVENT).pop().unwrap();
        assert_eq!(closed["reason"], "timeout");
        assert_eq!(closed["requestId"], event["requestId"]);
        assert!(inner.approvals.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn answers_reach_the_waiting_call() {
        let (host, inner) = inner(Duration::from_secs(5));
        let client = add_session(&inner, "s1", "Cursor");

        for (choice, expected) in [
            (RequestChoice::Once, Answer::Once),
            (RequestChoice::Deny, Answer::Deny),
        ] {
            host.events.lock().unwrap().clear();
            let waiting = {
                let inner = inner.clone();
                let client = client.clone();
                tokio::spawn(async move { ask(&inner, &client, &request()).await })
            };
            let id = request_id_of(&wait_for_request(&host).await);
            answer(&inner, &id, choice).unwrap();
            assert_eq!(waiting.await.unwrap(), expected);
            let closed = host.events_named(REQUEST_CLOSED_EVENT).pop().unwrap();
            assert_eq!(closed["reason"], "answered");
            // A second answer finds nothing.
            assert!(answer(&inner, &id, choice).is_err());
        }
    }

    #[tokio::test]
    async fn always_is_remembered_and_announced() {
        let (host, inner) = inner(Duration::from_secs(5));
        let client = add_session(&inner, "s1", "Cursor");
        let waiting = {
            let inner = inner.clone();
            tokio::spawn(async move { ask(&inner, &client, &request()).await })
        };
        let id = request_id_of(&wait_for_request(&host).await);
        answer(&inner, &id, RequestChoice::Always).unwrap();
        assert_eq!(waiting.await.unwrap(), Answer::Always);
        let always = host.events_named(ALWAYS_EVENT).pop().unwrap();
        assert_eq!(always["connectionId"], "c1");
        assert_eq!(always["capability"], "write");
        assert_eq!(inner.config().always["c1"], vec!["write".to_string()]);
    }

    #[tokio::test]
    async fn disconnecting_a_client_closes_its_requests() {
        let (host, inner) = inner(Duration::from_secs(5));
        let client = add_session(&inner, "s1", "Cursor");
        let waiting = {
            let inner: Arc<Inner> = inner.clone();
            tokio::spawn(async move { ask(&inner, &client, &request()).await })
        };
        wait_for_request(&host).await;
        assert!(inner.forget_session("s1"));
        assert_eq!(waiting.await.unwrap(), Answer::Closed);
        let closed = host.events_named(REQUEST_CLOSED_EVENT).pop().unwrap();
        assert_eq!(closed["reason"], "disconnected");
    }
}

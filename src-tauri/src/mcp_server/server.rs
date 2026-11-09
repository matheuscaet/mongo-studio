//! The listener and the JSON-RPC: streamable HTTP (JSON-RPC 2.0 over POST,
//! answered with plain JSON - no SSE stream) on 127.0.0.1 at the port set
//! in Settings. The HTTP pieces and the Host/Origin checks against DNS
//! rebinding are the Assistant's (`assistant::http`).
//!
//! A client starts with `initialize`, which needs only the token and gets an
//! `Mcp-Session-Id` back; every other request must carry that id. A session
//! the user disconnected answers 404, which tells the client (per the MCP
//! spec) to initialize again.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

use super::{connections, now_ms, policy, tools, ClientSession, Inner, Level, McpConfig};
use crate::assistant::http::{
    self, bearer, host_allowed, origin_allowed, Head, ReadError, Response,
};
use crate::state::CancelToken;

pub(crate) const PATH: &str = "/mcp";
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";
const SESSION_HEADER: &str = "Mcp-Session-Id";
/// How often idle Mongo clients are looked for.
const SWEEP_EVERY: Duration = Duration::from_secs(60);
/// `lastSeen` is pushed to the windows at most this often per client.
const LAST_SEEN_EVERY_MS: u64 = 15_000;

/// What `initialize` tells an agent about this server.
pub(crate) const INSTRUCTIONS: &str = "This server is Mongo Studio, a MongoDB client app on the \
user's computer. It gives access to the user's MongoDB connections as saved in Mongo Studio. \
Start with list_connections to see the connections you may use and their ids (if it isn't \
among your tools, the user turned off seeing connections: ask them); then use \
list_databases, list_collections, sample_schema (field names and types) and list_indexes to \
learn the data before querying it with find, count, aggregate or explain. Every tool that \
works on data takes a connection_id from list_connections. The user decides in Mongo Studio's \
settings what you may do: some tools may be turned off, each connection is read-only or \
read & write, and some calls wait until the user approves them in the app (up to two minutes) \
- when that happens, tell the user to look at Mongo Studio. A refusal means the user hasn't \
allowed it; don't retry, ask them. Passwords are never available through this server: \
connections you create are saved without one, and the user adds it in the app. Relations are \
the user's saved links between collections in different databases or connections (MongoDB's \
$lookup can't cross databases): list_relations shows them and find_related joins through one, \
with counts and sums per document. suggest_relation_links finds how two collections link from \
samples; relations you create or update are drafts the user reviews in the app before anything \
uses them.";

/// A running listener.
pub(crate) struct Listener {
    pub port: u16,
    shutdown: CancelToken,
}

pub(crate) fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}{PATH}")
}

/// Stores the config and starts, stops or restarts the listener. Only a
/// change of `enabled` or `port` touches the listener, so answering
/// "always" (which comes back as a new config) keeps the clients connected.
pub(crate) async fn apply_config(inner: &Arc<Inner>, config: McpConfig) {
    let _lifecycle = inner.lifecycle.lock().await;
    let old_port = inner.config.lock().unwrap().port;
    let enabled = config.enabled;
    let port = config.port;
    let levels = config.connections.clone();
    *inner.config.lock().unwrap() = config;

    // Clients of connections no longer shared are closed now.
    inner
        .pool
        .retain(
            |id| levels.get(id).copied().unwrap_or(Level::None) > Level::None,
            "the connection is no longer shared",
        )
        .await;

    let current = inner.listener.lock().unwrap().as_ref().map(|l| l.port);
    let running = current.is_some();
    // Port 0 (tests) binds any free port; asking for it again isn't a change.
    let needs_restart = current.is_none_or(|p| port != p && port != old_port);
    if !enabled {
        if running || inner.error.lock().unwrap().is_some() {
            stop(inner).await;
            *inner.error.lock().unwrap() = None;
            inner.emit_status();
        }
        return;
    }
    if !needs_restart {
        return;
    }
    if running {
        stop(inner).await;
    }
    start(inner, port).await;
    inner.emit_status();
}

async fn start(inner: &Arc<Inner>, port: u16) {
    let listener = match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(e) => {
            let message = if e.kind() == std::io::ErrorKind::AddrInUse {
                format!("Port {port} is already in use by another program. Pick another port.")
            } else {
                format!("Couldn't listen on port {port}: {e}")
            };
            log::warn!("MCP server: {message}");
            *inner.error.lock().unwrap() = Some(message);
            return;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    // Created now, so the first client doesn't wait on the keychain.
    if let Err(e) = inner.token() {
        log::warn!("MCP server: can't read its access token: {e}");
    }
    let shutdown = CancelToken::default();
    *inner.error.lock().unwrap() = None;
    *inner.listener.lock().unwrap() = Some(Listener {
        port,
        shutdown: shutdown.clone(),
    });
    log::info!("MCP server listening on 127.0.0.1:{port}");

    let accept_inner = inner.clone();
    let accept_shutdown = shutdown.clone();
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                () = accept_shutdown.cancelled() => return,
            };
            let Ok((stream, _)) = accepted else {
                continue;
            };
            tokio::spawn(serve_connection(
                accept_inner.clone(),
                stream,
                port,
                accept_shutdown.clone(),
            ));
        }
    });

    let sweep_inner = inner.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = tokio::time::sleep(SWEEP_EVERY) => {}
                () = shutdown.cancelled() => return,
            }
            sweep_inner.pool.close_idle(connections::IDLE_CLOSE).await;
        }
    });
}

/// Stops listening, drops every client session and closes the Mongo
/// clients.
async fn stop(inner: &Arc<Inner>) {
    let listener = inner.listener.lock().unwrap().take();
    if let Some(listener) = listener {
        listener.shutdown.cancel();
        log::info!("MCP server stopped listening on port {}", listener.port);
    }
    inner.sessions.lock().unwrap().clear();
    policy::close_all_requests(inner);
    inner.pool.close_all("the MCP server stopped").await;
}

async fn serve_connection(inner: Arc<Inner>, stream: TcpStream, port: u16, shutdown: CancelToken) {
    let _ = stream.set_nodelay(true);
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    loop {
        let head = tokio::select! {
            head = tokio::time::timeout(IDLE_TIMEOUT, http::read_head(&mut reader)) => head,
            () = shutdown.cancelled() => return,
        };
        let head = match head {
            Err(_) | Ok(Err(ReadError::Closed)) | Ok(Err(ReadError::Io)) => return,
            Ok(Err(e)) => {
                let _ = write
                    .write_all(&read_error_response(e).to_bytes(false))
                    .await;
                return;
            }
            Ok(Ok(head)) => head,
        };
        if head.expects_continue()
            && write
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .await
                .is_err()
        {
            return;
        }
        let body = match http::read_body(&mut reader, &head).await {
            Ok(body) => body,
            Err(ReadError::Closed | ReadError::Io) => return,
            Err(e) => {
                let _ = write
                    .write_all(&read_error_response(e).to_bytes(false))
                    .await;
                return;
            }
        };
        let keep_alive = head.keep_alive();
        let response = tokio::select! {
            response = handle(&inner, port, &head, &body) => response,
            () = shutdown.cancelled() => return,
        };
        if write
            .write_all(&response.to_bytes(keep_alive))
            .await
            .is_err()
            || !keep_alive
        {
            return;
        }
    }
}

fn read_error_response(error: ReadError) -> Response {
    match error {
        ReadError::TooLarge => Response::text(413, "request too large"),
        ReadError::Malformed(why) => Response::text(400, why),
        ReadError::Closed | ReadError::Io => Response::empty(400),
    }
}

pub(crate) async fn handle(inner: &Arc<Inner>, port: u16, head: &Head, body: &[u8]) -> Response {
    if !host_allowed(head.header("host"), port) || !origin_allowed(head.header("origin"), port) {
        return Response::text(403, "forbidden");
    }
    if head.path != PATH {
        return Response::text(404, "not found");
    }
    if !bearer(head).is_some_and(|t| inner.token_matches(t)) {
        return Response::text(401, "unauthorized")
            .with_header("WWW-Authenticate", "Bearer realm=\"mongo-studio\"");
    }
    let session_id = head.header(SESSION_HEADER).map(str::to_string);
    match head.method.as_str() {
        "POST" => {
            let message: Value = match serde_json::from_slice(body) {
                Ok(v) => v,
                Err(e) => {
                    return Response::json(
                        400,
                        &rpc_error(Value::Null, -32700, &format!("Parse error: {e}")),
                    )
                }
            };
            if message.get("method").and_then(Value::as_str) == Some("initialize") {
                return initialize(inner, &message);
            }
            let Some(session_id) = session_id else {
                return Response::json(
                    400,
                    &rpc_error(
                        Value::Null,
                        -32600,
                        "Bad Request: missing Mcp-Session-Id; send initialize first",
                    ),
                );
            };
            let Some(client) = touch(inner, &session_id) else {
                return Response::text(404, "session not found; initialize again");
            };
            match handle_payload(inner, &client, message).await {
                Some(answer) => Response::json(200, &answer),
                None => Response::empty(202),
            }
        }
        "DELETE" => match session_id {
            Some(id) if inner.forget_session(&id) => {
                inner.emit_status();
                Response::empty(200)
            }
            Some(_) => Response::text(404, "session not found"),
            None => Response::text(400, "missing Mcp-Session-Id"),
        },
        // No server-initiated stream is offered.
        _ => Response::empty(405).with_header("Allow", "POST, DELETE"),
    }
}

/// Starts a session for the client and answers `initialize`.
fn initialize(inner: &Inner, message: &Value) -> Response {
    let Some(id) = message.get("id").cloned() else {
        return Response::json(400, &rpc_error(Value::Null, -32600, "Invalid Request"));
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let info = params.get("clientInfo");
    let text = |key: &str| {
        info.and_then(|i| i.get(key))
            .and_then(Value::as_str)
            .map(|s| s.chars().take(80).collect::<String>())
            .filter(|s| !s.trim().is_empty())
    };
    let now = now_ms();
    let session = ClientSession {
        id: Uuid::new_v4().to_string(),
        name: text("title")
            .or_else(|| text("name"))
            .unwrap_or_else(|| "Unknown client".to_string()),
        version: text("version"),
        connected_at: now,
        last_seen: now,
    };
    log::info!(
        "MCP server: {} {} connected; session {}",
        session.name,
        session.version.as_deref().unwrap_or(""),
        session.id
    );
    let session_id = session.id.clone();
    inner
        .sessions
        .lock()
        .unwrap()
        .insert(session_id.clone(), session);
    inner.emit_status();

    let version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    let result = json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": super::SERVER_NAME,
            "title": "Mongo Studio",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    });
    Response::json(
        200,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    )
    .with_header(SESSION_HEADER, session_id)
}

/// The session's client, with `lastSeen` updated; `None` when unknown.
fn touch(inner: &Inner, session_id: &str) -> Option<ClientSession> {
    let (client, announce) = {
        let mut sessions = inner.sessions.lock().unwrap();
        let session = sessions.get_mut(session_id)?;
        let now = now_ms();
        let announce = now.saturating_sub(session.last_seen) >= LAST_SEEN_EVERY_MS;
        session.last_seen = now;
        (session.clone(), announce)
    };
    if announce {
        inner.emit_status();
    }
    Some(client)
}

/// A single message or a batch; `None` when nothing needs answering.
pub(crate) async fn handle_payload(
    inner: &Arc<Inner>,
    client: &ClientSession,
    payload: Value,
) -> Option<Value> {
    match payload {
        Value::Array(messages) => {
            if messages.is_empty() {
                return Some(rpc_error(Value::Null, -32600, "Invalid Request"));
            }
            let mut answers = Vec::new();
            for message in messages {
                if let Some(answer) = handle_message(inner, client, message).await {
                    answers.push(answer);
                }
            }
            (!answers.is_empty()).then_some(Value::Array(answers))
        }
        message => handle_message(inner, client, message).await,
    }
}

pub(crate) async fn handle_message(
    inner: &Arc<Inner>,
    client: &ClientSession,
    message: Value,
) -> Option<Value> {
    let Value::Object(message) = message else {
        return Some(rpc_error(Value::Null, -32600, "Invalid Request"));
    };
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return id.map(|id| rpc_error(id, -32600, "Invalid Request"));
    };
    // Notifications (initialized, cancelled, ...) need no answer.
    let id = id?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools::definitions(&inner.config()) }),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(id, -32602, "Missing tool name"));
            };
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            match tools::call(inner, client, name, args).await {
                Some(output) => json!({
                    "content": [{ "type": "text", "text": output.text }],
                    "isError": output.is_error,
                }),
                None => return Some(rpc_error(id, -32602, &format!("Unknown tool: {name}"))),
            }
        }
        _ => {
            return Some(rpc_error(
                id,
                -32601,
                &format!("Method not found: {method}"),
            ))
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tokio::io::AsyncReadExt;

    use super::super::test_support::inner;
    use super::super::{Policy, STATUS_EVENT};
    use super::*;

    fn config(port: u16, caps: &[(&str, Policy)]) -> McpConfig {
        McpConfig {
            enabled: true,
            port,
            caps: caps.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            connections: HashMap::from([("c1".to_string(), Level::Read)]),
            always: HashMap::new(),
            alerts: Default::default(),
        }
    }

    struct Reply {
        status: u16,
        headers: String,
        body: String,
    }

    impl Reply {
        fn json(&self) -> Value {
            serde_json::from_str(&self.body).unwrap_or(Value::Null)
        }

        fn header(&self, name: &str) -> Option<String> {
            self.headers.lines().find_map(|line| {
                let (n, v) = line.split_once(':')?;
                n.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
            })
        }
    }

    async fn send(port: u16, method: &str, headers: &[(&str, &str)], body: &str) -> Reply {
        let mut request = format!("{method} /mcp HTTP/1.1\r\nConnection: close\r\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        let raw = String::from_utf8(raw).unwrap();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        let status = head[9..12].parse().unwrap();
        Reply {
            status,
            headers: head.to_string(),
            body: body.to_string(),
        }
    }

    const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"2.1.0"}}}"#;

    #[tokio::test]
    async fn serves_initialize_tools_and_sessions_over_http() {
        let (host, inner) = inner(Duration::from_secs(1));
        apply_config(&inner, config(0, &[("read", Policy::Allow)])).await;
        let status = inner.status();
        assert!(status.running, "{:?}", status.error);
        let port = status.port;
        assert_eq!(status.url, format!("http://127.0.0.1:{port}/mcp"));
        let token = inner.token().unwrap();
        let auth = format!("Bearer {token}");
        let host_header = format!("127.0.0.1:{port}");

        // initialize: instructions and a session id.
        let reply = send(
            port,
            "POST",
            &[("Host", &host_header), ("Authorization", &auth)],
            INIT,
        )
        .await;
        assert_eq!(reply.status, 200, "{}", reply.body);
        let answer = reply.json();
        assert_eq!(answer["result"]["serverInfo"]["name"], "mongo-studio");
        assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
        let instructions = answer["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains("list_connections"));
        assert!(instructions.contains("Passwords are never available"));
        let session = reply.header(SESSION_HEADER).expect("session id");
        let clients = inner.status().clients;
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].name, "claude-code");
        assert_eq!(clients[0].version.as_deref(), Some("2.1.0"));
        assert!(!host.events_named(STATUS_EVENT).is_empty());

        // tools/list with the session.
        let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let headers = [
            ("Host", host_header.as_str()),
            ("Authorization", auth.as_str()),
            (SESSION_HEADER, session.as_str()),
        ];
        let reply = send(port, "POST", &headers, list).await;
        assert_eq!(reply.status, 200);
        let names: Vec<String> = reply.json()["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"find".to_string()));
        assert!(names.contains(&"list_databases".to_string()));
        assert!(!names.contains(&"insert_documents".to_string()));
        assert!(!names.contains(&"list_connections".to_string()));

        // A notification is accepted without an answer.
        let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert_eq!(send(port, "POST", &headers, note).await.status, 202);

        // No session id, or one that is unknown.
        let no_session = send(
            port,
            "POST",
            &[("Host", &host_header), ("Authorization", &auth)],
            list,
        )
        .await;
        assert_eq!(no_session.status, 400);
        let unknown = send(
            port,
            "POST",
            &[
                ("Host", &host_header),
                ("Authorization", &auth),
                (SESSION_HEADER, "nope"),
            ],
            list,
        )
        .await;
        assert_eq!(unknown.status, 404);

        // A client the user disconnected gets 404 and must initialize again.
        inner.forget_session(&session);
        assert_eq!(send(port, "POST", &headers, list).await.status, 404);
        assert!(inner.status().clients.is_empty());

        // Turning it off stops the listener.
        let mut off = config(port, &[]);
        off.enabled = false;
        apply_config(&inner, off).await;
        assert!(!inner.status().running);
        assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    }

    #[tokio::test]
    async fn refuses_bad_tokens_and_foreign_hosts() {
        let (_host, inner) = inner(Duration::from_secs(1));
        apply_config(&inner, config(0, &[])).await;
        let port = inner.status().port;
        let token = inner.token().unwrap();
        let host_header = format!("localhost:{port}");

        let wrong = send(
            port,
            "POST",
            &[("Host", &host_header), ("Authorization", "Bearer wrong")],
            INIT,
        )
        .await;
        assert_eq!(wrong.status, 401);
        assert!(wrong.header("WWW-Authenticate").is_some());
        let missing = send(port, "POST", &[("Host", &host_header)], INIT).await;
        assert_eq!(missing.status, 401);

        let auth = format!("Bearer {token}");
        let rebound = send(
            port,
            "POST",
            &[("Host", "attacker.example"), ("Authorization", &auth)],
            INIT,
        )
        .await;
        assert_eq!(rebound.status, 403);
        let origin = send(
            port,
            "POST",
            &[
                ("Host", &host_header),
                ("Origin", "https://evil.example"),
                ("Authorization", &auth),
            ],
            INIT,
        )
        .await;
        assert_eq!(origin.status, 403);
        let get = send(
            port,
            "GET",
            &[("Host", &host_header), ("Authorization", &auth)],
            "",
        )
        .await;
        assert_eq!(get.status, 405);
        assert!(inner.status().clients.is_empty());

        // A regenerated token locks the old one out.
        inner.regenerate_token().unwrap();
        let old = send(
            port,
            "POST",
            &[("Host", &host_header), ("Authorization", &auth)],
            INIT,
        )
        .await;
        assert_eq!(old.status, 401);
    }

    #[tokio::test]
    async fn a_taken_port_is_reported_not_replaced() {
        let taken = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = taken.local_addr().unwrap().port();
        let (_host, inner) = inner(Duration::from_secs(1));
        apply_config(&inner, config(port, &[])).await;
        let status = inner.status();
        assert!(!status.running);
        assert_eq!(status.port, port);
        assert!(status.error.unwrap().contains(&port.to_string()));

        // Once freed, applying the config again starts it.
        drop(taken);
        apply_config(&inner, config(port, &[])).await;
        let status = inner.status();
        assert!(status.running, "{:?}", status.error);
        assert!(status.error.is_none());
    }

    #[tokio::test]
    async fn policy_changes_keep_the_listener_and_its_clients() {
        let (_host, inner) = inner(Duration::from_secs(1));
        apply_config(&inner, config(0, &[])).await;
        let port = inner.status().port;
        super::super::test_support::add_session(&inner, "s1", "Cursor");
        apply_config(&inner, config(port, &[("write", Policy::Ask)])).await;
        let status = inner.status();
        assert!(status.running);
        assert_eq!(status.port, port);
        assert_eq!(status.clients.len(), 1);
    }
}

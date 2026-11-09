//! The tools: browse, query and change the user's saved connections, edit
//! the saved connections themselves, save scripts, and use and propose
//! relations (in `relations`).
//!
//! Every call goes the same way: its capability and the connection's level
//! are checked against the user's settings (`policy::decide`); calls that
//! change data first compute a dry run (how many documents would change),
//! which goes on the approval card when the user wants to be asked and into
//! the result; then the call runs on the server's own Mongo client for that
//! connection. Every call is reported to the windows as an `mcp-activity`
//! event. Logs name the tool, connection and namespace, never filters,
//! documents or scripts.

mod relations;

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::TryStreamExt;
use mongodb::bson::{doc, Document};
use mongodb::options::IndexOptions;
use mongodb::{Client, IndexModel};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::policy::{self, Answer, Cap, Decision, Request};
use super::{
    now_ms, ClientSession, Inner, Level, McpConfig, Policy, ACTIVITY_EVENT, CONNECTION_EVENT,
};
use crate::assistant::{
    doc_arg, infer_schema, int_arg, opt_doc_arg, opt_json_arg, pipeline_writes, plural,
    render_schema, str_arg, summarize_explain, thousands, truncate,
};
use crate::connection::extract_uri_credentials;
use crate::ejson::{bson_to_json, document_to_json, json_to_document};
use crate::error::{AppError, AppResult};
use crate::logging::{self, Detail, Op};
use crate::models::{ConnectionProfile, ConnectionSource};
use crate::state::CancelToken;

/// Server-side limit on reads.
const MAX_TIME: Duration = Duration::from_secs(30);
/// Client-side backstop for one operation (scripts have their own timeout).
const OP_TIMEOUT: Duration = Duration::from_secs(60);
/// Tool output is cut here.
const MAX_OUTPUT: usize = 100 * 1024;
/// The approval card's code is cut here.
const MAX_CODE: usize = 8 * 1024;
const MAX_META: usize = 120;
const DEFAULT_DOCS: i64 = 20;
const MAX_DOCS: i64 = 100;
const MAX_INSERT: usize = 1000;
const DEFAULT_SCRIPT_MS: i64 = 30_000;
const MAX_SCRIPT_MS: i64 = 300_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    ListConnections,
    ListDatabases,
    ListCollections,
    ListIndexes,
    SampleSchema,
    Find,
    Aggregate,
    Count,
    Explain,
    InsertDocuments,
    UpdateDocuments,
    DeleteDocuments,
    CreateCollection,
    DropCollection,
    CreateIndex,
    DropIndex,
    RunScript,
    CreateConnection,
    UpdateConnection,
    DeleteConnection,
    SaveScript,
    ListRelations,
    GetRelation,
    SuggestRelationLinks,
    CreateRelation,
    UpdateRelation,
    TestRelation,
    FindRelated,
    DeleteRelation,
}

/// What a tool works on, and so which of connection_id, database and
/// collection it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Scope {
    Nothing,
    Connection,
    Database,
    Collection,
}

impl Tool {
    const ALL: [Tool; 29] = [
        Tool::ListConnections,
        Tool::ListDatabases,
        Tool::ListCollections,
        Tool::ListIndexes,
        Tool::SampleSchema,
        Tool::Find,
        Tool::Aggregate,
        Tool::Count,
        Tool::Explain,
        Tool::InsertDocuments,
        Tool::UpdateDocuments,
        Tool::DeleteDocuments,
        Tool::CreateCollection,
        Tool::DropCollection,
        Tool::CreateIndex,
        Tool::DropIndex,
        Tool::RunScript,
        Tool::CreateConnection,
        Tool::UpdateConnection,
        Tool::DeleteConnection,
        Tool::SaveScript,
        Tool::ListRelations,
        Tool::GetRelation,
        Tool::SuggestRelationLinks,
        Tool::CreateRelation,
        Tool::UpdateRelation,
        Tool::TestRelation,
        Tool::FindRelated,
        Tool::DeleteRelation,
    ];

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }

    fn name(self) -> &'static str {
        match self {
            Tool::ListConnections => "list_connections",
            Tool::ListDatabases => "list_databases",
            Tool::ListCollections => "list_collections",
            Tool::ListIndexes => "list_indexes",
            Tool::SampleSchema => "sample_schema",
            Tool::Find => "find",
            Tool::Aggregate => "aggregate",
            Tool::Count => "count",
            Tool::Explain => "explain",
            Tool::InsertDocuments => "insert_documents",
            Tool::UpdateDocuments => "update_documents",
            Tool::DeleteDocuments => "delete_documents",
            Tool::CreateCollection => "create_collection",
            Tool::DropCollection => "drop_collection",
            Tool::CreateIndex => "create_index",
            Tool::DropIndex => "drop_index",
            Tool::RunScript => "run_script",
            Tool::CreateConnection => "create_connection",
            Tool::UpdateConnection => "update_connection",
            Tool::DeleteConnection => "delete_connection",
            Tool::SaveScript => "save_script",
            Tool::ListRelations => "list_relations",
            Tool::GetRelation => "get_relation",
            Tool::SuggestRelationLinks => "suggest_relation_links",
            Tool::CreateRelation => "create_relation",
            Tool::UpdateRelation => "update_relation",
            Tool::TestRelation => "test_relation",
            Tool::FindRelated => "find_related",
            Tool::DeleteRelation => "delete_relation",
        }
    }

    /// The capability the tool needs; an aggregation that writes needs
    /// `write` instead, test_relation takes `relEdit` when `relRead` is off,
    /// and find_related also needs `read`, see `call`.
    fn cap(self) -> Cap {
        match self {
            Tool::ListConnections => Cap::ConnList,
            Tool::ListDatabases
            | Tool::ListCollections
            | Tool::ListIndexes
            | Tool::SampleSchema => Cap::Browse,
            Tool::Find | Tool::Aggregate | Tool::Count | Tool::Explain => Cap::Read,
            Tool::InsertDocuments | Tool::UpdateDocuments | Tool::DeleteDocuments => Cap::Write,
            Tool::CreateCollection | Tool::DropCollection | Tool::CreateIndex | Tool::DropIndex => {
                Cap::Ddl
            }
            Tool::RunScript => Cap::Script,
            Tool::CreateConnection | Tool::UpdateConnection => Cap::ConnEdit,
            Tool::DeleteConnection => Cap::ConnDelete,
            Tool::SaveScript => Cap::SaveScript,
            Tool::ListRelations | Tool::GetRelation | Tool::TestRelation | Tool::FindRelated => {
                Cap::RelRead
            }
            Tool::SuggestRelationLinks
            | Tool::CreateRelation
            | Tool::UpdateRelation
            | Tool::DeleteRelation => Cap::RelEdit,
        }
    }

    fn scope(self) -> Scope {
        match self {
            Tool::ListConnections
            | Tool::CreateConnection
            | Tool::SaveScript
            | Tool::ListRelations
            | Tool::GetRelation
            | Tool::SuggestRelationLinks
            | Tool::CreateRelation
            | Tool::UpdateRelation
            | Tool::DeleteRelation => Scope::Nothing,
            // The From (or source) connection; the other side's is resolved
            // from the relation and checked too.
            Tool::TestRelation | Tool::FindRelated => Scope::Connection,
            Tool::ListDatabases | Tool::UpdateConnection | Tool::DeleteConnection => {
                Scope::Connection
            }
            Tool::ListCollections | Tool::RunScript => Scope::Database,
            _ => Scope::Collection,
        }
    }

    /// Whether the tool talks to MongoDB through the connection it's about.
    fn uses_mongo(self) -> bool {
        !matches!(
            self,
            Tool::ListConnections
                | Tool::CreateConnection
                | Tool::UpdateConnection
                | Tool::DeleteConnection
                | Tool::SaveScript
                | Tool::ListRelations
                | Tool::GetRelation
                | Tool::SuggestRelationLinks
                | Tool::CreateRelation
                | Tool::UpdateRelation
                | Tool::DeleteRelation
        )
    }

    fn is_relation_tool(self) -> bool {
        matches!(
            self,
            Tool::ListRelations
                | Tool::GetRelation
                | Tool::SuggestRelationLinks
                | Tool::CreateRelation
                | Tool::UpdateRelation
                | Tool::TestRelation
                | Tool::FindRelated
                | Tool::DeleteRelation
        )
    }
}

/// The connection level a capability needs on the connection it's about.
fn level_needed(cap: Cap) -> Level {
    match cap {
        Cap::Write | Cap::Ddl | Cap::Script => Level::Write,
        Cap::ConnList | Cap::SaveScript => Level::None,
        Cap::Browse | Cap::Read | Cap::ConnEdit | Cap::ConnDelete | Cap::RelRead | Cap::RelEdit => {
            Level::Read
        }
    }
}

/// The `tools/list` result: the tools whose capability isn't off.
pub(crate) fn definitions(config: &McpConfig) -> Value {
    let all = all_definitions();
    let tools: Vec<Value> = Tool::ALL
        .into_iter()
        .zip(all)
        .filter(|(tool, _)| policy::cap_policy(config, tool.cap()) != Policy::Off)
        .map(|(_, def)| def)
        .collect();
    Value::Array(tools)
}

fn all_definitions() -> Vec<Value> {
    let connection_id = json!({ "type": "string", "description": "Id of a saved connection, from list_connections." });
    let database = json!({ "type": "string", "description": "Database name." });
    let collection = json!({ "type": "string", "description": "Collection name." });
    let filter = json!({ "type": "object", "description": "Query filter in MongoDB Extended JSON, e.g. {\"status\": \"active\"}, {\"_id\": {\"$oid\": \"...\"}}, {\"createdAt\": {\"$gte\": {\"$date\": \"2026-01-01T00:00:00Z\"}}}. Omit for all documents." });
    let pipeline = json!({ "type": "array", "items": { "type": "object" }, "description": "Aggregation pipeline stages in Extended JSON." });
    let sort =
        json!({ "type": "object", "description": "Sort specification, e.g. {\"createdAt\": -1}." });
    let read_only =
        json!({ "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false });
    let writes = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false });
    let destructive =
        json!({ "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false });
    let on_db = |extra: Value, required: &[&str]| {
        let mut properties = Map::new();
        properties.insert("connection_id".into(), connection_id.clone());
        properties.insert("database".into(), database.clone());
        if let Value::Object(extra) = extra {
            properties.extend(extra);
        }
        let mut req = vec!["connection_id", "database"];
        req.extend_from_slice(required);
        json!({ "type": "object", "properties": properties, "required": req })
    };
    let on_coll = |extra: Value, required: &[&str]| {
        let mut properties = Map::new();
        properties.insert("collection".into(), collection.clone());
        if let Value::Object(extra) = extra {
            properties.extend(extra);
        }
        let mut req = vec!["collection"];
        req.extend_from_slice(required);
        on_db(Value::Object(properties), &req)
    };
    // In `Tool::ALL` order.
    let mut defs = vec![
        json!({
            "name": "list_connections",
            "description": "List the MongoDB connections saved in Mongo Studio that you may use: id, name, sidebar folder, hosts, default database and your access level (read or write). Start here: every other data tool takes a connection_id from this list. Passwords and full connection strings are never shown.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read_only,
        }),
        json!({
            "name": "list_databases",
            "description": "List the databases on a connection, with their size on disk.",
            "inputSchema": { "type": "object", "properties": { "connection_id": connection_id }, "required": ["connection_id"] },
            "annotations": read_only,
        }),
        json!({
            "name": "list_collections",
            "description": "List the collections and views in a database, with their type (collection, view, timeseries).",
            "inputSchema": on_db(json!({}), &[]),
            "annotations": read_only,
        }),
        json!({
            "name": "list_indexes",
            "description": "List a collection's indexes: name, key specification, and whether they are unique.",
            "inputSchema": on_coll(json!({}), &[]),
            "annotations": read_only,
        }),
        json!({
            "name": "sample_schema",
            "description": "Sample documents from a collection and return its field paths with their BSON types and how many sampled documents have each field (names and types only, no values). Use it before writing queries.",
            "inputSchema": on_coll(json!({
                "size": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 200, "description": "How many documents to sample." }
            }), &[]),
            "annotations": read_only,
        }),
        json!({
            "name": "find",
            "description": "Read documents matching a filter, as relaxed Extended JSON, one per line. At most 100 per call; page with skip.",
            "inputSchema": on_coll(json!({
                "filter": filter,
                "projection": { "type": "object", "description": "Projection, e.g. {\"name\": 1, \"_id\": 0}." },
                "sort": sort,
                "skip": { "type": "integer", "minimum": 0, "default": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_DOCS, "default": DEFAULT_DOCS },
            }), &[]),
            "annotations": read_only,
        }),
        json!({
            "name": "aggregate",
            "description": "Run an aggregation pipeline and return up to 100 result documents as relaxed Extended JSON. A pipeline with $out or $merge writes data: it needs write access and may need the user's approval.",
            "inputSchema": on_coll(json!({
                "pipeline": pipeline,
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_DOCS, "default": DEFAULT_DOCS, "description": "How many result documents to return." },
            }), &["pipeline"]),
            "annotations": read_only,
        }),
        json!({
            "name": "count",
            "description": "Count the documents matching a filter (exact count).",
            "inputSchema": on_coll(json!({ "filter": filter }), &[]),
            "annotations": read_only,
        }),
        json!({
            "name": "explain",
            "description": "Explain a find (filter and sort) or an aggregation pipeline with executionStats, and summarize the winning plan: stages, index used, keys and documents examined, documents returned, time.",
            "inputSchema": on_coll(json!({ "filter": filter, "sort": sort, "pipeline": pipeline }), &[]),
            "annotations": read_only,
        }),
        json!({
            "name": "insert_documents",
            "description": "Insert documents (Extended JSON) into a collection. Needs write access; the user may have to approve it in Mongo Studio first.",
            "inputSchema": on_coll(json!({
                "documents": { "type": "array", "items": { "type": "object" }, "minItems": 1, "maxItems": MAX_INSERT, "description": "The documents to insert." }
            }), &["documents"]),
            "annotations": writes,
        }),
        json!({
            "name": "update_documents",
            "description": "Update the documents matching a filter with an update document (operators like $set, $inc, $unset). Updates one document unless many is true. Reports how many documents match before anything changes. Needs write access; the user may have to approve it in Mongo Studio first.",
            "inputSchema": on_coll(json!({
                "filter": filter,
                "update": { "type": "object", "description": "Update document, e.g. {\"$set\": {\"status\": \"archived\"}}." },
                "many": { "type": "boolean", "default": false, "description": "Update every matching document instead of the first." },
                "upsert": { "type": "boolean", "default": false, "description": "Insert a document when none matches." },
            }), &["filter", "update"]),
            "annotations": destructive,
        }),
        json!({
            "name": "delete_documents",
            "description": "Delete the documents matching a filter. Deletes one document unless many is true. Reports how many documents match before anything is deleted. Needs write access; the user may have to approve it in Mongo Studio first.",
            "inputSchema": on_coll(json!({
                "filter": filter,
                "many": { "type": "boolean", "default": false, "description": "Delete every matching document instead of the first." },
            }), &["filter"]),
            "annotations": destructive,
        }),
        json!({
            "name": "create_collection",
            "description": "Create a collection. Needs write access; the user may have to approve it first.",
            "inputSchema": on_coll(json!({}), &[]),
            "annotations": writes,
        }),
        json!({
            "name": "drop_collection",
            "description": "Drop a collection with all its documents and indexes. Needs write access; the user may have to approve it first.",
            "inputSchema": on_coll(json!({}), &[]),
            "annotations": destructive,
        }),
        json!({
            "name": "create_index",
            "description": "Create an index on a collection. Needs write access; the user may have to approve it first.",
            "inputSchema": on_coll(json!({
                "keys": { "type": "object", "description": "Index keys, e.g. {\"email\": 1} or {\"createdAt\": -1, \"status\": 1}." },
                "name": { "type": "string", "description": "Index name; the server picks one when omitted." },
                "unique": { "type": "boolean", "default": false },
            }), &["keys"]),
            "annotations": writes,
        }),
        json!({
            "name": "drop_index",
            "description": "Drop an index by name (see list_indexes). Needs write access; the user may have to approve it first.",
            "inputSchema": on_coll(json!({ "name": { "type": "string", "description": "Index name." } }), &["name"]),
            "annotations": destructive,
        }),
        json!({
            "name": "run_script",
            "description": "Run JavaScript on a database with Mongo Studio's built-in engine (not full mongosh). It has db.getCollection(name) (or db.collection(name)) with find(filter, options), findOne, insertOne, insertMany, updateOne, updateMany, deleteOne, deleteMany, aggregate and countDocuments - each returns a promise, so await it - plus db.getSiblingDB(name), `use <db>` lines, print/console.log, ObjectId(hex) and ISODate(text). Returns the last expression's value as Extended JSON and what the script printed. Scripts can change data, so this needs write access, and the user may have to approve it first.",
            "inputSchema": on_db(json!({
                "script": { "type": "string", "description": "The script, e.g. const n = await db.getCollection(\"orders\").countDocuments({ status: \"open\" }); n" },
                "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": MAX_SCRIPT_MS, "default": DEFAULT_SCRIPT_MS },
            }), &["script"]),
            "annotations": destructive,
        }),
        json!({
            "name": "create_connection",
            "description": "Save a new connection in Mongo Studio from a connection string. Any password in the string is removed and not saved: the user adds it in the app. The new connection starts with read access for you.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Name shown in Mongo Studio's sidebar." },
                    "uri": { "type": "string", "description": "mongodb:// or mongodb+srv:// connection string, preferably without a password." },
                    "folder": { "type": "string", "description": "Name of an existing sidebar folder to put it in." },
                },
                "required": ["name", "uri"],
            },
            "annotations": writes,
        }),
        json!({
            "name": "update_connection",
            "description": "Rename a saved connection or change its connection string. Any password in the string is removed and not saved: the user adds it in the app.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connection_id": connection_id,
                    "name": { "type": "string", "description": "New name." },
                    "uri": { "type": "string", "description": "New mongodb:// or mongodb+srv:// connection string." },
                },
                "required": ["connection_id"],
            },
            "annotations": writes,
        }),
        json!({
            "name": "delete_connection",
            "description": "Delete a saved connection from Mongo Studio, with its saved password. The data on the server is not touched.",
            "inputSchema": { "type": "object", "properties": { "connection_id": connection_id }, "required": ["connection_id"] },
            "annotations": destructive,
        }),
        json!({
            "name": "save_script",
            "description": "Save a script to Mongo Studio's Saved scripts, so the user can open and run it in the app's console.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Script name, used as its file name." },
                    "content": { "type": "string", "description": "The mongosh-style JavaScript." },
                    "database": { "type": "string", "description": "Database the script is meant for; adds a `use` line." },
                },
                "required": ["name", "content"],
            },
            "annotations": writes,
        }),
    ];
    defs.extend(relations::definitions());
    defs
}

pub(crate) struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

/// What a tool call is about.
#[derive(Default)]
struct Target {
    connection_id: Option<String>,
    profile: Option<ConnectionProfile>,
    database: Option<String>,
    collection: Option<String>,
    /// Shown instead of the namespace, e.g. a relation's name.
    label: Option<String>,
}

impl Target {
    /// `shop.users`, `shop`, the label, or the connection's name.
    fn text(&self) -> String {
        if let (Some(label), None) = (&self.label, &self.collection) {
            return label.clone();
        }
        match (&self.database, &self.collection) {
            (Some(db), Some(coll)) => format!("{db}.{coll}"),
            (Some(db), None) => db.clone(),
            _ => self
                .profile
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_default(),
        }
    }

    fn db(&self) -> &str {
        self.database.as_deref().unwrap_or_default()
    }

    fn coll(&self) -> &str {
        self.collection.as_deref().unwrap_or_default()
    }

    fn connection_name(&self) -> &str {
        self.profile
            .as_ref()
            .map(|p| p.name.as_str())
            .unwrap_or_default()
    }
}

/// The approval card and the dry run.
struct Plan {
    title: String,
    code: String,
    dry_run: Option<String>,
}

/// A successful call: text for the client, meta for the activity line.
struct Done {
    text: String,
    meta: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityEvent<'a> {
    at: u64,
    client_name: &'a str,
    tool: &'a str,
    target: String,
    connection_id: Option<&'a str>,
    meta: String,
    result: &'a str,
    label: String,
}

struct Call<'a> {
    inner: &'a Arc<Inner>,
    client: &'a ClientSession,
    tool: Tool,
    target: Target,
    started: Instant,
}

impl Call<'_> {
    /// Reports the call to the windows and the log, and builds the output.
    fn finish(&self, result: &str, label: String, meta: String, text: String) -> ToolOutput {
        self.inner.emit(
            ACTIVITY_EVENT,
            &ActivityEvent {
                at: now_ms(),
                client_name: &self.client.name,
                tool: self.tool.name(),
                target: self.target.text(),
                connection_id: self.target.connection_id.as_deref(),
                meta: truncate(meta, MAX_META),
                result,
                label: truncate(label.clone(), MAX_META),
            },
        );
        ToolOutput {
            text: truncate(text, MAX_OUTPUT),
            is_error: result != "ok",
        }
    }

    fn log(&self, outcome: &str) {
        let connection = match &self.target.profile {
            Some(p) => format!(" [\"{}\" ({})]", p.name, p.id),
            None => String::new(),
        };
        let target = self.target.text();
        let on = if target.is_empty() || self.target.database.is_none() {
            String::new()
        } else {
            format!(" on {target}")
        };
        log::info!(
            "MCP {}: {}{on}{connection}: {outcome} ({} ms)",
            self.client.name,
            self.tool.name(),
            self.started.elapsed().as_millis()
        );
    }

    fn refuse(&self, label: &str, text: &str) -> ToolOutput {
        self.log(label);
        self.finish("warn", label.to_string(), String::new(), text.to_string())
    }

    fn fail(&self, error: AppError) -> ToolOutput {
        self.log(&format!(
            "failed: {}",
            logging::describe(&error, Detail::NoUserData)
        ));
        let message = logging::scrub_credentials(&error.to_string());
        self.finish("err", format!("Failed: {message}"), String::new(), message)
    }
}

/// Runs a `tools/call`. `None` when there's no tool by that name.
pub(crate) async fn call(
    inner: &Arc<Inner>,
    client: &ClientSession,
    name: &str,
    args: Value,
) -> Option<ToolOutput> {
    let tool = Tool::from_name(name)?;
    let args = match args {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };
    let mut call = Call {
        inner,
        client,
        tool,
        target: Target::default(),
        started: Instant::now(),
    };
    Some(run_call(&mut call, &args).await)
}

async fn run_call(call: &mut Call<'_>, args: &Value) -> ToolOutput {
    let tool = call.tool;
    let scope = tool.scope();
    if scope >= Scope::Connection {
        match str_arg(args, "connection_id") {
            Some(id) => call.target.connection_id = Some(id),
            None => {
                return call.fail(invalid(
                    "connection_id is required: take it from list_connections.",
                ))
            }
        }
    }
    if scope >= Scope::Database {
        match str_arg(args, "database") {
            Some(db) => call.target.database = Some(db),
            None => return call.fail(invalid("database is required.")),
        }
    }
    if scope >= Scope::Collection {
        match str_arg(args, "collection") {
            Some(coll) => call.target.collection = Some(coll),
            None => return call.fail(invalid("collection is required.")),
        }
    }

    let config = call.inner.config();
    let cap = match tool {
        Tool::Aggregate if aggregate_writes(args) => Cap::Write,
        Tool::TestRelation if policy::cap_policy(&config, Cap::RelRead) == Policy::Off => {
            Cap::RelEdit
        }
        _ => tool.cap(),
    };
    let decide = |cap: Cap| {
        policy::decide(
            &config,
            cap,
            call.target.connection_id.as_deref(),
            level_needed(cap),
        )
    };
    let (cap, decision) = match tool {
        // Joining returns documents: `read` must allow it as well.
        Tool::FindRelated => strictest((cap, decide(cap)), (Cap::Read, decide(Cap::Read))),
        _ => (cap, decide(cap)),
    };
    match decision {
        Decision::Off => return call.refuse("Refused: turned off", policy::REFUSED_OFF),
        Decision::NoAccess => return call.refuse("Refused: no access", policy::REFUSED_NO_ACCESS),
        Decision::Ask | Decision::Allow | Decision::AllowAlways => {}
    }

    if let Some(id) = &call.target.connection_id {
        match call.inner.state().connection_store.get(id) {
            Ok(profile) => call.target.profile = Some(profile),
            Err(_) => {
                return call.fail(AppError::NotFound(
                    "no saved connection has this id; call list_connections".to_string(),
                ))
            }
        }
    }

    if tool.is_relation_tool() {
        match relations::prepare(call, args) {
            Ok(relations::Access::Granted) => {}
            Ok(relations::Access::NoAccess) => {
                return call.refuse("Refused: no access", policy::REFUSED_NO_ACCESS)
            }
            Err(e) => return call.fail(e),
        }
    }

    let mongo = if tool.uses_mongo() {
        let profile = call.target.profile.as_ref().expect("checked above");
        match call
            .inner
            .pool
            .client(call.inner.host.as_ref(), profile)
            .await
        {
            Ok(client) => Some(client),
            Err(e) => return call.fail(e),
        }
    } else {
        None
    };

    let plan = match plan(call, args, mongo.as_ref()).await {
        Ok(plan) => plan,
        Err(e) => return call.fail(e),
    };

    let label = match decision {
        Decision::Allow => "Allowed",
        Decision::AllowAlways => "Allowed (always)",
        _ => {
            let request = Request {
                tool: tool.name(),
                capability: cap,
                connection_id: call.target.connection_id.clone(),
                connection_name: call.target.profile.as_ref().map(|p| p.name.clone()),
                database: call.target.database.clone(),
                collection: call.target.collection.clone(),
                title: plan.title.clone(),
                code: truncate(plan.code.clone(), MAX_CODE),
                dry_run: plan.dry_run.clone(),
            };
            match policy::ask(call.inner, call.client, &request).await {
                Answer::Once => "Approved by you",
                Answer::Always => "Approved by you (always)",
                Answer::Deny => return call.refuse("Denied by you", policy::DENIED),
                Answer::Timeout => return call.refuse("Not answered", policy::NOT_ANSWERED),
                Answer::Closed => return call.refuse("Closed", policy::CLOSED),
            }
        }
    };

    let timeout = match tool {
        Tool::RunScript => {
            Duration::from_millis(script_timeout(args) as u64) + Duration::from_secs(30)
        }
        _ => OP_TIMEOUT,
    };
    let result = tokio::time::timeout(timeout, execute(call, args, mongo.as_ref(), &plan))
        .await
        .unwrap_or_else(|_| {
            Err(AppError::InvalidInput(
                "The operation timed out.".to_string(),
            ))
        });
    match result {
        Ok(done) => {
            call.log(label);
            call.finish("ok", label.to_string(), done.meta, done.text)
        }
        Err(e) => call.fail(e),
    }
}

/// The stricter of two decisions, with its capability: off, then no
/// access, then ask, then allowed.
fn strictest(a: (Cap, Decision), b: (Cap, Decision)) -> (Cap, Decision) {
    let rank = |d: Decision| match d {
        Decision::Off => 0,
        Decision::NoAccess => 1,
        Decision::Ask => 2,
        Decision::AllowAlways => 3,
        Decision::Allow => 4,
    };
    if rank(b.1) < rank(a.1) {
        b
    } else {
        a
    }
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::InvalidInput(message.into())
}

fn aggregate_writes(args: &Value) -> bool {
    matches!(opt_json_arg(args, "pipeline"), Ok(Some(Value::Array(stages))) if pipeline_writes(&stages))
}

fn script_timeout(args: &Value) -> i64 {
    int_arg(args, "timeout_ms", DEFAULT_SCRIPT_MS, MAX_SCRIPT_MS).max(1000)
}

fn doc_of(args: &Value, key: &str) -> AppResult<Document> {
    doc_arg(args, key).map_err(AppError::InvalidInput)
}

fn opt_doc_of(args: &Value, key: &str) -> AppResult<Option<Document>> {
    opt_doc_arg(args, key).map_err(AppError::InvalidInput)
}

fn json_of(args: &Value, key: &str) -> AppResult<Option<Value>> {
    opt_json_arg(args, key).map_err(AppError::InvalidInput)
}

fn bool_of(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn pipeline_of(args: &Value) -> AppResult<Vec<Value>> {
    match json_of(args, "pipeline")? {
        Some(Value::Array(stages)) => Ok(stages),
        Some(_) => Err(invalid("pipeline must be a JSON array of stages.")),
        None => Err(invalid("pipeline is required.")),
    }
}

fn stages_of(stages: Vec<Value>) -> AppResult<Vec<Document>> {
    stages.into_iter().map(json_to_document).collect()
}

/// Compact JSON for the approval card.
fn js(value: &Value) -> String {
    value.to_string()
}

fn js_arg(args: &Value, key: &str, default: &str) -> String {
    match opt_json_arg(args, key) {
        Ok(Some(value)) => js(&value),
        _ => default.to_string(),
    }
}

/// `use shop` and a statement on one collection, as mongosh would read it.
pub(crate) fn code_on(db: &str, coll: Option<&str>, statement: &str) -> String {
    match coll {
        Some(coll) => format!(
            "use {db}\ndb.getCollection({}).{statement}",
            Value::String(coll.to_string())
        ),
        None => format!("use {db}\n{statement}"),
    }
}

/// What a dry run found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DryRun {
    Insert(u64),
    Update { matched: u64, upsert: bool },
    Delete(u64),
    Drop(Option<u64>),
}

pub(crate) fn dry_run_text(dry_run: DryRun) -> String {
    let docs = |n: u64| plural(n, "document", "documents");
    match dry_run {
        DryRun::Insert(n) => format!("{} would be inserted", docs(n)),
        DryRun::Update {
            matched: 0,
            upsert: true,
        } => "No documents match the filter; 1 document would be inserted (upsert)".to_string(),
        DryRun::Update { matched: 0, .. } => {
            "No documents match the filter; nothing would change".to_string()
        }
        DryRun::Update { matched, .. } => format!("{} would change", docs(matched)),
        DryRun::Delete(0) => "No documents match the filter; nothing would be deleted".to_string(),
        DryRun::Delete(n) => format!("{} would be deleted", docs(n)),
        DryRun::Drop(Some(n)) => format!("The collection and its {} would be deleted", docs(n)),
        DryRun::Drop(None) => "The collection would be deleted".to_string(),
    }
}

fn collection(client: &Client, db: &str, coll: &str) -> mongodb::Collection<Document> {
    client.database(db).collection::<Document>(coll)
}

/// Counts the matches of a write's filter, up to one unless `many`.
async fn count_matches(
    client: &Client,
    target: &Target,
    filter: Document,
    many: bool,
) -> AppResult<u64> {
    let coll = collection(client, target.db(), target.coll());
    let mut count = coll.count_documents(filter).max_time(MAX_TIME);
    if !many {
        count = count.limit(1);
    }
    Ok(count.await?)
}

/// The approval card's title and code, and the dry run of calls that
/// change data. Also validates the arguments, so a bad call fails before
/// the user is asked about it.
async fn plan(call: &Call<'_>, args: &Value, mongo: Option<&Client>) -> AppResult<Plan> {
    let t = &call.target;
    let (db, coll) = (t.db(), t.coll());
    let ns = t.text();
    let on_coll = |statement: String| code_on(db, Some(coll), &statement);
    let plan = |title: String, code: String| Plan {
        title,
        code,
        dry_run: None,
    };
    Ok(match call.tool {
        Tool::ListConnections => plan(
            "List your saved connections".to_string(),
            "list_connections".to_string(),
        ),
        Tool::ListDatabases => plan(
            format!("List the databases on {}", t.connection_name()),
            "show dbs".to_string(),
        ),
        Tool::ListCollections => plan(
            format!("List the collections in {db}"),
            code_on(db, None, "show collections"),
        ),
        Tool::ListIndexes => plan(
            format!("getIndexes on {ns}"),
            on_coll("getIndexes()".to_string()),
        ),
        Tool::SampleSchema => {
            let size = int_arg(args, "size", 200, 1000);
            plan(
                format!("Sample the schema of {ns}"),
                on_coll(format!("aggregate([{{\"$sample\":{{\"size\":{size}}}}}])")),
            )
        }
        Tool::Find => {
            opt_doc_of(args, "filter")?;
            opt_doc_of(args, "projection")?;
            opt_doc_of(args, "sort")?;
            let mut statement = format!(
                "find({}{})",
                js_arg(args, "filter", "{}"),
                match json_of(args, "projection")? {
                    Some(p) => format!(", {}", js(&p)),
                    None => String::new(),
                }
            );
            if let Some(sort) = json_of(args, "sort")? {
                statement.push_str(&format!(".sort({})", js(&sort)));
            }
            let skip = args.get("skip").and_then(Value::as_i64).unwrap_or(0);
            if skip > 0 {
                statement.push_str(&format!(".skip({skip})"));
            }
            let limit = int_arg(args, "limit", DEFAULT_DOCS, MAX_DOCS);
            statement.push_str(&format!(".limit({limit})"));
            plan(format!("find on {ns}"), on_coll(statement))
        }
        Tool::Aggregate => {
            let stages = pipeline_of(args)?;
            let writes = pipeline_writes(&stages);
            let code = on_coll(format!("aggregate({})", js(&Value::Array(stages.clone()))));
            stages_of(stages)?;
            let title = if writes {
                format!("aggregate with $out/$merge on {ns}")
            } else {
                format!("aggregate on {ns}")
            };
            plan(title, code)
        }
        Tool::Count => {
            opt_doc_of(args, "filter")?;
            plan(
                format!("countDocuments on {ns}"),
                on_coll(format!("countDocuments({})", js_arg(args, "filter", "{}"))),
            )
        }
        Tool::Explain => {
            let statement = match json_of(args, "pipeline")? {
                Some(Value::Array(stages)) => {
                    if pipeline_writes(&stages) {
                        return Err(invalid(
                            "explain doesn't run pipelines with $out or $merge; remove that stage.",
                        ));
                    }
                    format!(
                        "explain(\"executionStats\").aggregate({})",
                        js(&Value::Array(stages))
                    )
                }
                Some(_) => return Err(invalid("pipeline must be a JSON array of stages.")),
                None => {
                    opt_doc_of(args, "filter")?;
                    let mut s = format!("find({})", js_arg(args, "filter", "{}"));
                    if let Some(sort) = json_of(args, "sort")? {
                        s.push_str(&format!(".sort({})", js(&sort)));
                    }
                    s.push_str(".explain(\"executionStats\")");
                    s
                }
            };
            plan(format!("explain on {ns}"), on_coll(statement))
        }
        Tool::InsertDocuments => {
            let docs = documents_of(args)?;
            let n = docs.len() as u64;
            let code = if n == 1 {
                on_coll(format!("insertOne({})", js(&docs[0])))
            } else {
                on_coll(format!("insertMany({})", js(&Value::Array(docs))))
            };
            Plan {
                title: format!(
                    "{} on {ns}",
                    if n == 1 { "insertOne" } else { "insertMany" }
                ),
                code,
                dry_run: Some(dry_run_text(DryRun::Insert(n))),
            }
        }
        Tool::UpdateDocuments => {
            let filter = doc_of(args, "filter")?;
            let update = json_of(args, "update")?.ok_or_else(|| invalid("update is required."))?;
            if !update.is_object() {
                return Err(invalid(
                    "update must be an update document such as {\"$set\": {...}}.",
                ));
            }
            json_to_document(update.clone())?;
            let many = bool_of(args, "many");
            let upsert = bool_of(args, "upsert");
            let op = if many { "updateMany" } else { "updateOne" };
            let options = if upsert { ", {\"upsert\":true}" } else { "" };
            let mongo = mongo.expect("update_documents uses mongo");
            let matched = count_matches(mongo, t, filter, many).await?;
            Plan {
                title: format!("{op} on {ns}"),
                code: on_coll(format!(
                    "{op}({}, {}{options})",
                    js_arg(args, "filter", "{}"),
                    js(&update)
                )),
                dry_run: Some(dry_run_text(DryRun::Update { matched, upsert })),
            }
        }
        Tool::DeleteDocuments => {
            let filter = doc_of(args, "filter")?;
            let many = bool_of(args, "many");
            let op = if many { "deleteMany" } else { "deleteOne" };
            let mongo = mongo.expect("delete_documents uses mongo");
            let matched = count_matches(mongo, t, filter, many).await?;
            Plan {
                title: format!("{op} on {ns}"),
                code: on_coll(format!("{op}({})", js_arg(args, "filter", "{}"))),
                dry_run: Some(dry_run_text(DryRun::Delete(matched))),
            }
        }
        Tool::CreateCollection => plan(
            format!("createCollection {ns}"),
            code_on(
                db,
                None,
                &format!("db.createCollection({})", Value::String(coll.to_string())),
            ),
        ),
        Tool::DropCollection => {
            let mongo = mongo.expect("drop_collection uses mongo");
            let docs = collection(mongo, db, coll)
                .estimated_document_count()
                .max_time(MAX_TIME)
                .await
                .ok();
            Plan {
                title: format!("drop {ns}"),
                code: on_coll("drop()".to_string()),
                dry_run: Some(dry_run_text(DryRun::Drop(docs))),
            }
        }
        Tool::CreateIndex => {
            let keys = json_of(args, "keys")?.ok_or_else(|| invalid("keys is required."))?;
            json_to_document(keys.clone())?;
            let mut options = Map::new();
            if let Some(name) = str_arg(args, "name") {
                options.insert("name".into(), Value::String(name));
            }
            if bool_of(args, "unique") {
                options.insert("unique".into(), Value::Bool(true));
            }
            let options = if options.is_empty() {
                String::new()
            } else {
                format!(", {}", js(&Value::Object(options)))
            };
            plan(
                format!("createIndex on {ns}"),
                on_coll(format!("createIndex({}{options})", js(&keys))),
            )
        }
        Tool::DropIndex => {
            let name = str_arg(args, "name").ok_or_else(|| invalid("name is required."))?;
            plan(
                format!("dropIndex {name} on {ns}"),
                on_coll(format!("dropIndex({})", Value::String(name.clone()))),
            )
        }
        Tool::RunScript => {
            let script = script_of(args)?;
            plan(format!("Run a script on {db}"), code_on(db, None, &script))
        }
        Tool::CreateConnection => {
            let new = new_connection_args(args)?;
            let mut code = format!("name: {}\nuri: {}", new.name, new.uri);
            if let Some(folder) = &new.folder {
                code.push_str(&format!("\nfolder: {folder}"));
            }
            plan(format!("Save a new connection \"{}\"", new.name), code)
        }
        Tool::UpdateConnection => {
            let change = connection_change_args(args)?;
            let mut code = String::new();
            if let Some(name) = &change.name {
                code.push_str(&format!("name: {name}\n"));
            }
            if let Some(uri) = &change.uri {
                code.push_str(&format!("uri: {uri}\n"));
            }
            plan(
                format!("Edit the connection \"{}\"", t.connection_name()),
                code.trim_end().to_string(),
            )
        }
        Tool::DeleteConnection => {
            let profile = t.profile.as_ref().expect("checked before planning");
            plan(
                format!("Delete the connection \"{}\"", profile.name),
                format!(
                    "name: {}\nhosts: {}",
                    profile.name,
                    logging::host_summary(profile)
                ),
            )
        }
        Tool::SaveScript => {
            let (name, content) = script_save_args(args)?;
            plan(format!("Save the script \"{name}\""), content)
        }
        _ => relations::plan(call, args)?,
    })
}

fn documents_of(args: &Value) -> AppResult<Vec<Value>> {
    let docs = match json_of(args, "documents")? {
        Some(Value::Array(docs)) => docs,
        Some(doc @ Value::Object(_)) => vec![doc],
        Some(_) => return Err(invalid("documents must be an array of documents.")),
        None => return Err(invalid("documents is required.")),
    };
    if docs.is_empty() {
        return Err(invalid("documents is empty."));
    }
    if docs.len() > MAX_INSERT {
        return Err(invalid(format!(
            "At most {MAX_INSERT} documents per call; split the insert."
        )));
    }
    for doc in &docs {
        json_to_document(doc.clone())?;
    }
    Ok(docs)
}

fn script_of(args: &Value) -> AppResult<String> {
    args.get("script")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| invalid("script is required."))
}

/// Runs the call, after the policy (and the user) allowed it.
async fn execute(
    call: &Call<'_>,
    args: &Value,
    mongo: Option<&Client>,
    plan: &Plan,
) -> AppResult<Done> {
    let t = &call.target;
    let (db, coll) = (t.db(), t.coll());
    let ns = t.text();
    let connection = t.connection_name().to_string();
    let op_name = call.tool.name();
    let op = Op::new(
        op_name,
        db,
        t.collection.as_deref().filter(|c| !c.is_empty()),
    );
    macro_rules! timed {
        ($fut:expr) => {
            logging::timed(&op, &connection, $fut)
        };
    }
    let dry = plan
        .dry_run
        .as_ref()
        .map(|d| format!("Before running: {d}.\n"))
        .unwrap_or_default();
    match call.tool {
        Tool::ListConnections => Ok(list_connections(call.inner)),
        Tool::CreateConnection => create_connection(call, args),
        Tool::UpdateConnection => update_connection(call, args).await,
        Tool::DeleteConnection => delete_connection(call).await,
        Tool::SaveScript => save_script(call, args),
        tool if tool.is_relation_tool() => relations::execute(call, args, mongo).await,
        _ => {
            let client = mongo.expect("data tools have a client");
            match call.tool {
                Tool::ListDatabases => {
                    let dbs = timed!(async {
                        crate::driver::list_databases(client).await.map(|dbs| {
                            dbs.into_iter()
                                .map(|d| json!({ "name": d.name, "sizeOnDisk": d.size_on_disk }))
                                .collect::<Vec<_>>()
                        })
                    })
                    .await?;
                    let count = plural(dbs.len() as u64, "database", "databases");
                    Ok(Done {
                        text: format!("{count} on {connection}:\n{}", json_lines(dbs)),
                        meta: count,
                    })
                }
                Tool::ListCollections => {
                    let list = timed!(async {
                        crate::driver::list_collections(client, db)
                            .await
                            .map(|list| {
                                list.into_iter()
                                    .map(|c| json!({ "name": c.name, "type": c.collection_type }))
                                    .collect::<Vec<_>>()
                            })
                    })
                    .await?;
                    let count = plural(list.len() as u64, "collection", "collections");
                    Ok(Done {
                        text: format!("{count} in {db}:\n{}", json_lines(list)),
                        meta: count,
                    })
                }
                Tool::ListIndexes => {
                    let indexes = timed!(async {
                        crate::driver::list_indexes(client, db, coll)
                            .await
                            .map(|list| {
                                list.into_iter()
                                    .map(|i| json!({ "name": i.name, "key": i.key, "unique": i.unique }))
                                    .collect::<Vec<_>>()
                            })
                    })
                    .await?;
                    let count = plural(indexes.len() as u64, "index", "indexes");
                    Ok(Done {
                        text: format!("{count} on {ns}:\n{}", json_lines(indexes)),
                        meta: count,
                    })
                }
                Tool::SampleSchema => timed!(sample_schema(client, db, coll, &ns, args)).await,
                Tool::Find => timed!(find(client, db, coll, &ns, args)).await,
                Tool::Aggregate => timed!(aggregate(client, db, coll, &ns, args)).await,
                Tool::Count => {
                    let filter = doc_of(args, "filter")?;
                    let n = timed!(async {
                        Ok(collection(client, db, coll)
                            .count_documents(filter)
                            .max_time(MAX_TIME)
                            .await?)
                    })
                    .await?;
                    let docs = plural(n, "document", "documents");
                    Ok(Done {
                        text: format!("{docs} in {ns} match the filter."),
                        meta: docs,
                    })
                }
                Tool::Explain => timed!(explain(client, db, coll, &ns, args)).await,
                Tool::InsertDocuments => {
                    let docs = documents_of(args)?;
                    let result = timed!(crate::driver::insert_many(
                        client,
                        db,
                        coll,
                        Value::Array(docs),
                    ))
                    .await?;
                    let n = result["insertedCount"].as_u64().unwrap_or(0);
                    let inserted = plural(n, "document", "documents");
                    Ok(Done {
                        text: format!(
                            "Inserted {inserted} into {ns}. insertedIds: {}",
                            result["insertedIds"]
                        ),
                        meta: format!("{inserted} inserted"),
                    })
                }
                Tool::UpdateDocuments => {
                    let filter = doc_of(args, "filter")?;
                    let update = doc_of(args, "update")?;
                    let many = bool_of(args, "many");
                    let upsert = bool_of(args, "upsert");
                    let result = timed!(async {
                        let c = collection(client, db, coll);
                        Ok(if many {
                            c.update_many(filter, update).upsert(upsert).await?
                        } else {
                            c.update_one(filter, update).upsert(upsert).await?
                        })
                    })
                    .await?;
                    let mut text = format!(
                        "{dry}Matched {}, modified {} in {ns}.",
                        plural(result.matched_count, "document", "documents"),
                        result.modified_count
                    );
                    if let Some(id) = result.upserted_id {
                        text.push_str(&format!(" Upserted _id: {}", bson_to_json(id)));
                    }
                    Ok(Done {
                        text,
                        meta: format!(
                            "{} modified",
                            plural(result.modified_count, "document", "documents")
                        ),
                    })
                }
                Tool::DeleteDocuments => {
                    let filter = doc_of(args, "filter")?;
                    let many = bool_of(args, "many");
                    let result = timed!(async {
                        let c = collection(client, db, coll);
                        Ok(if many {
                            c.delete_many(filter).await?
                        } else {
                            c.delete_one(filter).await?
                        })
                    })
                    .await?;
                    let deleted = plural(result.deleted_count, "document", "documents");
                    Ok(Done {
                        text: format!("{dry}Deleted {deleted} from {ns}."),
                        meta: format!("{deleted} deleted"),
                    })
                }
                Tool::CreateCollection => {
                    timed!(async { Ok(client.database(db).create_collection(coll).await?) })
                        .await?;
                    Ok(Done {
                        text: format!("Created the collection {ns}."),
                        meta: "created".to_string(),
                    })
                }
                Tool::DropCollection => {
                    timed!(async { Ok(collection(client, db, coll).drop().await?) }).await?;
                    Ok(Done {
                        text: format!("{dry}Dropped the collection {ns}."),
                        meta: "dropped".to_string(),
                    })
                }
                Tool::CreateIndex => {
                    let keys = doc_of(args, "keys")?;
                    let options = IndexOptions::builder()
                        .name(str_arg(args, "name"))
                        .unique(bool_of(args, "unique").then_some(true))
                        .build();
                    let model = IndexModel::builder().keys(keys).options(options).build();
                    let created = timed!(async {
                        Ok(collection(client, db, coll).create_index(model).await?)
                    })
                    .await?;
                    Ok(Done {
                        text: format!("Created the index {} on {ns}.", created.index_name),
                        meta: created.index_name,
                    })
                }
                Tool::DropIndex => {
                    let name = str_arg(args, "name").unwrap_or_default();
                    timed!(async {
                        Ok(collection(client, db, coll)
                            .drop_index(name.clone())
                            .await?)
                    })
                    .await?;
                    Ok(Done {
                        text: format!("Dropped the index {name} on {ns}."),
                        meta: name.clone(),
                    })
                }
                Tool::RunScript => {
                    let script = script_of(args)?;
                    let timeout = script_timeout(args) as u64;
                    let result = timed!(crate::scripting::run_script(
                        client.clone(),
                        db.to_string(),
                        script,
                        Some(timeout),
                        CancelToken::default(),
                        |_| {},
                    ))
                    .await?;
                    let mut text = format!(
                        "The script ran on {db} (db is now {}).\nResult (relaxed Extended JSON):\n{}",
                        result.database,
                        serde_json::to_string_pretty(&result.value).unwrap_or_default()
                    );
                    if !result.logs.is_empty() {
                        text.push_str(&format!("\nPrinted:\n{}", result.logs.join("\n")));
                    }
                    Ok(Done {
                        text,
                        meta: format!(
                            "{} printed",
                            plural(result.logs.len() as u64, "line", "lines")
                        ),
                    })
                }
                _ => unreachable!("handled above"),
            }
        }
    }
}

type BoxedOp<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = AppResult<T>> + Send + 'a>>;

fn json_lines(items: Vec<Value>) -> String {
    items
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn doc_lines(docs: Vec<Document>) -> String {
    json_lines(docs.into_iter().map(document_to_json).collect())
}

fn sample_schema<'a>(
    client: &'a Client,
    db: &'a str,
    coll: &'a str,
    ns: &'a str,
    args: &'a Value,
) -> BoxedOp<'a, Done> {
    Box::pin(async move {
        let size = int_arg(args, "size", 200, 1000);
        let c = collection(client, db, coll);
        let docs: Vec<Document> = c
            .aggregate(vec![doc! { "$sample": { "size": size } }])
            .max_time(MAX_TIME)
            .await?
            .try_collect()
            .await?;
        let total = c.estimated_document_count().max_time(MAX_TIME).await.ok();
        let fields = infer_schema(&docs);
        let of_total = match total {
            Some(total) => format!("{} of {}", docs.len(), thousands(total)),
            None => docs.len().to_string(),
        };
        let text = if docs.is_empty() {
            format!("Sampled {of_total} documents in {ns}: it is empty.")
        } else {
            format!(
                "Sampled {of_total} documents in {ns}.\n{}",
                render_schema(&fields, docs.len())
            )
        };
        Ok(Done {
            text,
            meta: format!(
                "{of_total} documents · {}",
                plural(fields.len() as u64, "field", "fields")
            ),
        })
    })
}

fn find<'a>(
    client: &'a Client,
    db: &'a str,
    coll: &'a str,
    ns: &'a str,
    args: &'a Value,
) -> BoxedOp<'a, Done> {
    Box::pin(async move {
        let limit = int_arg(args, "limit", DEFAULT_DOCS, MAX_DOCS);
        let skip = args.get("skip").and_then(Value::as_u64).unwrap_or(0);
        let c = collection(client, db, coll);
        let mut find = c
            .find(doc_of(args, "filter")?)
            .limit(limit)
            .skip(skip)
            .max_time(MAX_TIME);
        if let Some(projection) = opt_doc_of(args, "projection")? {
            find = find.projection(projection);
        }
        if let Some(sort) = opt_doc_of(args, "sort")? {
            find = find.sort(sort);
        }
        let docs: Vec<Document> = find.await?.try_collect().await?;
        let n = docs.len() as u64;
        let returned = plural(n, "document", "documents");
        let mut text =
            format!("Returned {returned} from {ns} (relaxed Extended JSON, one per line):");
        if n > 0 {
            text.push('\n');
            text.push_str(&doc_lines(docs));
        }
        if n as i64 == limit {
            text.push_str(&format!(
                "\nThere may be more: pass skip={} for the next page.",
                skip + n
            ));
        }
        Ok(Done {
            text,
            meta: returned,
        })
    })
}

fn aggregate<'a>(
    client: &'a Client,
    db: &'a str,
    coll: &'a str,
    ns: &'a str,
    args: &'a Value,
) -> BoxedOp<'a, Done> {
    Box::pin(async move {
        let stages = pipeline_of(args)?;
        let writes = pipeline_writes(&stages);
        let mut stages = stages_of(stages)?;
        let limit = int_arg(args, "limit", DEFAULT_DOCS, MAX_DOCS);
        if !writes {
            stages.push(doc! { "$limit": limit + 1 });
        }
        let started = Instant::now();
        let mut docs: Vec<Document> = collection(client, db, coll)
            .aggregate(stages)
            .max_time(if writes { OP_TIMEOUT } else { MAX_TIME })
            .await?
            .try_collect()
            .await?;
        let ms = started.elapsed().as_millis();
        if writes {
            return Ok(Done {
                text: format!(
                    "The pipeline ran on {ns} in {ms} ms and wrote its output ($out/$merge)."
                ),
                meta: format!("wrote output · {ms} ms"),
            });
        }
        let more = docs.len() as i64 > limit;
        docs.truncate(limit as usize);
        let returned = plural(docs.len() as u64, "document", "documents");
        let mut text = format!(
            "The pipeline returned {returned} on {ns} in {ms} ms (relaxed Extended JSON, one per line):"
        );
        if !docs.is_empty() {
            text.push('\n');
            text.push_str(&doc_lines(docs));
        }
        if more {
            text.push_str(
                "\nThere are more results: add $skip/$limit stages or narrow the pipeline.",
            );
        }
        Ok(Done {
            text,
            meta: format!("{returned} · {ms} ms"),
        })
    })
}

fn explain<'a>(
    client: &'a Client,
    db: &'a str,
    coll: &'a str,
    ns: &'a str,
    args: &'a Value,
) -> BoxedOp<'a, Done> {
    Box::pin(async move {
        let max_time_ms = MAX_TIME.as_millis() as i64;
        let command = match json_of(args, "pipeline")? {
            Some(Value::Array(stages)) => {
                doc! { "aggregate": coll, "pipeline": stages_of(stages)?, "cursor": {}, "maxTimeMS": max_time_ms }
            }
            Some(_) => return Err(invalid("pipeline must be a JSON array of stages.")),
            None => {
                let mut command = doc! { "find": coll, "filter": doc_of(args, "filter")? };
                if let Some(sort) = opt_doc_of(args, "sort")? {
                    command.insert("sort", sort);
                }
                command.insert("maxTimeMS", max_time_ms);
                command
            }
        };
        let result = client
            .database(db)
            .run_command(doc! { "explain": command, "verbosity": "executionStats" })
            .await?;
        let summary = summarize_explain(&document_to_json(result));
        Ok(Done {
            text: format!("Explained the query on {ns}.\n{}", summary.text),
            meta: summary.meta,
        })
    })
}

// --- Saved connections ---

/// Every folder of the sidebar layout by connection id: (folder id,
/// "Parent / Child" label).
fn folders_by_connection(layout: &Value) -> std::collections::HashMap<String, (String, String)> {
    fn walk(
        nodes: &Value,
        parent: Option<(&str, &str)>,
        out: &mut std::collections::HashMap<String, (String, String)>,
    ) {
        for node in nodes.as_array().into_iter().flatten() {
            let Some(id) = node.get("id").and_then(Value::as_str) else {
                continue;
            };
            match node.get("type").and_then(Value::as_str) {
                Some("connection") => {
                    if let Some((folder_id, label)) = parent {
                        out.insert(id.to_string(), (folder_id.to_string(), label.to_string()));
                    }
                }
                Some("folder") => {
                    let name = node.get("name").and_then(Value::as_str).unwrap_or_default();
                    let label = match parent {
                        Some((_, parent_label)) => format!("{parent_label} / {name}"),
                        None => name.to_string(),
                    };
                    walk(
                        node.get("children").unwrap_or(&Value::Null),
                        Some((id, &label)),
                        out,
                    );
                }
                _ => {}
            }
        }
    }
    let mut out = std::collections::HashMap::new();
    walk(layout.get("root").unwrap_or(&Value::Null), None, &mut out);
    out
}

/// The folder named `name` (its plain name or its "Parent / Child" label),
/// ignoring case: (id, label).
fn find_folder(layout: &Value, name: &str) -> Option<(String, String)> {
    fn walk(nodes: &Value, prefix: &str, name: &str) -> Option<(String, String)> {
        for node in nodes.as_array().into_iter().flatten() {
            if node.get("type").and_then(Value::as_str) != Some("folder") {
                continue;
            }
            let id = node.get("id").and_then(Value::as_str)?;
            let own = node.get("name").and_then(Value::as_str).unwrap_or_default();
            let label = format!("{prefix}{own}");
            if own.eq_ignore_ascii_case(name) || label.eq_ignore_ascii_case(name) {
                return Some((id.to_string(), label));
            }
            if let Some(found) = walk(
                node.get("children").unwrap_or(&Value::Null),
                &format!("{label} / "),
                name,
            ) {
                return Some(found);
            }
        }
        None
    }
    walk(layout.get("root").unwrap_or(&Value::Null), "", name.trim())
}

fn list_connections(inner: &Inner) -> Done {
    let config = inner.config();
    let folders = folders_by_connection(&inner.host.sidebar_layout());
    let visible: Vec<Value> = inner
        .state()
        .connection_store
        .list()
        .into_iter()
        .filter_map(|profile| {
            let level = policy::level_of(&config, &profile.id);
            if level < Level::Read {
                return None;
            }
            Some(json!({
                "id": profile.id,
                "name": profile.name,
                "folder": folders.get(&profile.id).map(|(_, label)| label.clone()),
                "hosts": logging::host_summary(&profile),
                "database": profile.database,
                "access": if level == Level::Write { "read & write" } else { "read" },
            }))
        })
        .collect();
    let count = plural(visible.len() as u64, "connection", "connections");
    let text = if visible.is_empty() {
        "No connections are shared with MCP clients. The user can share connections in Mongo \
         Studio's settings."
            .to_string()
    } else {
        format!(
            "{count} you may use (one JSON object per line):\n{}",
            json_lines(visible)
        )
    };
    Done { text, meta: count }
}

struct NewConnection {
    name: String,
    /// Without its password.
    uri: String,
    username: Option<String>,
    had_password: bool,
    folder: Option<String>,
}

fn checked_uri(uri: &str) -> AppResult<(String, Option<String>, bool)> {
    let uri = uri.trim();
    if !(uri.starts_with("mongodb://") || uri.starts_with("mongodb+srv://")) {
        return Err(invalid(
            "uri must be a mongodb:// or mongodb+srv:// connection string.",
        ));
    }
    let (stripped, username, password) = extract_uri_credentials(uri);
    Ok((
        stripped,
        username.filter(|u| !u.is_empty()),
        password.is_some(),
    ))
}

fn name_arg(args: &Value) -> Option<String> {
    str_arg(args, "name").map(|n| n.chars().take(200).collect())
}

fn new_connection_args(args: &Value) -> AppResult<NewConnection> {
    let name = name_arg(args).ok_or_else(|| invalid("name is required."))?;
    let uri = str_arg(args, "uri").ok_or_else(|| invalid("uri is required."))?;
    let (uri, username, had_password) = checked_uri(&uri)?;
    Ok(NewConnection {
        name,
        uri,
        username,
        had_password,
        folder: str_arg(args, "folder"),
    })
}

struct ConnectionChange {
    name: Option<String>,
    uri: Option<String>,
    username: Option<String>,
    had_password: bool,
}

fn connection_change_args(args: &Value) -> AppResult<ConnectionChange> {
    let name = name_arg(args);
    let (uri, username, had_password) = match str_arg(args, "uri") {
        Some(uri) => {
            let (uri, username, had_password) = checked_uri(&uri)?;
            (Some(uri), username, had_password)
        }
        None => (None, None, false),
    };
    if name.is_none() && uri.is_none() {
        return Err(invalid("Pass a new name, a new uri, or both."));
    }
    Ok(ConnectionChange {
        name,
        uri,
        username,
        had_password,
    })
}

fn now_secs() -> String {
    (now_ms() / 1000).to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionEvent<'a> {
    change: &'a str,
    connection_id: &'a str,
    name: &'a str,
    folder_id: Option<&'a str>,
    level: Option<Level>,
}

fn create_connection(call: &Call<'_>, args: &Value) -> AppResult<Done> {
    let new = new_connection_args(args)?;
    let folder = new
        .folder
        .as_deref()
        .and_then(|name| find_folder(&call.inner.host.sidebar_layout(), name));
    let now = now_secs();
    let profile = ConnectionProfile {
        id: String::new(),
        name: new.name.clone(),
        color: None,
        source: ConnectionSource::Uri { uri: new.uri },
        database: None,
        username: new.username.clone(),
        has_password: false,
        tls: Default::default(),
        ssh_tunnel: None,
        advanced: Default::default(),
        created_at: now.clone(),
        updated_at: now,
    };
    let saved = call.inner.state().connection_store.upsert(profile)?;
    // Usable right away; the frontend saves the level when it hears of it.
    call.inner
        .config
        .lock()
        .unwrap()
        .connections
        .insert(saved.id.clone(), Level::Read);
    call.inner.emit(
        CONNECTION_EVENT,
        &ConnectionEvent {
            change: "created",
            connection_id: &saved.id,
            name: &saved.name,
            folder_id: folder.as_ref().map(|(id, _)| id.as_str()),
            level: Some(Level::Read),
        },
    );
    log::info!(
        "MCP {}: saved connection \"{}\" ({}) at {}",
        call.client.name,
        saved.name,
        saved.id,
        logging::route(&saved)
    );
    let mut text = format!(
        "Saved the connection \"{}\" with id {}. You have read access to it; the user can \
         change that in Mongo Studio's settings.",
        saved.name, saved.id
    );
    match (&folder, &new.folder) {
        (Some((_, label)), _) => text.push_str(&format!(" It is in the folder \"{label}\".")),
        (None, Some(name)) => text.push_str(&format!(
            " There is no folder named \"{name}\", so it is at the top of the sidebar."
        )),
        _ => {}
    }
    if new.had_password {
        text.push_str(" The password in the uri was removed and not saved.");
    }
    if let Some(user) = &new.username {
        text.push_str(&format!(
            " It logs in as \"{user}\": the user must open the connection in Mongo Studio and \
             enter the password before it can be used (passwords never go through MCP)."
        ));
    }
    Ok(Done {
        text,
        meta: saved.name,
    })
}

async fn update_connection(call: &Call<'_>, args: &Value) -> AppResult<Done> {
    let change = connection_change_args(args)?;
    let state = call.inner.state();
    let mut profile = call.target.profile.clone().expect("checked before");
    let mut notes = Vec::new();
    if let Some(name) = &change.name {
        profile.name = name.clone();
    }
    if let Some(uri) = change.uri {
        profile.source = ConnectionSource::Uri { uri };
        if change.username.is_some() && change.username != profile.username {
            profile.username = change.username.clone();
            if profile.has_password {
                profile.has_password = false;
                let _ = state
                    .secret_store
                    .delete(&profile.id, crate::secrets::SecretKind::Password);
            }
            notes.push(format!(
                "It now logs in as \"{}\": the user must enter the password in Mongo Studio.",
                change.username.as_deref().unwrap_or_default()
            ));
        }
        if change.had_password {
            notes.push("The password in the uri was removed and not saved.".to_string());
        }
    }
    profile.updated_at = now_secs();
    let saved = state.connection_store.upsert(profile)?;
    call.inner
        .pool
        .close(&saved.id, "the connection was edited")
        .await;
    call.inner.emit(
        CONNECTION_EVENT,
        &ConnectionEvent {
            change: "updated",
            connection_id: &saved.id,
            name: &saved.name,
            folder_id: None,
            level: None,
        },
    );
    log::info!(
        "MCP {}: edited connection \"{}\" ({}) at {}",
        call.client.name,
        saved.name,
        saved.id,
        logging::route(&saved)
    );
    let mut text = format!("Updated the connection \"{}\".", saved.name);
    for note in notes {
        text.push(' ');
        text.push_str(&note);
    }
    Ok(Done {
        text,
        meta: saved.name,
    })
}

async fn delete_connection(call: &Call<'_>) -> AppResult<Done> {
    let profile = call.target.profile.clone().expect("checked before");
    let state = call.inner.state();
    state.connection_store.delete(&profile.id)?;
    let _ = state.secret_store.delete_all(&profile.id);
    call.inner
        .pool
        .close(&profile.id, "the connection was deleted")
        .await;
    {
        let mut config = call.inner.config.lock().unwrap();
        config.connections.remove(&profile.id);
        config.always.remove(&profile.id);
    }
    call.inner.emit(
        CONNECTION_EVENT,
        &ConnectionEvent {
            change: "deleted",
            connection_id: &profile.id,
            name: &profile.name,
            folder_id: None,
            level: None,
        },
    );
    log::info!(
        "MCP {}: deleted connection \"{}\" ({})",
        call.client.name,
        profile.name,
        profile.id
    );
    Ok(Done {
        text: format!("Deleted the connection \"{}\".", profile.name),
        meta: profile.name,
    })
}

// --- Saved scripts ---

/// The script's name and its content as saved: a first line naming the
/// client, and a `use` line for its database.
fn script_save_args(args: &Value) -> AppResult<(String, String)> {
    let name = name_arg(args).ok_or_else(|| invalid("name is required."))?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| invalid("content is required."))?;
    let database = str_arg(args, "database");
    Ok((name, script_body(content, database.as_deref(), None)))
}

pub(crate) fn script_body(content: &str, database: Option<&str>, client: Option<&str>) -> String {
    let mut body = String::new();
    if let Some(client) = client {
        body.push_str(&format!(
            "// Saved by {client} through Mongo Studio's MCP server\n"
        ));
    }
    let starts_with_use = content
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("//"))
        .is_some_and(|l| l.trim_start().starts_with("use "));
    if let Some(db) = database.filter(|_| !starts_with_use) {
        body.push_str(&format!("use {db}\n"));
    }
    body.push_str(content);
    if !body.ends_with('\n') {
        body.push('\n');
    }
    body
}

/// A file name from a script name: letters, digits, spaces, `-`, `_`, `.`.
pub(crate) fn script_file_stem(name: &str) -> String {
    let name = name.trim();
    let name = name
        .strip_suffix(".js")
        .or_else(|| name.strip_suffix(".JS"))
        .unwrap_or(name);
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .take(80)
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if cleaned.is_empty() {
        "script".to_string()
    } else {
        cleaned.to_string()
    }
}

fn save_script(call: &Call<'_>, args: &Value) -> AppResult<Done> {
    let name = name_arg(args).ok_or_else(|| invalid("name is required."))?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let body = script_body(
        content,
        str_arg(args, "database").as_deref(),
        Some(&call.client.name),
    );
    let scripts = call.inner.host.saved_scripts();
    let folder = scripts
        .suggest_path()?
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| invalid("no folder for saved scripts"))?;
    let stem = script_file_stem(&name);
    let mut path = folder.join(format!("{stem}.js"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{stem} ({n}).js"));
        n += 1;
    }
    let saved = scripts.save(Some(&path.to_string_lossy()), &body)?;
    log::info!(
        "MCP {}: saved a script to {}",
        call.client.name,
        crate::assistant::detect::display_path(&path)
    );
    Ok(Done {
        text: format!(
            "Saved the script \"{name}\" to {}. The user finds it in Mongo Studio's Saved scripts.",
            saved.path
        ),
        meta: saved.name,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::super::test_support::{add_session, inner, set_policy, TestHost};
    use super::super::{REQUEST_CLOSED_EVENT, REQUEST_EVENT};
    use super::*;
    use crate::models::{ConnectionAdvancedOptions, TlsOptions};

    pub(crate) fn profile(id: &str, name: &str, uri: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: id.to_string(),
            name: name.to_string(),
            color: None,
            source: ConnectionSource::Uri {
                uri: uri.to_string(),
            },
            database: None,
            username: None,
            has_password: false,
            tls: TlsOptions::default(),
            ssh_tunnel: None,
            advanced: ConnectionAdvancedOptions::default(),
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        }
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

    fn names(config: &McpConfig) -> Vec<String> {
        definitions(config)
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn definitions_line_up_with_the_tools() {
        let all = all_definitions();
        assert_eq!(all.len(), Tool::ALL.len());
        for (tool, def) in Tool::ALL.into_iter().zip(&all) {
            assert_eq!(def["name"], tool.name());
            assert!(def["description"].as_str().unwrap().len() > 20);
            assert_eq!(def["inputSchema"]["type"], "object");
            assert_eq!(Tool::from_name(tool.name()), Some(tool));
        }
    }

    #[test]
    fn tools_list_leaves_out_capabilities_that_are_off() {
        let browse_only = names(&config(&[], &[]));
        assert_eq!(
            browse_only,
            [
                "list_databases",
                "list_collections",
                "list_indexes",
                "sample_schema"
            ]
        );
        let more = names(&config(
            &[
                ("connList", Policy::Allow),
                ("read", Policy::Ask),
                ("write", Policy::Ask),
                ("saveScript", Policy::Off),
            ],
            &[],
        ));
        assert!(more.contains(&"list_connections".to_string()));
        assert!(more.contains(&"find".to_string()));
        assert!(more.contains(&"update_documents".to_string()));
        assert!(!more.contains(&"save_script".to_string()));
        assert!(!more.contains(&"drop_collection".to_string()));
        assert!(!more.contains(&"run_script".to_string()));
    }

    #[test]
    fn dry_run_summaries() {
        assert_eq!(
            dry_run_text(DryRun::Update {
                matched: 12,
                upsert: false
            }),
            "12 documents would change"
        );
        assert_eq!(
            dry_run_text(DryRun::Update {
                matched: 1,
                upsert: false
            }),
            "1 document would change"
        );
        assert_eq!(
            dry_run_text(DryRun::Update {
                matched: 0,
                upsert: false
            }),
            "No documents match the filter; nothing would change"
        );
        assert!(dry_run_text(DryRun::Update {
            matched: 0,
            upsert: true
        })
        .contains("upsert"));
        assert_eq!(
            dry_run_text(DryRun::Delete(1234)),
            "1,234 documents would be deleted"
        );
        assert_eq!(
            dry_run_text(DryRun::Insert(1)),
            "1 document would be inserted"
        );
        assert_eq!(
            dry_run_text(DryRun::Drop(Some(3))),
            "The collection and its 3 documents would be deleted"
        );
    }

    #[test]
    fn code_reads_like_mongosh() {
        assert_eq!(
            code_on("shop", Some("users"), "updateMany({}, {})"),
            "use shop\ndb.getCollection(\"users\").updateMany({}, {})"
        );
        assert_eq!(
            code_on("shop", None, "show collections"),
            "use shop\nshow collections"
        );
    }

    #[test]
    fn script_names_and_bodies() {
        assert_eq!(script_file_stem("Monthly report.js"), "Monthly report");
        assert_eq!(script_file_stem("../../etc/passwd"), "-..-etc-passwd");
        assert_eq!(script_file_stem("  "), "script");
        assert_eq!(
            script_body("db.users.find()", Some("shop"), Some("Cursor")),
            "// Saved by Cursor through Mongo Studio's MCP server\nuse shop\ndb.users.find()\n"
        );
        assert_eq!(
            script_body("use other\ndb.x.find()\n", Some("shop"), None),
            "use other\ndb.x.find()\n"
        );
    }

    #[test]
    fn folders_from_the_sidebar_layout() {
        let layout = json!({ "version": 1, "root": [
            { "type": "connection", "id": "top" },
            { "type": "folder", "id": "f1", "name": "Prod", "children": [
                { "type": "connection", "id": "c1" },
                { "type": "folder", "id": "f2", "name": "EU", "children": [
                    { "type": "connection", "id": "c2" }
                ] }
            ] }
        ] });
        let folders = folders_by_connection(&layout);
        assert_eq!(folders["c1"], ("f1".to_string(), "Prod".to_string()));
        assert_eq!(folders["c2"], ("f2".to_string(), "Prod / EU".to_string()));
        assert!(!folders.contains_key("top"));
        assert_eq!(find_folder(&layout, "eu").unwrap().0, "f2");
        assert_eq!(find_folder(&layout, "Prod / EU").unwrap().0, "f2");
        assert!(find_folder(&layout, "Staging").is_none());
    }

    async fn run(inner: &Arc<Inner>, name: &str, args: Value) -> ToolOutput {
        let client = add_session(inner, "s1", "Cursor");
        call(inner, &client, name, args).await.unwrap()
    }

    fn last_activity(host: &TestHost) -> Value {
        host.events_named(ACTIVITY_EVENT).pop().unwrap()
    }

    #[tokio::test]
    async fn refusals_are_tool_errors_with_activity() {
        let (host, inner) = inner(Duration::from_secs(1));
        set_policy(
            &inner,
            config(&[("read", Policy::Allow)], &[("c1", Level::Read)]),
        );

        let out = run(
            &inner,
            "insert_documents",
            json!({ "connection_id": "c1", "database": "shop", "collection": "users", "documents": [{}] }),
        )
        .await;
        assert!(out.is_error);
        assert_eq!(out.text, policy::REFUSED_OFF);
        let activity = last_activity(&host);
        assert_eq!(activity["label"], "Refused: turned off");
        assert_eq!(activity["result"], "warn");
        assert_eq!(activity["clientName"], "Cursor");
        assert_eq!(activity["target"], "shop.users");
        assert_eq!(activity["connectionId"], "c1");

        let out = run(
            &inner,
            "find",
            json!({ "connection_id": "c2", "database": "shop", "collection": "users" }),
        )
        .await;
        assert!(out.is_error);
        assert_eq!(out.text, policy::REFUSED_NO_ACCESS);
        assert_eq!(last_activity(&host)["label"], "Refused: no access");

        // A pipeline that writes needs write.
        let out = run(
            &inner,
            "aggregate",
            json!({ "connection_id": "c1", "database": "shop", "collection": "users",
                    "pipeline": [{ "$match": {} }, { "$out": "copy" }] }),
        )
        .await;
        assert_eq!(out.text, policy::REFUSED_OFF);

        let out = run(
            &inner,
            "find",
            json!({ "database": "shop", "collection": "users" }),
        )
        .await;
        assert!(out.is_error);
        assert!(out.text.contains("connection_id is required"));
        assert_eq!(last_activity(&host)["result"], "err");

        assert!(call(
            &inner,
            &add_session(&inner, "s1", "Cursor"),
            "drop_database",
            json!({})
        )
        .await
        .is_none());
    }

    #[tokio::test]
    async fn list_connections_shows_only_shared_ones_without_secrets() {
        let (host, inner) = inner(Duration::from_secs(1));
        let store = &host.state.connection_store;
        store
            .upsert(profile(
                "c1",
                "Local",
                "mongodb://localhost:27017/?tls=false",
            ))
            .unwrap();
        store
            .upsert(profile("c2", "Hidden", "mongodb://hidden:27017"))
            .unwrap();
        *host.layout.lock().unwrap() = json!({ "root": [
            { "type": "folder", "id": "f1", "name": "Dev", "children": [{ "type": "connection", "id": "c1" }] }
        ] });
        set_policy(
            &inner,
            config(&[("connList", Policy::Allow)], &[("c1", Level::Write)]),
        );
        let out = run(&inner, "list_connections", json!({})).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("1 connection you may use"));
        let line: Value = serde_json::from_str(out.text.lines().nth(1).unwrap()).unwrap();
        assert_eq!(line["id"], "c1");
        assert_eq!(line["folder"], "Dev");
        assert_eq!(line["access"], "read & write");
        assert_eq!(line["hosts"], "mongodb://localhost:27017");
        assert!(!out.text.contains("Hidden"));
        assert!(!out.text.contains("tls=false"));
    }

    #[tokio::test]
    async fn create_update_and_delete_connections() {
        let (host, inner) = inner(Duration::from_secs(1));
        *host.layout.lock().unwrap() = json!({ "root": [
            { "type": "folder", "id": "f1", "name": "Dev", "children": [] }
        ] });
        set_policy(
            &inner,
            config(
                &[("connEdit", Policy::Allow), ("connDelete", Policy::Allow)],
                &[],
            ),
        );
        let out = run(
            &inner,
            "create_connection",
            json!({ "name": "Atlas", "uri": "mongodb+srv://ana:s3cret@cluster0.example.net/app", "folder": "dev" }),
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert!(!out.text.contains("s3cret"));
        assert!(out.text.contains("password"));
        let saved = host.state.connection_store.list();
        assert_eq!(saved.len(), 1);
        let created = &saved[0];
        assert_eq!(created.username.as_deref(), Some("ana"));
        assert!(!created.has_password);
        match &created.source {
            ConnectionSource::Uri { uri } => {
                assert_eq!(uri, "mongodb+srv://cluster0.example.net/app")
            }
            other => panic!("{other:?}"),
        }
        // Readable right away, and announced so the frontend saves it.
        assert_eq!(inner.config().connections[&created.id], Level::Read);
        let event = host.events_named(CONNECTION_EVENT).pop().unwrap();
        assert_eq!(event["change"], "created");
        assert_eq!(event["connectionId"], created.id.as_str());
        assert_eq!(event["folderId"], "f1");
        assert_eq!(event["level"], "read");
        assert!(host
            .events
            .lock()
            .unwrap()
            .iter()
            .all(|(_, v)| !v.to_string().contains("s3cret")));

        let id = created.id.clone();
        let out = run(
            &inner,
            "update_connection",
            json!({ "connection_id": id, "name": "Atlas prod" }),
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(
            host.state.connection_store.get(&id).unwrap().name,
            "Atlas prod"
        );

        let out = run(&inner, "delete_connection", json!({ "connection_id": id })).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(host.state.connection_store.list().is_empty());
        assert!(!inner.config().connections.contains_key(&id));
        assert_eq!(
            host.events_named(CONNECTION_EVENT).pop().unwrap()["change"],
            "deleted"
        );
    }

    #[tokio::test]
    async fn save_script_waits_for_the_user_and_times_out() {
        let (host, inner) = inner(Duration::from_millis(50));
        set_policy(&inner, config(&[("saveScript", Policy::Ask)], &[]));
        let out = run(
            &inner,
            "save_script",
            json!({ "name": "report", "content": "db.orders.find()", "database": "shop" }),
        )
        .await;
        assert!(out.is_error);
        assert_eq!(out.text, policy::NOT_ANSWERED);
        let request = host.events_named(REQUEST_EVENT).pop().unwrap();
        assert_eq!(request["title"], "Save the script \"report\"");
        assert_eq!(request["capability"], "saveScript");
        assert!(request["connectionId"].is_null());
        assert_eq!(
            host.events_named(REQUEST_CLOSED_EVENT).pop().unwrap()["reason"],
            "timeout"
        );
        assert_eq!(last_activity(&host)["label"], "Not answered");
    }

    #[tokio::test]
    async fn save_script_approved_writes_the_file() {
        let (host, inner) = inner(Duration::from_secs(5));
        set_policy(&inner, config(&[("saveScript", Policy::Ask)], &[]));
        let waiting = {
            let inner = inner.clone();
            tokio::spawn(async move {
                run(
                    &inner,
                    "save_script",
                    json!({ "name": "report", "content": "db.orders.find()", "database": "shop" }),
                )
                .await
            })
        };
        let request = loop {
            if let Some(event) = host.events_named(REQUEST_EVENT).pop() {
                break event;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        policy::answer(
            &inner,
            request["requestId"].as_str().unwrap(),
            super::super::RequestChoice::Always,
        )
        .unwrap();
        let out = waiting.await.unwrap();
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(last_activity(&host)["label"], "Approved by you (always)");
        let scripts = host.scripts.list().unwrap();
        assert_eq!(scripts.len(), 1);
        let content = std::fs::read_to_string(&scripts[0].path).unwrap();
        assert!(content.starts_with("// Saved by Cursor"));
        assert!(content.contains("use shop\ndb.orders.find()"));

        // "Always" holds for the next call.
        let out = run(
            &inner,
            "save_script",
            json!({ "name": "report", "content": "db.x.find()" }),
        )
        .await;
        assert!(!out.is_error);
        assert_eq!(last_activity(&host)["label"], "Allowed (always)");
        assert_eq!(host.scripts.list().unwrap().len(), 2);
    }
}

#[cfg(test)]
mod live_tests {
    //! Against a live MongoDB named by `MONGO_STUDIO_TEST_URI`, in the
    //! throwaway database the other live tests use (`mongo_studio_test`),
    //! on a fresh collection per run. Run with:
    //!   MONGO_STUDIO_TEST_URI=mongodb://localhost:27017 \
    //!     cargo test --lib -- --ignored mcp_server::tools::live_tests
    use std::collections::HashMap;
    use std::time::Duration;

    use super::super::test_support::{add_session, inner, set_policy, TestHost};
    use super::super::{RequestChoice, REQUEST_EVENT};
    use super::tests::profile;
    use super::*;

    const TEST_DB: &str = "mongo_studio_test";

    fn setup(ask: Duration) -> (Arc<TestHost>, Arc<Inner>) {
        let uri = std::env::var("MONGO_STUDIO_TEST_URI")
            .expect("set MONGO_STUDIO_TEST_URI to run this test");
        let (host, inner) = inner(ask);
        host.state
            .connection_store
            .upsert(profile("live", "Live", &uri))
            .unwrap();
        let caps = ["read", "write", "ddl", "script"]
            .map(|c| (c.to_string(), Policy::Allow))
            .into_iter()
            .collect();
        set_policy(
            &inner,
            McpConfig {
                enabled: true,
                port: 0,
                caps,
                connections: HashMap::from([("live".to_string(), Level::Write)]),
                always: HashMap::new(),
                alerts: Default::default(),
            },
        );
        (host, inner)
    }

    async fn run(inner: &Arc<Inner>, name: &str, args: Value) -> ToolOutput {
        let client = add_session(inner, "s1", "Live test");
        let out = call(inner, &client, name, args).await.unwrap();
        out
    }

    fn on(coll: &str, extra: Value) -> Value {
        let mut args = json!({ "connection_id": "live", "database": TEST_DB, "collection": coll });
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        args
    }

    #[tokio::test]
    #[ignore]
    async fn live_browse_read_write_ddl_and_script() {
        let (_host, inner) = setup(Duration::from_secs(1));
        let coll = format!("mcp_{}", uuid::Uuid::new_v4().simple());

        let out = run(&inner, "create_collection", on(&coll, json!({}))).await;
        assert!(!out.is_error, "{}", out.text);
        let out = run(
            &inner,
            "insert_documents",
            on(
                &coll,
                json!({ "documents": [
                { "status": "shipped", "total": 42 },
                { "status": "shipped", "total": 17 },
                { "status": "pending", "total": 5 }
            ] }),
            ),
        )
        .await;
        assert!(out.text.starts_with("Inserted 3 documents"), "{}", out.text);

        let out = run(&inner, "list_databases", json!({ "connection_id": "live" })).await;
        assert!(out.text.contains(TEST_DB), "{}", out.text);
        let out = run(
            &inner,
            "list_collections",
            json!({ "connection_id": "live", "database": TEST_DB }),
        )
        .await;
        assert!(out.text.contains(&coll), "{}", out.text);
        let out = run(&inner, "sample_schema", on(&coll, json!({}))).await;
        assert!(out.text.contains("status"), "{}", out.text);

        let out = run(
            &inner,
            "find",
            on(
                &coll,
                json!({ "filter": { "status": "shipped" }, "sort": { "total": -1 }, "limit": 1 }),
            ),
        )
        .await;
        assert!(out.text.starts_with("Returned 1 document"), "{}", out.text);
        assert!(out.text.contains("\"total\":42"), "{}", out.text);
        assert!(out.text.contains("skip=1"), "{}", out.text);

        let out = run(
            &inner,
            "count",
            on(&coll, json!({ "filter": { "status": "shipped" } })),
        )
        .await;
        assert_eq!(
            out.text,
            format!("2 documents in {TEST_DB}.{coll} match the filter.")
        );

        let out = run(
            &inner,
            "aggregate",
            on(
                &coll,
                json!({ "pipeline": [{ "$group": { "_id": "$status", "n": { "$sum": 1 } } }] }),
            ),
        )
        .await;
        assert!(out.text.contains("returned 2 documents"), "{}", out.text);

        let out = run(
            &inner,
            "explain",
            on(&coll, json!({ "filter": { "status": "shipped" } })),
        )
        .await;
        assert!(out.text.contains("Winning plan"), "{}", out.text);

        let out = run(
            &inner,
            "update_documents",
            on(&coll, json!({ "filter": { "status": "shipped" }, "update": { "$set": { "status": "done" } }, "many": true })),
        )
        .await;
        assert!(
            out.text.starts_with(
                "Before running: 2 documents would change.\nMatched 2 documents, modified 2"
            ),
            "{}",
            out.text
        );

        let out = run(
            &inner,
            "create_index",
            on(
                &coll,
                json!({ "keys": { "status": 1 }, "name": "by_status" }),
            ),
        )
        .await;
        assert!(out.text.contains("by_status"), "{}", out.text);
        let out = run(&inner, "list_indexes", on(&coll, json!({}))).await;
        assert!(out.text.contains("by_status"), "{}", out.text);
        let out = run(
            &inner,
            "drop_index",
            on(&coll, json!({ "name": "by_status" })),
        )
        .await;
        assert!(!out.is_error, "{}", out.text);

        let out = run(
            &inner,
            "run_script",
            json!({ "connection_id": "live", "database": TEST_DB,
                    "script": format!("print('hi'); await db.getCollection('{coll}').countDocuments({{}})") }),
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("3"), "{}", out.text);
        assert!(out.text.contains("hi"), "{}", out.text);

        let out = run(
            &inner,
            "delete_documents",
            on(&coll, json!({ "filter": { "status": "done" } })),
        )
        .await;
        assert!(
            out.text
                .starts_with("Before running: 1 document would be deleted.\nDeleted 1 document"),
            "{}",
            out.text
        );

        let out = run(&inner, "drop_collection", on(&coll, json!({}))).await;
        assert!(out.text.contains("Dropped the collection"), "{}", out.text);
        assert_eq!(inner.pool.len().await, 1);
        inner.pool.close_all("test over").await;
    }

    #[tokio::test]
    #[ignore]
    async fn live_write_waits_for_approval_with_a_dry_run() {
        let (host, inner) = setup(Duration::from_secs(5));
        inner
            .config
            .lock()
            .unwrap()
            .caps
            .insert("write".to_string(), Policy::Ask);
        // A fresh collection: nothing matches, and nothing is at risk.
        let coll = format!("mcp_{}", uuid::Uuid::new_v4().simple());
        let waiting = {
            let inner = inner.clone();
            let coll = coll.clone();
            tokio::spawn(async move {
                run(
                    &inner,
                    "delete_documents",
                    on(&coll, json!({ "filter": {}, "many": true })),
                )
                .await
            })
        };
        let request = loop {
            if let Some(event) = host.events_named(REQUEST_EVENT).pop() {
                break event;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(request["tool"], "delete_documents");
        assert_eq!(
            request["dryRun"],
            "No documents match the filter; nothing would be deleted"
        );
        assert_eq!(request["title"], format!("deleteMany on {TEST_DB}.{coll}"));
        policy::answer(
            &inner,
            request["requestId"].as_str().unwrap(),
            RequestChoice::Deny,
        )
        .unwrap();
        let out = waiting.await.unwrap();
        assert_eq!(out.text, policy::DENIED);
        inner.pool.close_all("test over").await;
    }

    /// The whole path a client takes, over HTTP, reading the dev `shop`
    /// database without changing it.
    #[tokio::test]
    #[ignore]
    async fn live_http_round_trip_reads_shop() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (_host, inner) = setup(Duration::from_secs(1));
        inner
            .config
            .lock()
            .unwrap()
            .caps
            .insert("connList".into(), Policy::Allow);
        let config = inner.config();
        super::super::server::apply_config(&inner, config).await;
        let port = inner.status().port;
        let token = inner.token().unwrap();
        let post = |session: Option<String>, body: Value| {
            let token = token.clone();
            async move {
                let body = body.to_string();
                let session = session
                    .map(|s| format!("Mcp-Session-Id: {s}\r\n"))
                    .unwrap_or_default();
                let request = format!(
                    "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\n{session}Content-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .unwrap();
                stream.write_all(request.as_bytes()).await.unwrap();
                let mut raw = String::new();
                stream.read_to_string(&mut raw).await.unwrap();
                let (head, body) = raw.split_once("\r\n\r\n").unwrap();
                (
                    head.to_string(),
                    serde_json::from_str::<Value>(body).unwrap(),
                )
            }
        };
        let (head, _) = post(
            None,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "clientInfo": { "name": "live" } } }),
        )
        .await;
        let session = head
            .lines()
            .find_map(|l| l.strip_prefix("Mcp-Session-Id: "))
            .unwrap()
            .to_string();
        let tool = |name: &str, args: Value| json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": name, "arguments": args } });
        let (_, answer) = post(Some(session.clone()), tool("list_connections", json!({}))).await;
        assert!(answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"live\""));
        let (_, answer) = post(
            Some(session.clone()),
            tool(
                "list_collections",
                json!({ "connection_id": "live", "database": "shop" }),
            ),
        )
        .await;
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(answer["result"]["isError"], false, "{text}");
        let Some(first) = text.lines().nth(1) else {
            println!("shop has no collections, skipping the count");
            return;
        };
        let coll: Value = serde_json::from_str(first).unwrap();
        let (_, answer) = post(
            Some(session),
            tool(
                "count",
                json!({ "connection_id": "live", "database": "shop", "collection": coll["name"] }),
            ),
        )
        .await;
        let text = answer["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("match the filter"), "{text}");
        inner.pool.close_all("test over").await;
    }
}

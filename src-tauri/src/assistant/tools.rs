//! The MCP tools: read-only views of the session's database.
//!
//! Every call is bound to the database of the session whose token made it,
//! checks the policy the frontend set (connection shared, indexes shared,
//! document values allowed), and reports itself to the windows as an
//! `assistant-step` when it starts and when it ends. Tools that return
//! document values ask the user first unless the policy already allows it.
//!
//! Nothing here can write: filters only reach find/count/explain, and
//! pipelines are refused when any stage (nested ones included) is `$out` or
//! `$merge`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::TryStreamExt;
use mongodb::bson::{doc, Document};
use mongodb::Client;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::schema;
use super::{
    AgentSession, ApprovalChoice, ApprovalEvent, Inner, PendingApproval, StepEvent, StepState,
    APPROVAL_EVENT, STEP_EVENT,
};
use crate::driver;
use crate::ejson::{document_to_json, json_to_document};

/// Server-side limit on every operation.
const MAX_TIME: Duration = Duration::from_secs(15);
/// Client-side backstop for operations that take no `maxTimeMS`.
const OP_TIMEOUT: Duration = Duration::from_secs(20);
/// Tool output is cut here, to keep the agent's context small.
const MAX_OUTPUT: usize = 20 * 1024;
const MAX_META: usize = 120;
/// Aggregations count up to this many results, then say "1,000+".
const AGGREGATE_CAP: i64 = 1000;

pub(crate) const REFUSED_WRITE: &str =
    "Refused: $out and $merge write data, and the Assistant's tools are read-only.";
const REFUSED_INDEXES: &str = "Indexes and explain plans aren't shared with the Assistant.";
const REFUSED_CONNECTION: &str = "This connection isn't shared with the Assistant.";
const DENIED_VALUES: &str =
    "The user declined to share document values. Work from field names and types, or ask them.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    ListCollections,
    SampleSchema,
    ListIndexes,
    Explain,
    Count,
    Find,
    Aggregate,
}

impl Tool {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "list_collections" => Tool::ListCollections,
            "sample_schema" => Tool::SampleSchema,
            "list_indexes" => Tool::ListIndexes,
            "explain" => Tool::Explain,
            "count" => Tool::Count,
            "find" => Tool::Find,
            "aggregate" => Tool::Aggregate,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Tool::ListCollections => "list_collections",
            Tool::SampleSchema => "sample_schema",
            Tool::ListIndexes => "list_indexes",
            Tool::Explain => "explain",
            Tool::Count => "count",
            Tool::Find => "find",
            Tool::Aggregate => "aggregate",
        }
    }

    /// Labels for a running, finished and failed step on `target`
    /// (`db` or `db.coll`).
    fn labels(self, target: &str) -> (String, String, String) {
        let (run, ok, fail) = match self {
            Tool::ListCollections => (
                "Listing collections in",
                "Listed collections in",
                "Couldn't list collections in",
            ),
            Tool::SampleSchema => ("Sampling", "Sampled", "Couldn't sample"),
            Tool::ListIndexes => (
                "Reading indexes on",
                "Read indexes on",
                "Couldn't read indexes on",
            ),
            Tool::Explain => (
                "Explaining the query on",
                "Explained the query on",
                "Couldn't explain the query on",
            ),
            Tool::Count => ("Counting", "Counted", "Couldn't count"),
            Tool::Find => (
                "Reading documents from",
                "Read documents from",
                "Couldn't read documents from",
            ),
            Tool::Aggregate => (
                "Dry-running the pipeline on",
                "Dry-ran the pipeline on",
                "Couldn't dry-run the pipeline on",
            ),
        };
        (
            format!("{run} {target}"),
            format!("{ok} {target}"),
            format!("{fail} {target}"),
        )
    }
}

/// The `tools/list` result.
pub(crate) fn definitions() -> Value {
    let collection =
        json!({ "type": "string", "description": "Collection name in the session's database." });
    let filter = json!({ "type": "object", "description": "Query filter, MongoDB Extended JSON (e.g. {\"_id\": {\"$oid\": \"...\"}}, {\"createdAt\": {\"$gte\": {\"$date\": \"2026-01-01T00:00:00Z\"}}})." });
    let pipeline = json!({ "type": "array", "items": { "type": "object" }, "description": "Aggregation pipeline stages, Extended JSON. $out and $merge are refused." });
    let read_only =
        json!({ "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false });
    json!([
        {
            "name": "list_collections",
            "description": "List the collections and views in the session's database, with their type (collection, view, timeseries).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read_only,
        },
        {
            "name": "sample_schema",
            "description": "Sample documents from a collection and return its field paths with their BSON types and how many sampled documents have each field. Names and types only, never values. Use it before writing any query.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "collection": collection,
                    "size": { "type": "integer", "minimum": 1, "maximum": 500, "default": 200, "description": "How many documents to sample." }
                },
                "required": ["collection"],
            },
            "annotations": read_only,
        },
        {
            "name": "list_indexes",
            "description": "List a collection's indexes: name, key specification, and whether they are unique.",
            "inputSchema": { "type": "object", "properties": { "collection": collection }, "required": ["collection"] },
            "annotations": read_only,
        },
        {
            "name": "explain",
            "description": "Run explain (executionStats) for a find (filter and sort) or an aggregation pipeline, and summarize the winning plan: stages, index used, keys and documents examined, documents returned, time.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "collection": collection,
                    "filter": filter,
                    "sort": { "type": "object", "description": "Sort specification, e.g. {\"createdAt\": -1}." },
                    "pipeline": pipeline,
                },
                "required": ["collection"],
            },
            "annotations": read_only,
        },
        {
            "name": "count",
            "description": "Count the documents matching a filter. Returns only the number.",
            "inputSchema": { "type": "object", "properties": { "collection": collection, "filter": filter }, "required": ["collection"] },
            "annotations": read_only,
        },
        {
            "name": "find",
            "description": "Read documents (their values) as relaxed Extended JSON. May ask the user for permission first; prefer sample_schema, count and aggregate when names and types are enough.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "collection": collection,
                    "filter": filter,
                    "projection": { "type": "object", "description": "Projection, e.g. {\"status\": 1}." },
                    "sort": { "type": "object", "description": "Sort specification." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 20 },
                },
                "required": ["collection"],
            },
            "annotations": read_only,
        },
        {
            "name": "aggregate",
            "description": "Dry-run an aggregation pipeline: returns how many documents it outputs (up to 1,000+), how long it took, and the output's field names and types. Set include_documents to also get the first documents' values, which may ask the user first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "collection": collection,
                    "pipeline": pipeline,
                    "include_documents": { "type": "boolean", "default": false },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 20, "description": "How many documents to include when include_documents is true." },
                },
                "required": ["collection", "pipeline"],
            },
            "annotations": read_only,
        },
    ])
}

/// What `initialize` tells the agent about this server.
pub(crate) const INSTRUCTIONS: &str = "Read-only access to one MongoDB database in Mongo Studio. \
list_collections lists what is there; sample_schema gives a collection's field paths and types \
(no values); list_indexes and explain show indexes and query plans; count counts matches; \
aggregate dry-runs a pipeline and reports its result count, time and output fields; find \
(and aggregate with include_documents) return document values and may ask the user first. \
Nothing can write: $out and $merge are refused.";

pub(crate) struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    fn ok(text: String) -> Self {
        Self {
            text: truncate(text, MAX_OUTPUT),
            is_error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: truncate(text.into(), MAX_OUTPUT),
            is_error: true,
        }
    }
}

/// A successful operation: text for the agent, and the step's final label
/// (when it depends on the result) and meta.
struct Done {
    text: String,
    label: Option<String>,
    meta: String,
}

/// Reports one tool call to the windows.
struct Step<'a> {
    inner: &'a Inner,
    session: &'a AgentSession,
    id: String,
    tool: Tool,
    call: String,
}

impl Step<'_> {
    fn emit(&self, state: StepState, label: &str, meta: &str, out: &str) {
        self.inner.emit(
            STEP_EVENT,
            &StepEvent {
                agent_session_id: self.session.id.clone(),
                step_id: self.id.clone(),
                tool: self.tool.name().to_string(),
                state,
                label: label.to_string(),
                meta: truncate(meta.to_string(), MAX_META),
                call: self.call.clone(),
                out: truncate(out.to_string(), 200),
            },
        );
    }
}

/// Runs a `tools/call`. `None` when there's no tool by that name.
pub(crate) async fn call(
    inner: &Arc<Inner>,
    session: &Arc<AgentSession>,
    name: &str,
    args: Value,
) -> Option<ToolOutput> {
    let tool = Tool::from_name(name)?;
    let args = match args {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };
    let step = Step {
        inner,
        session,
        id: Uuid::new_v4().to_string(),
        tool,
        call: truncate(format!("{}({})", tool.name(), args), 2000),
    };

    let db = session.database.as_str();
    let collection = match tool {
        Tool::ListCollections => None,
        _ => match str_arg(&args, "collection").or(session.collection.clone()) {
            Some(c) => Some(c),
            None => {
                let (_, _, fail) = tool.labels(db);
                step.emit(StepState::Error, &fail, "collection is required", "");
                return Some(ToolOutput::error("The collection argument is required."));
            }
        },
    };
    let target = match &collection {
        Some(c) => format!("{db}.{c}"),
        None => db.to_string(),
    };
    let (run_label, ok_label, fail_label) = tool.labels(&target);

    let policy = inner.policy();
    if !policy.allowed_connections.contains(&session.connection_id) {
        step.emit(
            StepState::Denied,
            &run_label,
            "not shared with the Assistant",
            "",
        );
        return Some(ToolOutput::error(REFUSED_CONNECTION));
    }
    if matches!(tool, Tool::ListIndexes | Tool::Explain) && !policy.indexes {
        step.emit(
            StepState::Denied,
            &run_label,
            "not shared with the Assistant",
            "",
        );
        return Some(ToolOutput::error(REFUSED_INDEXES));
    }

    step.emit(StepState::Run, &run_label, "", "");

    let wants_values = match tool {
        Tool::Find => true,
        Tool::Aggregate => args
            .get("include_documents")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        _ => false,
    };
    let limit = match tool {
        Tool::Find | Tool::Aggregate => int_arg(&args, "limit", 20, 50),
        _ => 0,
    };
    let grant = if wants_values {
        match request_values(inner, session, tool, collection.as_deref(), limit).await {
            Some(grant) => Some(grant),
            None => {
                step.emit(
                    StepState::Denied,
                    "Reading document values",
                    "denied by you",
                    "",
                );
                return Some(ToolOutput::error(DENIED_VALUES));
            }
        }
    } else {
        None
    };

    let Some(client) = inner.host.client(session.mongo_session_id.clone()).await else {
        let message = "The connection to the database was closed.";
        step.emit(StepState::Error, &fail_label, message, "");
        return Some(ToolOutput::error(message));
    };

    let coll = collection.as_deref().unwrap_or_default();
    let result = tokio::time::timeout(
        OP_TIMEOUT,
        run(tool, &client, db, coll, &target, &args, limit, grant),
    )
    .await
    .unwrap_or_else(|_| Err("The operation timed out.".to_string()));

    Some(match result {
        Ok(done) => {
            let out = done.text.lines().next().unwrap_or_default().to_string();
            step.emit(
                StepState::Ok,
                done.label.as_deref().unwrap_or(&ok_label),
                &done.meta,
                &out,
            );
            ToolOutput::ok(done.text)
        }
        Err(message) => {
            step.emit(StepState::Error, &fail_label, &message, "");
            ToolOutput::error(message)
        }
    })
}

/// Whether the tool may return document values: `Some(meta)` describing
/// why, or `None` when the user declined. Asks the user unless the policy
/// already allows it; the question waits for `assistant_answer` or for the
/// session to stop.
async fn request_values(
    inner: &Arc<Inner>,
    session: &Arc<AgentSession>,
    tool: Tool,
    collection: Option<&str>,
    limit: i64,
) -> Option<String> {
    let policy = inner.policy();
    if policy.values {
        return Some("document values on".to_string());
    }
    let always = format!("always allowed on {}", session.connection_name);
    if policy.values_connections.contains(&session.connection_id) {
        return Some(always);
    }
    if !inner.is_live(session) {
        return None;
    }
    let request_id = Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    inner.approvals.lock().unwrap().insert(
        request_id.clone(),
        PendingApproval {
            agent_session_id: session.id.clone(),
            tx,
        },
    );
    inner.emit(
        APPROVAL_EVENT,
        &ApprovalEvent {
            agent_session_id: session.id.clone(),
            request_id,
            tool: tool.name().to_string(),
            database: session.database.clone(),
            collection: collection.map(str::to_string),
            limit: Some(limit),
            provider: session.agent.provider().to_string(),
        },
    );
    match rx.await {
        Ok(ApprovalChoice::Once) => Some("allowed once".to_string()),
        Ok(ApprovalChoice::Always) => Some(always),
        Ok(ApprovalChoice::Deny) | Err(_) => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    tool: Tool,
    client: &Client,
    db: &str,
    coll: &str,
    target: &str,
    args: &Value,
    limit: i64,
    grant: Option<String>,
) -> Result<Done, String> {
    let err = |e: crate::error::AppError| e.to_string();
    match tool {
        Tool::ListCollections => {
            let list = driver::list_collections(client, db).await.map_err(err)?;
            let lines: Vec<String> = list
                .iter()
                .map(|c| format!("{} ({})", c.name, c.collection_type))
                .collect();
            Ok(Done {
                text: format!(
                    "{} in {db}:\n{}",
                    plural(list.len() as u64, "collection", "collections"),
                    lines.join("\n")
                ),
                label: None,
                meta: plural(list.len() as u64, "collection", "collections"),
            })
        }
        Tool::SampleSchema => sample_schema(client, db, coll, target, args).await,
        Tool::ListIndexes => {
            let indexes = driver::list_indexes(client, db, coll).await.map_err(err)?;
            let lines: Vec<String> = indexes
                .iter()
                .map(|i| {
                    let unique = if i.unique { " unique" } else { "" };
                    format!("{} {}{unique}", i.name, i.key)
                })
                .collect();
            let count = plural(indexes.len() as u64, "index", "indexes");
            Ok(Done {
                text: format!("{count} on {target}:\n{}", lines.join("\n")),
                label: None,
                meta: count,
            })
        }
        Tool::Explain => explain(client, db, coll, target, args).await,
        Tool::Count => {
            let filter = doc_arg(args, "filter")?;
            let n = collection(client, db, coll)
                .count_documents(filter)
                .max_time(MAX_TIME)
                .await
                .map_err(|e| e.to_string())?;
            let docs = plural(n, "document", "documents");
            Ok(Done {
                text: format!("{docs} in {target} match the filter."),
                label: None,
                meta: docs,
            })
        }
        Tool::Find => {
            let filter = doc_arg(args, "filter")?;
            let collection = collection(client, db, coll);
            let mut find = collection.find(filter).limit(limit).max_time(MAX_TIME);
            if let Some(projection) = opt_doc_arg(args, "projection")? {
                find = find.projection(projection);
            }
            if let Some(sort) = opt_doc_arg(args, "sort")? {
                find = find.sort(sort);
            }
            let docs: Vec<Document> = find
                .await
                .map_err(|e| e.to_string())?
                .try_collect()
                .await
                .map_err(|e| e.to_string())?;
            let n = docs.len() as u64;
            let label = format!("Read {} from {target}", plural(n, "document", "documents"));
            Ok(Done {
                text: format!(
                    "{label} (relaxed Extended JSON, one per line):\n{}",
                    json_lines(docs)
                ),
                label: Some(label),
                meta: grant.unwrap_or_default(),
            })
        }
        Tool::Aggregate => aggregate(client, db, coll, target, args, limit, grant.is_some()).await,
    }
}

fn collection(client: &Client, db: &str, coll: &str) -> mongodb::Collection<Document> {
    client.database(db).collection::<Document>(coll)
}

async fn sample_schema(
    client: &Client,
    db: &str,
    coll: &str,
    target: &str,
    args: &Value,
) -> Result<Done, String> {
    let size = int_arg(args, "size", 200, 500);
    let collection = collection(client, db, coll);
    let docs: Vec<Document> = collection
        .aggregate(vec![doc! { "$sample": { "size": size } }])
        .max_time(MAX_TIME)
        .await
        .map_err(|e| e.to_string())?
        .try_collect()
        .await
        .map_err(|e| e.to_string())?;
    // Views have no cheap count; their total is just left out.
    let total = collection
        .estimated_document_count()
        .max_time(MAX_TIME)
        .await
        .ok();
    let fields = schema::infer(&docs);
    let of_total = match total {
        Some(total) => format!("{} of {}", docs.len(), thousands(total)),
        None => docs.len().to_string(),
    };
    let fields_count = plural(fields.len() as u64, "field", "fields");
    let text = if docs.is_empty() {
        format!("Sampled {of_total} documents in {target}: it is empty.")
    } else {
        format!(
            "Sampled {of_total} documents in {target}.\n{}",
            schema::render(&fields, docs.len())
        )
    };
    Ok(Done {
        text,
        label: None,
        meta: format!("{of_total} documents · {fields_count}"),
    })
}

async fn explain(
    client: &Client,
    db: &str,
    coll: &str,
    target: &str,
    args: &Value,
) -> Result<Done, String> {
    let max_time_ms = MAX_TIME.as_millis() as i64;
    let inner_command = match opt_json_arg(args, "pipeline")? {
        Some(pipeline) => {
            let stages = pipeline_arg(pipeline)?;
            doc! { "aggregate": coll, "pipeline": stages, "cursor": {}, "maxTimeMS": max_time_ms }
        }
        None => {
            let mut command = doc! { "find": coll, "filter": doc_arg(args, "filter")? };
            if let Some(sort) = opt_doc_arg(args, "sort")? {
                command.insert("sort", sort);
            }
            command.insert("maxTimeMS", max_time_ms);
            command
        }
    };
    let result = client
        .database(db)
        .run_command(doc! { "explain": inner_command, "verbosity": "executionStats" })
        .await
        .map_err(|e| e.to_string())?;
    let summary = summarize_explain(&document_to_json(result));
    Ok(Done {
        text: format!("Explained the query on {target}.\n{}", summary.text),
        label: None,
        meta: summary.meta,
    })
}

async fn aggregate(
    client: &Client,
    db: &str,
    coll: &str,
    target: &str,
    args: &Value,
    limit: i64,
    include_documents: bool,
) -> Result<Done, String> {
    let pipeline = opt_json_arg(args, "pipeline")?
        .ok_or_else(|| "The pipeline argument is required.".to_string())?;
    let mut stages = pipeline_arg(pipeline)?;
    stages.push(doc! { "$limit": AGGREGATE_CAP + 1 });
    let started = Instant::now();
    let docs: Vec<Document> = collection(client, db, coll)
        .aggregate(stages)
        .max_time(MAX_TIME)
        .await
        .map_err(|e| e.to_string())?
        .try_collect()
        .await
        .map_err(|e| e.to_string())?;
    let ms = started.elapsed().as_millis();
    let count = if docs.len() as i64 > AGGREGATE_CAP {
        format!("{}+ documents", thousands(AGGREGATE_CAP as u64))
    } else {
        plural(docs.len() as u64, "document", "documents")
    };
    let head: Vec<&Document> = docs.iter().take(20).collect();
    let fields = schema::infer(head.iter().copied());
    let mut text = format!("The pipeline returned {count} on {target} in {ms} ms.");
    if !head.is_empty() {
        text.push_str(&format!(
            "\nOutput fields (from the first {}):\n{}",
            plural(head.len() as u64, "document", "documents"),
            schema::render(&fields, head.len())
        ));
    }
    if include_documents && !docs.is_empty() {
        let shown: Vec<Document> = docs.into_iter().take(limit as usize).collect();
        text.push_str(&format!(
            "\nFirst {} (relaxed Extended JSON, one per line):\n{}",
            plural(shown.len() as u64, "document", "documents"),
            json_lines(shown)
        ));
    }
    Ok(Done {
        text,
        label: None,
        meta: format!("{count} · {ms} ms"),
    })
}

/// Whether a pipeline writes: any stage is `$out` or `$merge`, including
/// inside `$facet`, `$lookup.pipeline` and `$unionWith.pipeline`.
pub(crate) fn pipeline_writes(stages: &[Value]) -> bool {
    stages.iter().any(|stage| {
        let Some(stage) = stage.as_object() else {
            return false;
        };
        stage.iter().any(|(name, spec)| match name.as_str() {
            "$out" | "$merge" => true,
            "$facet" => spec.as_object().is_some_and(|facets| {
                facets
                    .values()
                    .any(|p| p.as_array().is_some_and(|p| pipeline_writes(p)))
            }),
            "$lookup" | "$unionWith" => spec
                .get("pipeline")
                .and_then(Value::as_array)
                .is_some_and(|p| pipeline_writes(p)),
            _ => false,
        })
    })
}

/// A pipeline argument as BSON stages, refused if it writes.
fn pipeline_arg(value: Value) -> Result<Vec<Document>, String> {
    let Value::Array(stages) = value else {
        return Err("The pipeline must be a JSON array of stages.".to_string());
    };
    if pipeline_writes(&stages) {
        return Err(REFUSED_WRITE.to_string());
    }
    stages
        .into_iter()
        .map(|stage| json_to_document(stage).map_err(|e| e.to_string()))
        .collect()
}

pub(crate) struct ExplainSummary {
    pub text: String,
    pub meta: String,
}

/// Stages that read the data; the one found in the winning plan names how.
const ACCESS_STAGES: [&str; 11] = [
    "IXSCAN",
    "COLLSCAN",
    "IDHACK",
    "EXPRESS_IXSCAN",
    "EXPRESS_CLUSTERED_IXSCAN",
    "CLUSTERED_IXSCAN",
    "COUNT_SCAN",
    "DISTINCT_SCAN",
    "TEXT_MATCH",
    "GEO_NEAR_2DSPHERE",
    "EOF",
];

/// The parts of an `explain` (executionStats) answer that matter for a
/// query's performance, for find- and aggregate-shaped explains alike.
pub(crate) fn summarize_explain(explain: &Value) -> ExplainSummary {
    let planner = find_key(explain, "queryPlanner");
    let winning = planner.and_then(|p| p.get("winningPlan"));
    let plan = winning.map(|w| w.get("queryPlan").unwrap_or(w));
    let mut stages = Vec::new();
    let mut indexes = Vec::new();
    if let Some(plan) = plan {
        walk_plan(plan, &mut stages, &mut indexes);
    }
    let access = stages
        .iter()
        .find(|s| ACCESS_STAGES.contains(&s.as_str()))
        .or(stages.last())
        .cloned();

    let stats = find_key(explain, "executionStats");
    let num = |key: &str| stats.and_then(|s| s.get(key)).and_then(Value::as_i64);
    let (keys, docs, returned, millis) = (
        num("totalKeysExamined"),
        num("totalDocsExamined"),
        num("nReturned"),
        num("executionTimeMillis"),
    );

    let mut lines = Vec::new();
    if stages.is_empty() {
        lines.push("Winning plan: not reported.".to_string());
    } else {
        let index_detail: Vec<String> = indexes
            .iter()
            .map(|(name, key)| match key {
                Some(key) => format!("{name} {key}"),
                None => name.clone(),
            })
            .collect();
        let mut line = format!("Winning plan: {}", stages.join(" > "));
        if !index_detail.is_empty() {
            line.push_str(&format!(" (index {})", index_detail.join(", ")));
        }
        lines.push(line);
    }
    if let Some(pipeline) = explain.get("stages").and_then(Value::as_array) {
        let names: Vec<&str> = pipeline
            .iter()
            .filter_map(|s| s.as_object()?.keys().next().map(String::as_str))
            .collect();
        lines.push(format!("Pipeline stages: {}", names.join(", ")));
    }
    let show = |v: Option<i64>| v.map_or("?".to_string(), |n| thousands(n.max(0) as u64));
    if stats.is_some() {
        lines.push(format!(
            "Keys examined: {}, documents examined: {}, returned: {}, time: {} ms.",
            show(keys),
            show(docs),
            show(returned),
            show(millis)
        ));
    }

    let mut meta = access.unwrap_or_else(|| "plan".to_string());
    if let Some((name, _)) = indexes.first() {
        meta.push_str(&format!(" {name}"));
    }
    if let Some(docs) = docs {
        meta.push_str(&format!(" · {} examined", thousands(docs.max(0) as u64)));
    }
    if let Some(ms) = millis {
        meta.push_str(&format!(" · {ms} ms"));
    }
    ExplainSummary {
        text: lines.join("\n"),
        meta,
    }
}

type IndexUse = (String, Option<String>);

fn walk_plan(node: &Value, stages: &mut Vec<String>, indexes: &mut Vec<IndexUse>) {
    if let Some(stage) = node.get("stage").and_then(Value::as_str) {
        stages.push(stage.to_string());
    }
    if let Some(name) = node.get("indexName").and_then(Value::as_str) {
        if !indexes.iter().any(|(n, _)| n == name) {
            let key = node.get("keyPattern").map(|k| k.to_string());
            indexes.push((name.to_string(), key));
        }
    }
    if let Some(child) = node.get("inputStage") {
        walk_plan(child, stages, indexes);
    }
    if let Some(children) = node.get("inputStages").and_then(Value::as_array) {
        for child in children {
            walk_plan(child, stages, indexes);
        }
    }
}

/// The first value under `key`, searching depth-first.
fn find_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map
            .get(key)
            .or_else(|| map.values().find_map(|v| find_key(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_key(v, key)),
        _ => None,
    }
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// An integer argument clamped to `1..=max`.
fn int_arg(args: &Value, key: &str, default: i64, max: i64) -> i64 {
    args.get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(default)
        .clamp(1, max)
}

/// A JSON argument; agents sometimes send objects as JSON-encoded strings,
/// so those are decoded.
fn opt_json_arg(args: &Value, key: &str) -> Result<Option<Value>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => serde_json::from_str(s)
            .map(Some)
            .map_err(|e| format!("{key} is not valid JSON: {e}")),
        Some(other) => Ok(Some(other.clone())),
    }
}

fn opt_doc_arg(args: &Value, key: &str) -> Result<Option<Document>, String> {
    match opt_json_arg(args, key)? {
        None => Ok(None),
        Some(value @ Value::Object(_)) => json_to_document(value)
            .map(Some)
            .map_err(|e| format!("{key}: {e}")),
        Some(_) => Err(format!("{key} must be a JSON object.")),
    }
}

/// A document argument that defaults to `{}`.
fn doc_arg(args: &Value, key: &str) -> Result<Document, String> {
    Ok(opt_doc_arg(args, key)?.unwrap_or_default())
}

fn json_lines(docs: Vec<Document>) -> String {
    docs.into_iter()
        .map(|d| document_to_json(d).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `1234567` -> `1,234,567`.
pub(crate) fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", thousands(n))
    }
}

/// Cuts `text` to at most `max` bytes on a character boundary, saying so.
pub(crate) fn truncate(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let marker = "… (truncated)";
    let mut end = max.saturating_sub(marker.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{marker}", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{inner_with, session, RecordingHost};
    use super::super::AssistantPolicy;
    use super::*;

    #[test]
    fn detects_writes_at_any_depth() {
        assert!(pipeline_writes(&[
            json!({ "$match": {} }),
            json!({ "$out": "x" })
        ]));
        assert!(pipeline_writes(&[json!({ "$merge": { "into": "x" } })]));
        assert!(pipeline_writes(&[json!({ "$facet": {
            "a": [{ "$match": {} }],
            "b": [{ "$out": "x" }]
        } })]));
        assert!(pipeline_writes(&[json!({ "$lookup": {
            "from": "y", "as": "z",
            "pipeline": [{ "$facet": { "c": [{ "$merge": "x" }] } }]
        } })]));
        assert!(pipeline_writes(&[json!({ "$unionWith": {
            "coll": "y", "pipeline": [{ "$out": "x" }]
        } })]));

        assert!(!pipeline_writes(&[
            json!({ "$match": { "$out": 1 } }),
            json!({ "$lookup": { "from": "y", "localField": "a", "foreignField": "b", "as": "c" } }),
            json!({ "$unionWith": "y" }),
            json!({ "$project": { "out": "$merge" } }),
        ]));
        assert_eq!(
            pipeline_arg(json!([{ "$facet": { "b": [{ "$out": "x" }] } }])).unwrap_err(),
            REFUSED_WRITE
        );
    }

    #[test]
    fn summarizes_a_find_explain() {
        let explain = json!({
            "queryPlanner": { "winningPlan": {
                "stage": "FETCH",
                "inputStage": { "stage": "IXSCAN", "indexName": "status_1", "keyPattern": { "status": 1 } }
            } },
            "executionStats": { "nReturned": 71, "executionTimeMillis": 3, "totalKeysExamined": 71, "totalDocsExamined": 71 }
        });
        let summary = summarize_explain(&explain);
        assert_eq!(summary.meta, "IXSCAN status_1 · 71 examined · 3 ms");
        assert!(summary
            .text
            .contains("Winning plan: FETCH > IXSCAN (index status_1 {\"status\":1})"));
    }

    #[test]
    fn summarizes_an_sbe_aggregate_explain() {
        let explain = json!({
            "stages": [
                { "$cursor": {
                    "queryPlanner": { "winningPlan": { "queryPlan": { "stage": "COLLSCAN" } } },
                    "executionStats": { "nReturned": 250, "executionTimeMillis": 4, "totalKeysExamined": 0, "totalDocsExamined": 250 }
                } },
                { "$group": {} }
            ]
        });
        let summary = summarize_explain(&explain);
        assert_eq!(summary.meta, "COLLSCAN · 250 examined · 4 ms");
        assert!(summary.text.contains("Pipeline stages: $cursor, $group"));
    }

    #[test]
    fn formats_numbers_and_cuts_output() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(plural(1, "index", "indexes"), "1 index");
        assert_eq!(plural(1500, "document", "documents"), "1,500 documents");
        let cut = truncate("é".repeat(100), 51);
        assert!(cut.len() <= 51 && cut.ends_with("(truncated)"));
        assert_eq!(truncate("short".into(), 51), "short");
    }

    #[test]
    fn arguments_accept_json_strings_and_clamp() {
        let args = json!({ "filter": "{\"a\": 1}", "limit": 500, "sort": 3 });
        assert_eq!(doc_arg(&args, "filter").unwrap(), doc! { "a": 1 });
        assert_eq!(doc_arg(&args, "missing").unwrap(), Document::new());
        assert!(opt_doc_arg(&args, "sort").is_err());
        assert_eq!(int_arg(&args, "limit", 20, 50), 50);
        assert_eq!(int_arg(&args, "other", 20, 50), 20);
    }

    fn step_states(host: &RecordingHost) -> Vec<(String, String, String)> {
        host.events_named(STEP_EVENT)
            .iter()
            .map(|e| {
                (
                    e["state"].as_str().unwrap().to_string(),
                    e["label"].as_str().unwrap().to_string(),
                    e["meta"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn policy_refusals_happen_before_touching_the_database() {
        let host = Arc::new(RecordingHost::default());
        let session = session("shop", "c1");
        let inner = inner_with(
            host.clone(),
            &session,
            AssistantPolicy {
                allowed_connections: vec!["c1".into()],
                ..Default::default()
            },
        );
        let out = call(
            &inner,
            &session,
            "list_indexes",
            json!({ "collection": "orders" }),
        )
        .await
        .unwrap();
        assert!(out.is_error);
        assert_eq!(out.text, REFUSED_INDEXES);
        assert_eq!(
            step_states(&host),
            [(
                "denied".to_string(),
                "Reading indexes on shop.orders".to_string(),
                "not shared with the Assistant".to_string()
            )]
        );

        inner.policy.lock().unwrap().allowed_connections.clear();
        let out = call(&inner, &session, "count", json!({ "collection": "orders" }))
            .await
            .unwrap();
        assert_eq!(out.text, REFUSED_CONNECTION);
        assert!(call(&inner, &session, "drop", json!({})).await.is_none());
    }

    #[tokio::test]
    async fn a_denied_value_request_never_reads() {
        let host = Arc::new(RecordingHost::default());
        let session = session("shop", "c1");
        let inner = inner_with(
            host.clone(),
            &session,
            AssistantPolicy {
                allowed_connections: vec!["c1".into()],
                ..Default::default()
            },
        );
        let task = {
            let (inner, session) = (inner.clone(), session.clone());
            tokio::spawn(async move {
                call(
                    &inner,
                    &session,
                    "find",
                    json!({ "collection": "orders", "limit": 5 }),
                )
                .await
                .unwrap()
            })
        };
        let request = loop {
            if let Some(e) = host.events_named(APPROVAL_EVENT).pop() {
                break e;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(request["tool"], "find");
        assert_eq!(request["collection"], "orders");
        assert_eq!(request["limit"], 5);
        assert_eq!(request["provider"], "Anthropic");
        super::super::Assistant {
            inner: inner.clone(),
        }
        .answer(request["requestId"].as_str().unwrap(), ApprovalChoice::Deny)
        .unwrap();
        let out = task.await.unwrap();
        assert!(out.is_error);
        assert_eq!(out.text, DENIED_VALUES);
        let states = step_states(&host);
        assert_eq!(states[0].0, "run");
        assert_eq!(
            states[1],
            (
                "denied".to_string(),
                "Reading document values".to_string(),
                "denied by you".to_string()
            )
        );
    }
}

#[cfg(test)]
mod live_tests {
    //! Against a throwaway database (`mongo_studio_test`), the same fixed
    //! name driver.rs's and scripting.rs's self-seeding live tests use.
    //! Each test seeds its own `orders_<uuid>` collection, so nothing here
    //! depends on dev data. The collection is dropped at the end of a passing
    //! run; a failing one leaves it behind, under a name no other run reuses.
    //! Run with:
    //!   MONGO_STUDIO_TEST_URI=mongodb://localhost:27017 \
    //!     cargo test --lib -- --ignored assistant::tools::live_tests
    use super::super::test_support::{inner_with, session, RecordingHost};
    use super::super::AssistantPolicy;
    use super::*;

    const TEST_DB: &str = "mongo_studio_test";

    async fn setup(policy: AssistantPolicy) -> (Arc<RecordingHost>, Arc<AgentSession>, Arc<Inner>) {
        let uri = std::env::var("MONGO_STUDIO_TEST_URI")
            .expect("set MONGO_STUDIO_TEST_URI to run this test");
        let client = Client::with_uri_str(&uri).await.unwrap();
        let host = Arc::new(RecordingHost {
            client: Some(client),
            ..Default::default()
        });
        let session = session(TEST_DB, "c1");
        let inner = inner_with(host.clone(), &session, policy);
        (host, session, inner)
    }

    fn open_policy() -> AssistantPolicy {
        AssistantPolicy {
            indexes: true,
            values: true,
            values_connections: vec![],
            allowed_connections: vec!["c1".into()],
        }
    }

    /// A fresh collection name per test run, so two runs (or the two tests
    /// in this module) never collide or depend on each other's leftovers.
    fn fresh_collection() -> String {
        format!("orders_{}", Uuid::new_v4().simple())
    }

    /// Enough orders to exercise every assertion below: more than one
    /// document (plural wording, and `find`'s `limit: 2`), a `status` of
    /// "shipped" (the `explain` filter), and more than one distinct status
    /// (the `$group` pipelines). `_id` is left to the server, which always
    /// assigns an ObjectId.
    async fn seed_orders(client: &Client, coll: &str) {
        collection(client, TEST_DB, coll)
            .insert_many(vec![
                doc! { "status": "shipped", "total": 42 },
                doc! { "status": "shipped", "total": 17 },
                doc! { "status": "pending", "total": 5 },
                doc! { "status": "cancelled", "total": 3 },
            ])
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn live_schema_and_counts() {
        let (host, session, inner) = setup(open_policy()).await;
        let client = host.client.clone().unwrap();
        let coll = fresh_collection();
        seed_orders(&client, &coll).await;
        let target = format!("{TEST_DB}.{coll}");

        let out = call(&inner, &session, "list_collections", json!({}))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains(&format!("{coll} (collection)")),
            "{}",
            out.text
        );

        let out = call(
            &inner,
            &session,
            "sample_schema",
            json!({ "collection": coll, "size": 50 }),
        )
        .await
        .unwrap();
        println!("{}", out.text);
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Sampled "));
        assert!(out.text.contains("\n_id: objectId (100%)"));

        let out = call(
            &inner,
            &session,
            "count",
            json!({ "collection": coll, "filter": {} }),
        )
        .await
        .unwrap();
        assert!(
            out.text.contains(&format!("documents in {target} match")),
            "{}",
            out.text
        );

        let steps = host.events_named(STEP_EVENT);
        let sampled = steps
            .iter()
            .find(|s| s["tool"] == "sample_schema" && s["state"] == "ok")
            .unwrap();
        assert_eq!(sampled["label"], format!("Sampled {target}"));
        println!("sample meta: {}", sampled["meta"]);
        assert!(sampled["meta"].as_str().unwrap().contains(" documents · "));

        collection(&client, TEST_DB, &coll).drop().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn live_indexes_explain_find_aggregate() {
        let (host, session, inner) = setup(open_policy()).await;
        let client = host.client.clone().unwrap();
        let coll = fresh_collection();
        seed_orders(&client, &coll).await;
        let target = format!("{TEST_DB}.{coll}");

        let out = call(
            &inner,
            &session,
            "list_indexes",
            json!({ "collection": coll }),
        )
        .await
        .unwrap();
        println!("{}", out.text);
        assert!(out.text.contains("_id_ {\"_id\":1}"), "{}", out.text);

        let out = call(
            &inner,
            &session,
            "explain",
            json!({ "collection": coll, "filter": { "status": "shipped" }, "sort": { "_id": -1 } }),
        )
        .await
        .unwrap();
        println!("{}", out.text);
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("Winning plan: "));

        let out = call(
            &inner,
            &session,
            "explain",
            json!({ "collection": coll, "pipeline": [{ "$group": { "_id": "$status", "n": { "$sum": 1 } } }] }),
        )
        .await
        .unwrap();
        println!("{}", out.text);
        assert!(!out.is_error, "{}", out.text);

        let out = call(
            &inner,
            &session,
            "find",
            json!({ "collection": coll, "limit": 2, "projection": { "_id": 1 } }),
        )
        .await
        .unwrap();
        assert!(
            out.text
                .starts_with(&format!("Read 2 documents from {target}")),
            "{}",
            out.text
        );
        assert!(out.text.contains("{\"_id\":{\"$oid\":"), "{}", out.text);

        let out = call(
            &inner,
            &session,
            "aggregate",
            json!({ "collection": coll, "pipeline": [{ "$group": { "_id": "$status", "n": { "$sum": 1 } } }] }),
        )
        .await
        .unwrap();
        println!("{}", out.text);
        assert!(
            out.text.starts_with("The pipeline returned "),
            "{}",
            out.text
        );
        assert!(out.text.contains("\nn: int (100%)"), "{}", out.text);
        assert!(
            !out.text.contains("First "),
            "no values without include_documents"
        );

        let out = call(
            &inner,
            &session,
            "aggregate",
            json!({ "collection": coll, "pipeline": [{ "$out": "copy" }] }),
        )
        .await
        .unwrap();
        assert!(out.is_error);
        assert_eq!(out.text, REFUSED_WRITE);

        let find_ok = host
            .events_named(STEP_EVENT)
            .into_iter()
            .find(|s| s["tool"] == "find" && s["state"] == "ok")
            .unwrap();
        assert_eq!(find_ok["meta"], "document values on");
        assert_eq!(find_ok["label"], format!("Read 2 documents from {target}"));

        collection(&client, TEST_DB, &coll).drop().await.unwrap();
    }
}

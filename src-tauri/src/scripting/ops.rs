//! What the script engine's `__native(op, db, coll, args)` does. Arguments
//! and results are canonical Extended JSON, so BSON types survive both
//! ways. Options are checked by name: one the engine doesn't know is an
//! error instead of being dropped without a word.
//!
//! A read-only run refuses every write; a dry run doesn't make them but
//! counts what they would touch and notes it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Duration;

use futures_util::TryStreamExt;
use mongodb::bson::{self, doc, oid::ObjectId, Bson, Document};
use mongodb::options::{
    AggregateOptions, CountOptions, DeleteOptions, DistinctOptions, EstimatedDocumentCountOptions,
    FindOneAndDeleteOptions, FindOneAndReplaceOptions, FindOneAndUpdateOptions, FindOptions,
    InsertManyOptions, InsertOneOptions, ReplaceOptions, UpdateModifications, UpdateOptions,
};
use mongodb::{Client, Collection, Cursor};
use serde::de::DeserializeOwned;
use serde_json::{json, Value as JsonValue};

use super::{ScriptMode, WriteTally};
use crate::bulk::{self, WriteOp};
use crate::ejson::json_to_document;
use crate::error::{AppError, AppResult};

/// Documents handed to the script per round trip of a cursor.
const BATCH: usize = 500;
/// `sleep` is capped here.
const MAX_SLEEP: Duration = Duration::from_secs(10 * 60);

/// Commands a read-only or dry run may still send with runCommand: only
/// ones that read data or describe it. `profile` (it sets the profiler),
/// `validate` (it can repair) and the server's logs and operations stay out.
const READ_COMMANDS: &[&str] = &[
    "ping",
    "buildInfo",
    "buildinfo",
    "dbStats",
    "collStats",
    "dataSize",
    "listCollections",
    "listIndexes",
    "listDatabases",
    "count",
    "distinct",
    "find",
    "aggregate",
    "explain",
    "hello",
    "isMaster",
    "ismaster",
    "connectionStatus",
];

const FIND: &[&str] = &[
    "projection",
    "sort",
    "limit",
    "skip",
    "hint",
    "maxTimeMS",
    "allowDiskUse",
    "collation",
    "batchSize",
    "comment",
    "min",
    "max",
    "returnKey",
    "showRecordId",
    "noCursorTimeout",
    "let",
];
const AGGREGATE: &[&str] = &[
    "allowDiskUse",
    "maxTimeMS",
    "hint",
    "collation",
    "batchSize",
    "comment",
    "let",
    "bypassDocumentValidation",
];
const COUNT: &[&str] = &["limit", "skip", "hint", "maxTimeMS", "collation", "comment"];
const ESTIMATED: &[&str] = &["maxTimeMS", "comment"];
const DISTINCT: &[&str] = &["maxTimeMS", "collation", "comment", "hint"];
const INSERT_ONE: &[&str] = &["bypassDocumentValidation", "comment"];
const INSERT_MANY: &[&str] = &["ordered", "bypassDocumentValidation", "comment"];
const UPDATE: &[&str] = &[
    "upsert",
    "arrayFilters",
    "hint",
    "collation",
    "bypassDocumentValidation",
    "let",
    "comment",
];
const REPLACE: &[&str] = &[
    "upsert",
    "hint",
    "collation",
    "bypassDocumentValidation",
    "let",
    "comment",
];
const DELETE: &[&str] = &["hint", "collation", "let", "comment"];
const FIND_AND_UPDATE: &[&str] = &[
    "projection",
    "sort",
    "upsert",
    "returnDocument",
    "returnNewDocument",
    "arrayFilters",
    "hint",
    "collation",
    "maxTimeMS",
    "bypassDocumentValidation",
    "let",
    "comment",
];
const FIND_AND_REPLACE: &[&str] = &[
    "projection",
    "sort",
    "upsert",
    "returnDocument",
    "returnNewDocument",
    "hint",
    "collation",
    "maxTimeMS",
    "bypassDocumentValidation",
    "let",
    "comment",
];
const FIND_AND_DELETE: &[&str] = &[
    "projection",
    "sort",
    "hint",
    "collation",
    "maxTimeMS",
    "let",
    "comment",
];
const BULK: &[&str] = &["ordered", "bypassDocumentValidation"];

/// The state a script's natives share.
pub(super) struct Engine {
    pub client: Client,
    pub mode: ScriptMode,
    cursors: RefCell<HashMap<u32, Cursor<Document>>>,
    next_cursor: Cell<u32>,
    pub tally: RefCell<WriteTally>,
    /// What a dry run would have done, one line per write.
    pub planned: RefCell<Vec<String>>,
}

impl Engine {
    pub fn new(client: Client, mode: ScriptMode) -> Self {
        Self {
            client,
            mode,
            cursors: RefCell::new(HashMap::new()),
            next_cursor: Cell::new(1),
            tally: RefCell::new(WriteTally::default()),
            planned: RefCell::new(Vec::new()),
        }
    }

    fn plan(&self, line: String) {
        self.planned.borrow_mut().push(line);
    }

    async fn keep_cursor(&self, mut cursor: Cursor<Document>, size: usize) -> AppResult<JsonValue> {
        let (docs, done) = take_batch(&mut cursor, size).await?;
        let id = if done {
            0
        } else {
            let id = self.next_cursor.get();
            self.next_cursor.set(id + 1);
            self.cursors.borrow_mut().insert(id, cursor);
            id
        };
        Ok(json!({ "id": id, "docs": docs, "done": done }))
    }
}

async fn take_batch(
    cursor: &mut Cursor<Document>,
    size: usize,
) -> AppResult<(Vec<JsonValue>, bool)> {
    let mut docs = Vec::new();
    while docs.len() < size {
        match cursor.try_next().await? {
            Some(doc) => docs.push(canon_doc(doc)),
            None => return Ok((docs, true)),
        }
    }
    Ok((docs, false))
}

fn canon(value: Bson) -> JsonValue {
    value.into_canonical_extjson()
}

fn canon_doc(doc: Document) -> JsonValue {
    canon(Bson::Document(doc))
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::InvalidInput(message.into())
}

fn arg(args: &[JsonValue], i: usize) -> Option<&JsonValue> {
    args.get(i).filter(|v| !v.is_null())
}

/// A document argument; absent is `{}`.
fn doc_arg(args: &[JsonValue], i: usize, op: &str, what: &str) -> AppResult<Document> {
    match arg(args, i) {
        None => Ok(Document::new()),
        Some(value @ JsonValue::Object(_)) => json_to_document(value.clone()),
        Some(_) => Err(invalid(format!("{op}: {what} must be a document"))),
    }
}

fn required_doc(args: &[JsonValue], i: usize, op: &str, what: &str) -> AppResult<Document> {
    if arg(args, i).is_none() {
        return Err(invalid(format!("{op} needs {what}")));
    }
    doc_arg(args, i, op, what)
}

fn pipeline_arg(args: &[JsonValue], i: usize, op: &str) -> AppResult<Vec<Document>> {
    match arg(args, i) {
        None => Ok(Vec::new()),
        Some(JsonValue::Array(stages)) => stages
            .iter()
            .map(|s| match s {
                JsonValue::Object(_) => json_to_document(s.clone()),
                _ => Err(invalid(format!(
                    "{op}: every pipeline stage must be a document"
                ))),
            })
            .collect(),
        Some(_) => Err(invalid(format!(
            "{op}: the pipeline must be an array of stages"
        ))),
    }
}

/// An update: operators, or a pipeline (an array).
fn update_arg(args: &[JsonValue], i: usize, op: &str) -> AppResult<UpdateModifications> {
    match arg(args, i) {
        Some(JsonValue::Array(_)) => Ok(UpdateModifications::Pipeline(pipeline_arg(args, i, op)?)),
        Some(JsonValue::Object(_)) => {
            let update = json_to_document(args[i].clone())?;
            if update.keys().next().is_some_and(|k| !k.starts_with('$')) {
                return Err(invalid(format!(
                    "{op} takes update operators such as {{ $set: {{...}} }}; to replace the whole document use replaceOne"
                )));
            }
            Ok(UpdateModifications::Document(update))
        }
        _ => Err(invalid(format!(
            "{op} needs an update document or pipeline"
        ))),
    }
}

fn update_bson(update: &UpdateModifications) -> Bson {
    match update {
        UpdateModifications::Document(d) => Bson::Document(d.clone()),
        UpdateModifications::Pipeline(p) => {
            Bson::Array(p.iter().cloned().map(Bson::Document).collect())
        }
        _ => Bson::Null,
    }
}

fn replacement_arg(args: &[JsonValue], i: usize, op: &str) -> AppResult<Document> {
    let doc = required_doc(args, i, op, "a replacement document")?;
    if doc.keys().any(|k| k.starts_with('$')) {
        return Err(invalid(format!(
            "{op} takes a whole document, not update operators; use updateOne for $set and friends"
        )));
    }
    Ok(doc)
}

/// The options object at `args[i]` as a document, after checking every
/// name against `allowed`.
fn raw_options(args: &[JsonValue], i: usize, op: &str, allowed: &[&str]) -> AppResult<Document> {
    let Some(value) = arg(args, i) else {
        return Ok(Document::new());
    };
    let JsonValue::Object(map) = value else {
        return Err(invalid(format!("{op}: the options must be an object")));
    };
    if let Some(unknown) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(invalid(format!(
            "{op} doesn't support the option \"{unknown}\" here, and would have ignored it. Supported: {}.",
            allowed.join(", ")
        )));
    }
    json_to_document(value.clone())
}

/// The options at `args[i]` read into the driver's options, with
/// `maxTimeMS` apart (the driver doesn't read it from a document) and the
/// checked document itself.
fn options<T: DeserializeOwned + Default>(
    args: &[JsonValue],
    i: usize,
    op: &str,
    allowed: &[&str],
) -> AppResult<(T, Option<Duration>, Document)> {
    let raw = raw_options(args, i, op, allowed)?;
    let mut parse = raw.clone();
    let max_time = match parse.remove("maxTimeMS") {
        None | Some(Bson::Null) => None,
        Some(v) => Some(Duration::from_millis(
            bson_number(&v).filter(|ms| *ms >= 0.0).ok_or_else(|| {
                invalid(format!("{op}: maxTimeMS must be a number of milliseconds"))
            })? as u64,
        )),
    };
    // mongosh's returnNewDocument; the driver reads returnDocument.
    if let Some(new) = parse.remove("returnNewDocument") {
        if new.as_bool() == Some(true) && !parse.contains_key("returnDocument") {
            parse.insert("returnDocument", "after");
        }
    }
    let parsed = bson::deserialize_from_document::<T>(parse)
        .map_err(|e| invalid(format!("{op} options: {e}")))?;
    Ok((parsed, max_time, raw))
}

fn bson_number(value: &Bson) -> Option<f64> {
    match value {
        Bson::Int32(n) => Some(*n as f64),
        Bson::Int64(n) => Some(*n as f64),
        Bson::Double(n) => Some(*n),
        _ => None,
    }
}

fn is_write_pipeline(pipeline: &[Document]) -> bool {
    pipeline
        .last()
        .is_some_and(|stage| stage.contains_key("$out") || stage.contains_key("$merge"))
}

fn collection(engine: &Engine, db: &str, coll: &str) -> AppResult<Collection<Document>> {
    if coll.is_empty() {
        return Err(invalid("a collection name is required"));
    }
    Ok(engine.client.database(db).collection::<Document>(coll))
}

/// What the run's mode does with a write: refuse it (read-only), skip it
/// (dry run, returning what to hand the script instead), or let it run.
enum Gate {
    Run,
    Simulated(JsonValue),
}

async fn gate(
    engine: &Engine,
    op: &str,
    db: &str,
    coll: &str,
    args: &[JsonValue],
) -> AppResult<Gate> {
    match engine.mode {
        ScriptMode::Normal => Ok(Gate::Run),
        ScriptMode::ReadOnly => Err(invalid(format!(
            "This script runs read-only: {op} on {} would change data, so it was refused and the script stopped. Nothing was written by it.",
            ns(db, coll)
        ))),
        ScriptMode::DryRun => simulate(engine, op, db, coll, args).await.map(Gate::Simulated),
    }
}

fn ns(db: &str, coll: &str) -> String {
    if coll.is_empty() {
        db.to_string()
    } else {
        format!("{db}.{coll}")
    }
}

fn docs_word(n: u64) -> String {
    format!("{n} document{}", if n == 1 { "" } else { "s" })
}

/// The dry run of a write: count what it would touch, note it, and make up
/// the result the script expects.
async fn simulate(
    engine: &Engine,
    op: &str,
    db: &str,
    coll: &str,
    args: &[JsonValue],
) -> AppResult<JsonValue> {
    let target = ns(db, coll);
    let count = |filter: Document, one: bool, collation: Option<Bson>| async move {
        let c = collection(engine, db, coll)?;
        let mut options = CountOptions::default();
        if one {
            options.limit = Some(1);
        }
        if let Some(Bson::Document(collation)) = collation {
            options.collation = bson::deserialize_from_document(collation).ok();
        }
        AppResult::Ok(c.count_documents(filter).with_options(options).await?)
    };
    match op {
        "insertOne" => {
            let mut doc = required_doc(args, 0, op, "a document")?;
            let id = ensure_id(&mut doc);
            engine.plan(format!(
                "insertOne into {target}: 1 document would be inserted"
            ));
            Ok(json!({ "acknowledged": true, "insertedId": canon(id), "dryRun": true }))
        }
        "insertMany" => {
            let docs = docs_arg(args, 0, op)?;
            let ids: Vec<JsonValue> = docs
                .into_iter()
                .map(|mut d| canon(ensure_id(&mut d)))
                .collect();
            engine.plan(format!(
                "insertMany into {target}: {} would be inserted",
                docs_word(ids.len() as u64)
            ));
            Ok(
                json!({ "acknowledged": true, "insertedCount": ids.len(), "insertedIds": ids, "dryRun": true }),
            )
        }
        "updateOne" | "updateMany" | "replaceOne" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            if op == "replaceOne" {
                replacement_arg(args, 1, op)?;
            } else {
                update_arg(args, 1, op)?;
            }
            let allowed = if op == "replaceOne" { REPLACE } else { UPDATE };
            let raw = raw_options(args, 2, op, allowed)?;
            let upsert = raw.get_bool("upsert").unwrap_or(false);
            let matched = count(filter, op != "updateMany", raw.get("collation").cloned()).await?;
            let line = match (matched, upsert) {
                (0, true) => format!(
                    "{op} on {target}: nothing matches; 1 document would be inserted (upsert)"
                ),
                (0, false) => format!("{op} on {target}: nothing matches; nothing would change"),
                (n, _) => format!("{op} on {target}: {} would change", docs_word(n)),
            };
            engine.plan(line);
            let upserted = u64::from(matched == 0 && upsert);
            Ok(
                json!({ "acknowledged": true, "matchedCount": matched, "modifiedCount": matched, "upsertedCount": upserted, "upsertedId": null, "dryRun": true }),
            )
        }
        "deleteOne" | "deleteMany" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let raw = raw_options(args, 1, op, DELETE)?;
            let n = count(filter, op == "deleteOne", raw.get("collation").cloned()).await?;
            engine.plan(format!(
                "{op} on {target}: {} would be deleted",
                docs_word(n)
            ));
            Ok(json!({ "acknowledged": true, "deletedCount": n, "dryRun": true }))
        }
        "findOneAndUpdate" | "findOneAndReplace" | "findOneAndDelete" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let options_at = if op == "findOneAndDelete" { 1 } else { 2 };
            let allowed = match op {
                "findOneAndUpdate" => FIND_AND_UPDATE,
                "findOneAndReplace" => FIND_AND_REPLACE,
                _ => FIND_AND_DELETE,
            };
            let raw = raw_options(args, options_at, op, allowed)?;
            let mut find = FindOptions::default();
            find.sort = raw.get_document("sort").ok().cloned();
            find.projection = raw.get_document("projection").ok().cloned();
            find.limit = Some(1);
            let c = collection(engine, db, coll)?;
            let current: Vec<Document> = c
                .find(filter)
                .with_options(find)
                .await?
                .try_collect()
                .await?;
            let found = current.into_iter().next();
            let verb = match op {
                "findOneAndDelete" => "would be deleted",
                "findOneAndReplace" => "would be replaced",
                _ => "would change",
            };
            engine.plan(match &found {
                Some(_) => format!("{op} on {target}: 1 document {verb}"),
                None => format!("{op} on {target}: nothing matches"),
            });
            Ok(found.map(canon_doc).unwrap_or(JsonValue::Null))
        }
        "bulkWrite" => {
            let ops = bulk_ops(args, op)?;
            let (mut inserts, mut changes, mut deletes) = (0u64, 0u64, 0u64);
            for write in ops {
                match write {
                    WriteOp::Insert(_) => inserts += 1,
                    WriteOp::Update { filter, multi, .. } => {
                        changes += count(filter, !multi, None).await?
                    }
                    WriteOp::Delete { filter, multi, .. } => {
                        deletes += count(filter, !multi, None).await?
                    }
                }
            }
            engine.plan(format!(
                "bulkWrite on {target}: {} would be inserted, {} would change, {} would be deleted",
                docs_word(inserts),
                docs_word(changes),
                docs_word(deletes)
            ));
            Ok(
                json!({ "acknowledged": true, "insertedCount": inserts, "matchedCount": changes, "modifiedCount": changes, "deletedCount": deletes, "upsertedCount": 0, "insertedIds": {}, "upsertedIds": {}, "dryRun": true }),
            )
        }
        "aggregate" => {
            engine.plan(format!(
                "aggregate on {target} would write its output ($out/$merge)"
            ));
            Ok(json!({ "id": 0, "docs": [], "done": true }))
        }
        "runCommand" => {
            let command = doc_arg(args, 0, op, "the command")?;
            let name = command.keys().next().cloned().unwrap_or_default();
            engine.plan(format!("the command {name} on {db} would run"));
            Ok(json!({ "ok": 1, "dryRun": true }))
        }
        _ => {
            engine.plan(format!("{op} on {target} would run"));
            Ok(match op {
                "createIndex" => JsonValue::String("(dry run)".to_string()),
                "drop" => JsonValue::Bool(true),
                _ => json!({ "ok": 1, "dryRun": true }),
            })
        }
    }
}

fn ensure_id(doc: &mut Document) -> Bson {
    if let Some(id) = doc.get("_id") {
        return id.clone();
    }
    let id = Bson::ObjectId(ObjectId::new());
    let mut with_id = doc! { "_id": id.clone() };
    with_id.extend(std::mem::take(doc));
    *doc = with_id;
    id
}

fn docs_arg(args: &[JsonValue], i: usize, op: &str) -> AppResult<Vec<Document>> {
    match arg(args, i) {
        Some(JsonValue::Array(items)) if !items.is_empty() => items
            .iter()
            .map(|d| match d {
                JsonValue::Object(_) => json_to_document(d.clone()),
                _ => Err(invalid(format!("{op}: every item must be a document"))),
            })
            .collect(),
        Some(JsonValue::Array(_)) => Err(invalid(format!("{op} needs at least one document"))),
        _ => Err(invalid(format!("{op} expects an array of documents"))),
    }
}

/// bulkWrite's operations, as mongosh and the Node driver write them.
fn bulk_ops(args: &[JsonValue], op: &str) -> AppResult<Vec<WriteOp>> {
    let Some(JsonValue::Array(items)) = arg(args, 0) else {
        return Err(invalid(format!("{op} expects an array of operations")));
    };
    let mut ops = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let JsonValue::Object(map) = item else {
            return Err(invalid(format!("{op}: operation #{i} must be an object")));
        };
        let (Some((kind, body)), 1) = (map.iter().next(), map.len()) else {
            return Err(invalid(format!(
                "{op}: operation #{i} must have exactly one of insertOne, updateOne, updateMany, replaceOne, deleteOne, deleteMany"
            )));
        };
        let JsonValue::Object(body) = body else {
            return Err(invalid(format!(
                "{op}: operation #{i} ({kind}) must hold an object"
            )));
        };
        let allowed: &[&str] = match kind.as_str() {
            "insertOne" => &["document"],
            "updateOne" | "updateMany" => &[
                "filter",
                "update",
                "upsert",
                "arrayFilters",
                "hint",
                "collation",
            ],
            "replaceOne" => &["filter", "replacement", "upsert", "hint", "collation"],
            "deleteOne" | "deleteMany" => &["filter", "hint", "collation"],
            other => {
                return Err(invalid(format!(
                    "{op}: operation #{i} has an unknown kind \"{other}\""
                )))
            }
        };
        if let Some(unknown) = body.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(invalid(format!(
                "{op}: {kind} (operation #{i}) doesn't support \"{unknown}\". Supported: {}.",
                allowed.join(", ")
            )));
        }
        let field = |name: &str| body.get(name).cloned().unwrap_or(JsonValue::Null);
        let doc_field = |name: &str| -> AppResult<Document> {
            match field(name) {
                JsonValue::Null => Ok(Document::new()),
                v @ JsonValue::Object(_) => json_to_document(v),
                _ => Err(invalid(format!(
                    "{op}: {kind}.{name} (operation #{i}) must be a document"
                ))),
            }
        };
        let mut extra = Document::new();
        for key in ["arrayFilters", "hint", "collation"] {
            if let Some(value) = body.get(key) {
                let wrapped = json_to_document(json!({ "v": value }))?;
                if let Some(v) = wrapped.get("v") {
                    extra.insert(key, v.clone());
                }
            }
        }
        let upsert = body
            .get("upsert")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        ops.push(match kind.as_str() {
            "insertOne" => {
                if !matches!(field("document"), JsonValue::Object(_)) {
                    return Err(invalid(format!(
                        "{op}: insertOne (operation #{i}) needs a document"
                    )));
                }
                WriteOp::Insert(doc_field("document")?)
            }
            "updateOne" | "updateMany" => {
                let update = update_arg(
                    &[field("update")],
                    0,
                    &format!("{op}: {kind} (operation #{i})"),
                )?;
                WriteOp::Update {
                    filter: doc_field("filter")?,
                    update: update_bson(&update),
                    multi: kind == "updateMany",
                    upsert,
                    extra,
                }
            }
            "replaceOne" => WriteOp::Update {
                filter: doc_field("filter")?,
                update: Bson::Document(replacement_arg(
                    &[field("replacement")],
                    0,
                    &format!("{op}: replaceOne (operation #{i})"),
                )?),
                multi: false,
                upsert,
                extra,
            },
            _ => WriteOp::Delete {
                filter: doc_field("filter")?,
                multi: kind == "deleteMany",
                extra,
            },
        });
    }
    Ok(ops)
}

fn is_write(op: &str, args: &[JsonValue]) -> bool {
    match op {
        "insertOne" | "insertMany" | "updateOne" | "updateMany" | "replaceOne" | "deleteOne"
        | "deleteMany" | "findOneAndUpdate" | "findOneAndReplace" | "findOneAndDelete"
        | "bulkWrite" | "createIndex" | "createIndexes" | "dropIndex" | "dropIndexes" | "drop"
        | "createCollection" => true,
        "aggregate" => pipeline_arg(args, 0, op).is_ok_and(|p| is_write_pipeline(&p)),
        "explain" => {
            args.first().and_then(JsonValue::as_str) == Some("aggregate")
                && pipeline_arg(args, 1, op).is_ok_and(|p| is_write_pipeline(&p))
        }
        "runCommand" => match arg(args, 0) {
            Some(JsonValue::Object(map)) => match map.keys().next() {
                Some(name) if READ_COMMANDS.contains(&name.as_str()) => {
                    // an aggregate command with $out/$merge writes
                    name == "aggregate"
                        && map
                            .get("pipeline")
                            .and_then(JsonValue::as_array)
                            .and_then(|p| p.last())
                            .and_then(JsonValue::as_object)
                            .is_some_and(|s| s.contains_key("$out") || s.contains_key("$merge"))
                }
                _ => true,
            },
            _ => true,
        },
        _ => false,
    }
}

/// Runs one native call.
pub(super) async fn dispatch(
    engine: &Engine,
    op: &str,
    db: &str,
    coll: &str,
    args: Vec<JsonValue>,
) -> AppResult<JsonValue> {
    if is_write(op, &args) {
        if let Gate::Simulated(result) = gate(engine, op, db, coll, &args).await? {
            return Ok(result);
        }
    }
    let result = run(engine, op, db, coll, &args).await?;
    count_write(engine, op, db, coll, &args, &result);
    Ok(result)
}

/// Adds what a write did to the run's tally.
fn count_write(
    engine: &Engine,
    op: &str,
    db: &str,
    coll: &str,
    args: &[JsonValue],
    result: &JsonValue,
) {
    if !is_write(op, args) {
        return;
    }
    let n = |key: &str| result.get(key).and_then(json_count).unwrap_or(0);
    let mut tally = engine.tally.borrow_mut();
    match op {
        "insertOne" => tally.inserted += 1,
        "insertMany" => tally.inserted += n("insertedCount"),
        "updateOne" | "updateMany" | "replaceOne" | "bulkWrite" => {
            tally.inserted += n("insertedCount");
            tally.matched += n("matchedCount");
            tally.modified += n("modifiedCount");
            tally.upserted += n("upsertedCount");
            tally.deleted += n("deletedCount");
        }
        "deleteOne" | "deleteMany" => tally.deleted += n("deletedCount"),
        "findOneAndDelete" if !result.is_null() => tally.deleted += 1,
        "findOneAndUpdate" | "findOneAndReplace" if !result.is_null() => tally.modified += 1,
        "findOneAndDelete" | "findOneAndUpdate" | "findOneAndReplace" => {}
        _ => tally.other.push(format!("{op} on {}", ns(db, coll))),
    }
}

/// A count in canonical Extended JSON.
fn json_count(value: &JsonValue) -> Option<u64> {
    match value {
        JsonValue::Number(n) => n.as_u64(),
        JsonValue::Object(map) => map
            .get("$numberInt")
            .or_else(|| map.get("$numberLong"))
            .and_then(JsonValue::as_str)
            .and_then(|s| s.parse().ok()),
        _ => None,
    }
}

async fn run(
    engine: &Engine,
    op: &str,
    db: &str,
    coll: &str,
    args: &[JsonValue],
) -> AppResult<JsonValue> {
    let database = engine.client.database(db);
    match op {
        "find" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let (mut options, max_time, _): (FindOptions, _, _) = options(args, 1, op, FIND)?;
            options.max_time = max_time;
            let size = options
                .batch_size
                .map_or(BATCH, |b| (b as usize).clamp(1, BATCH));
            let cursor = collection(engine, db, coll)?
                .find(filter)
                .with_options(options)
                .await?;
            engine.keep_cursor(cursor, size).await
        }
        "findOne" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let (mut options, max_time, _): (FindOptions, _, _) = options(args, 1, op, FIND)?;
            options.max_time = max_time;
            options.limit = Some(1);
            let docs: Vec<Document> = collection(engine, db, coll)?
                .find(filter)
                .with_options(options)
                .await?
                .try_collect()
                .await?;
            Ok(docs
                .into_iter()
                .next()
                .map(canon_doc)
                .unwrap_or(JsonValue::Null))
        }
        "aggregate" => {
            let pipeline = pipeline_arg(args, 0, op)?;
            let (mut options, max_time, _): (AggregateOptions, _, _) =
                options(args, 1, op, AGGREGATE)?;
            options.max_time = max_time;
            let size = options
                .batch_size
                .map_or(BATCH, |b| (b as usize).clamp(1, BATCH));
            let cursor = if coll.is_empty() {
                database.aggregate(pipeline).with_options(options).await?
            } else {
                collection(engine, db, coll)?
                    .aggregate(pipeline)
                    .with_options(options)
                    .await?
            };
            engine.keep_cursor(cursor, size).await
        }
        "more" => {
            let id = arg(args, 0).and_then(json_count).unwrap_or(0) as u32;
            let cursor = engine.cursors.borrow_mut().remove(&id);
            let Some(mut cursor) = cursor else {
                return Ok(json!({ "docs": [], "done": true }));
            };
            let (docs, done) = take_batch(&mut cursor, BATCH).await?;
            if !done {
                engine.cursors.borrow_mut().insert(id, cursor);
            }
            Ok(json!({ "docs": docs, "done": done }))
        }
        "close" => {
            let id = arg(args, 0).and_then(json_count).unwrap_or(0) as u32;
            engine.cursors.borrow_mut().remove(&id);
            Ok(JsonValue::Null)
        }
        "countDocuments" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let (mut options, max_time, _): (CountOptions, _, _) = options(args, 1, op, COUNT)?;
            options.max_time = max_time;
            let n = collection(engine, db, coll)?
                .count_documents(filter)
                .with_options(options)
                .await?;
            // a plain number, as in mongosh, so `n === 0` works
            Ok(json!(n))
        }
        "estimatedDocumentCount" => {
            let (mut options, max_time, _): (EstimatedDocumentCountOptions, _, _) =
                options(args, 0, op, ESTIMATED)?;
            options.max_time = max_time;
            let n = collection(engine, db, coll)?
                .estimated_document_count()
                .with_options(options)
                .await?;
            // a plain number, as in mongosh, so `n === 0` works
            Ok(json!(n))
        }
        "distinct" => {
            let field = arg(args, 0)
                .and_then(JsonValue::as_str)
                .ok_or_else(|| invalid("distinct needs a field name"))?
                .to_string();
            let filter = doc_arg(args, 1, op, "the filter")?;
            let (mut options, max_time, _): (DistinctOptions, _, _) =
                options(args, 2, op, DISTINCT)?;
            options.max_time = max_time;
            let values = collection(engine, db, coll)?
                .distinct(field, filter)
                .with_options(options)
                .await?;
            Ok(canon(Bson::Array(values)))
        }
        "insertOne" => {
            let mut doc = required_doc(args, 0, op, "a document")?;
            ensure_id(&mut doc);
            let (options, _, _): (InsertOneOptions, _, _) = options(args, 1, op, INSERT_ONE)?;
            let result = collection(engine, db, coll)?
                .insert_one(doc)
                .with_options(options)
                .await?;
            Ok(json!({ "acknowledged": true, "insertedId": canon(result.inserted_id) }))
        }
        "insertMany" => {
            let docs = docs_arg(args, 0, op)?;
            let (options, _, _): (InsertManyOptions, _, _) = options(args, 1, op, INSERT_MANY)?;
            let result = collection(engine, db, coll)?
                .insert_many(docs)
                .with_options(options)
                .await?;
            let mut ids: Vec<_> = result.inserted_ids.into_iter().collect();
            ids.sort_by_key(|(index, _)| *index);
            let ids: Vec<JsonValue> = ids.into_iter().map(|(_, id)| canon(id)).collect();
            Ok(json!({ "acknowledged": true, "insertedCount": ids.len(), "insertedIds": ids }))
        }
        "updateOne" | "updateMany" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let update = update_arg(args, 1, op)?;
            let (options, _, _): (UpdateOptions, _, _) = options(args, 2, op, UPDATE)?;
            let c = collection(engine, db, coll)?;
            let result = if op == "updateOne" {
                c.update_one(filter, update).with_options(options).await?
            } else {
                c.update_many(filter, update).with_options(options).await?
            };
            Ok(update_result(
                result.matched_count,
                result.modified_count,
                result.upserted_id,
            ))
        }
        "replaceOne" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let replacement = replacement_arg(args, 1, op)?;
            let (options, _, _): (ReplaceOptions, _, _) = options(args, 2, op, REPLACE)?;
            let result = collection(engine, db, coll)?
                .replace_one(filter, replacement)
                .with_options(options)
                .await?;
            Ok(update_result(
                result.matched_count,
                result.modified_count,
                result.upserted_id,
            ))
        }
        "deleteOne" | "deleteMany" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let (options, _, _): (DeleteOptions, _, _) = options(args, 1, op, DELETE)?;
            let c = collection(engine, db, coll)?;
            let result = if op == "deleteOne" {
                c.delete_one(filter).with_options(options).await?
            } else {
                c.delete_many(filter).with_options(options).await?
            };
            Ok(json!({ "acknowledged": true, "deletedCount": result.deleted_count }))
        }
        "findOneAndUpdate" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let update = update_arg(args, 1, op)?;
            let (mut options, max_time, _): (FindOneAndUpdateOptions, _, _) =
                options(args, 2, op, FIND_AND_UPDATE)?;
            options.max_time = max_time;
            let doc = collection(engine, db, coll)?
                .find_one_and_update(filter, update)
                .with_options(options)
                .await?;
            Ok(doc.map(canon_doc).unwrap_or(JsonValue::Null))
        }
        "findOneAndReplace" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let replacement = replacement_arg(args, 1, op)?;
            let (mut options, max_time, _): (FindOneAndReplaceOptions, _, _) =
                options(args, 2, op, FIND_AND_REPLACE)?;
            options.max_time = max_time;
            let doc = collection(engine, db, coll)?
                .find_one_and_replace(filter, replacement)
                .with_options(options)
                .await?;
            Ok(doc.map(canon_doc).unwrap_or(JsonValue::Null))
        }
        "findOneAndDelete" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let (mut options, max_time, _): (FindOneAndDeleteOptions, _, _) =
                options(args, 1, op, FIND_AND_DELETE)?;
            options.max_time = max_time;
            let doc = collection(engine, db, coll)?
                .find_one_and_delete(filter)
                .with_options(options)
                .await?;
            Ok(doc.map(canon_doc).unwrap_or(JsonValue::Null))
        }
        "bulkWrite" => {
            let ops = bulk_ops(args, op)?;
            let raw = raw_options(args, 1, op, BULK)?;
            let ordered = raw.get_bool("ordered").unwrap_or(true);
            let bypass = raw.get_bool("bypassDocumentValidation").ok();
            collection(engine, db, coll)?;
            let result = bulk::bulk_write(&engine.client, db, coll, ops, ordered, bypass).await?;
            let ids = |list: Vec<(usize, Bson)>| -> JsonValue {
                JsonValue::Object(
                    list.into_iter()
                        .map(|(i, id)| (i.to_string(), canon(id)))
                        .collect(),
                )
            };
            Ok(json!({
                "acknowledged": true,
                "insertedCount": result.inserted,
                "matchedCount": result.matched,
                "modifiedCount": result.modified,
                "deletedCount": result.deleted,
                "upsertedCount": result.upserted,
                "insertedIds": ids(result.inserted_ids),
                "upsertedIds": ids(result.upserted_ids),
            }))
        }
        "getIndexes" => {
            let reply = database.run_command(doc! { "listIndexes": coll }).await?;
            let indexes = reply
                .get_document("cursor")
                .and_then(|c| c.get_array("firstBatch"))
                .cloned()
                .unwrap_or_default();
            Ok(canon(Bson::Array(indexes)))
        }
        "createIndex" | "createIndexes" => {
            let keys_list: Vec<Document> = if op == "createIndex" {
                vec![required_doc(args, 0, op, "the index keys")?]
            } else {
                match arg(args, 0) {
                    Some(JsonValue::Array(items)) => items
                        .iter()
                        .map(|k| json_to_document(k.clone()))
                        .collect::<AppResult<_>>()?,
                    _ => return Err(invalid("createIndexes expects an array of index keys")),
                }
            };
            let options = doc_arg(args, 1, op, "the index options")?;
            let indexes: Vec<Bson> = keys_list
                .into_iter()
                .map(|keys| {
                    let mut spec = doc! { "key": keys.clone() };
                    spec.extend(options.clone());
                    if !spec.contains_key("name") {
                        spec.insert("name", index_name(&keys));
                    }
                    Bson::Document(spec)
                })
                .collect();
            let names: Vec<Bson> = indexes
                .iter()
                .filter_map(|i| i.as_document()?.get("name").cloned())
                .collect();
            database
                .run_command(doc! { "createIndexes": coll, "indexes": indexes })
                .await?;
            Ok(if op == "createIndex" {
                canon(names.into_iter().next().unwrap_or(Bson::Null))
            } else {
                canon(Bson::Array(names))
            })
        }
        "dropIndex" | "dropIndexes" => {
            let index = match arg(args, 0) {
                None => Bson::String("*".to_string()),
                Some(JsonValue::String(name)) => Bson::String(name.clone()),
                Some(v @ JsonValue::Object(_)) => Bson::Document(json_to_document(v.clone())?),
                Some(JsonValue::Array(names)) => Bson::Array(
                    names
                        .iter()
                        .filter_map(|n| n.as_str().map(|s| Bson::String(s.to_string())))
                        .collect(),
                ),
                Some(_) => return Err(invalid(format!("{op} takes an index name or key pattern"))),
            };
            let reply = database
                .run_command(doc! { "dropIndexes": coll, "index": index })
                .await?;
            Ok(canon_doc(reply))
        }
        "drop" => {
            collection(engine, db, coll)?.drop().await?;
            Ok(JsonValue::Bool(true))
        }
        "stats" => Ok(canon_doc(
            database.run_command(doc! { "collStats": coll }).await?,
        )),
        "getCollectionNames" => {
            let mut names = database.list_collection_names().await?;
            names.sort();
            Ok(json!(names))
        }
        "getCollectionInfos" => {
            let filter = doc_arg(args, 0, op, "the filter")?;
            let reply = database
                .run_command(doc! { "listCollections": 1, "filter": filter })
                .await?;
            let infos = reply
                .get_document("cursor")
                .and_then(|c| c.get_array("firstBatch"))
                .cloned()
                .unwrap_or_default();
            Ok(canon(Bson::Array(infos)))
        }
        "getDatabaseNames" => Ok(json!(engine.client.list_database_names().await?)),
        "createCollection" => {
            if coll.is_empty() {
                return Err(invalid("createCollection needs a collection name"));
            }
            let mut command = doc! { "create": coll };
            command.extend(doc_arg(args, 0, op, "the options")?);
            database.run_command(command).await?;
            Ok(json!({ "ok": 1 }))
        }
        "runCommand" => {
            let command = required_doc(args, 0, op, "a command document")?;
            Ok(canon_doc(database.run_command(command).await?))
        }
        "explain" => {
            let kind = arg(args, 0).and_then(JsonValue::as_str).unwrap_or("find");
            let options = doc_arg(args, 2, op, "the options")?;
            let verbosity = arg(args, 3)
                .and_then(JsonValue::as_str)
                .unwrap_or("queryPlanner");
            let verbosity = match verbosity {
                "queryPlanner" | "executionStats" | "allPlansExecution" => verbosity.to_string(),
                other => return Err(invalid(format!("explain: unknown verbosity \"{other}\" (queryPlanner, executionStats or allPlansExecution)"))),
            };
            let mut command = if kind == "aggregate" {
                doc! { "aggregate": if coll.is_empty() { Bson::Int32(1) } else { Bson::String(coll.to_string()) }, "pipeline": pipeline_arg(args, 1, op)?, "cursor": {} }
            } else {
                doc! { "find": coll, "filter": doc_arg(args, 1, op, "the filter")? }
            };
            for (key, value) in options {
                if !["batchSize", "noCursorTimeout"].contains(&key.as_str()) {
                    command.insert(key, value);
                }
            }
            Ok(canon_doc(
                database
                    .run_command(doc! { "explain": command, "verbosity": verbosity })
                    .await?,
            ))
        }
        "sleep" => {
            let ms = arg(args, 0)
                .and_then(JsonValue::as_f64)
                .unwrap_or(0.0)
                .max(0.0) as u64;
            tokio::time::sleep(Duration::from_millis(ms).min(MAX_SLEEP)).await;
            Ok(JsonValue::Null)
        }
        other => Err(invalid(format!("the engine has no operation \"{other}\""))),
    }
}

fn update_result(matched: u64, modified: u64, upserted: Option<Bson>) -> JsonValue {
    let upserted_count = u64::from(upserted.is_some());
    json!({
        "acknowledged": true,
        "matchedCount": matched,
        "modifiedCount": modified,
        "upsertedCount": upserted_count,
        "upsertedId": upserted.map(canon),
    })
}

/// The name the server would give: `a_1_b_-1`.
fn index_name(keys: &Document) -> String {
    keys.iter()
        .map(|(k, v)| {
            let v = match v {
                Bson::String(s) => s.clone(),
                Bson::Int32(n) => n.to_string(),
                Bson::Int64(n) => n.to_string(),
                Bson::Double(n) if n.fract() == 0.0 => format!("{}", *n as i64),
                other => other.to_string(),
            };
            format!("{k}_{v}")
        })
        .collect::<Vec<_>>()
        .join("_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::options::ReturnDocument;

    #[test]
    fn unknown_options_are_refused_by_name() {
        let args = vec![
            json!({}),
            json!({ "$set": { "a": 1 } }),
            json!({ "upsert": true, "multi": true }),
        ];
        let err = options::<UpdateOptions>(&args, 2, "updateOne", UPDATE).unwrap_err();
        assert!(
            err.to_string()
                .contains("doesn't support the option \"multi\""),
            "{err}"
        );
        let args = vec![json!({}), json!({ "upsert": true, "hint": "a_1" })];
        let (opts, _, raw): (UpdateOptions, _, _) = options(&args, 1, "updateOne", UPDATE).unwrap();
        assert_eq!(opts.upsert, Some(true));
        assert!(raw.get_bool("upsert").unwrap());
    }

    #[test]
    fn max_time_and_return_new_document_are_read() {
        let args = vec![json!({ "maxTimeMS": 1500, "allowDiskUse": true, "hint": { "a": 1 } })];
        let (opts, max, _): (AggregateOptions, _, _) =
            options(&args, 0, "aggregate", AGGREGATE).unwrap();
        assert_eq!(max, Some(Duration::from_millis(1500)));
        assert_eq!(opts.allow_disk_use, Some(true));
        assert!(opts.hint.is_some());
        let args = vec![json!({ "returnNewDocument": true, "upsert": true })];
        let (opts, _, _): (FindOneAndUpdateOptions, _, _) =
            options(&args, 0, "findOneAndUpdate", FIND_AND_UPDATE).unwrap();
        assert!(matches!(opts.return_document, Some(ReturnDocument::After)));
    }

    #[test]
    fn writes_are_told_from_reads() {
        assert!(is_write("updateOne", &[]));
        assert!(!is_write("find", &[]));
        assert!(is_write(
            "aggregate",
            &[json!([{ "$match": {} }, { "$out": "x" }])]
        ));
        assert!(!is_write("aggregate", &[json!([{ "$match": {} }])]));
        assert!(!is_write("runCommand", &[json!({ "ping": 1 })]));
        assert!(is_write("runCommand", &[json!({ "dropDatabase": 1 })]));
        assert!(is_write("runCommand", &[json!({ "profile": 2 })]));
        assert!(is_write(
            "runCommand",
            &[json!({ "validate": "x", "repair": true })]
        ));
        assert!(is_write(
            "runCommand",
            &[json!({ "aggregate": "x", "pipeline": [{ "$merge": "y" }] })]
        ));
    }

    #[test]
    fn updates_and_replacements_are_not_mixed_up() {
        let err = update_arg(&[json!({ "a": 1 })], 0, "updateOne").unwrap_err();
        assert!(err.to_string().contains("replaceOne"), "{err}");
        let err = replacement_arg(&[json!({ "$set": { "a": 1 } })], 0, "replaceOne").unwrap_err();
        assert!(err.to_string().contains("updateOne"), "{err}");
        assert!(matches!(
            update_arg(&[json!([{ "$set": { "a": 1 } }])], 0, "updateOne").unwrap(),
            UpdateModifications::Pipeline(_)
        ));
    }

    #[test]
    fn bulk_operations_are_read_and_checked() {
        let ops = bulk_ops(&[json!([
            { "insertOne": { "document": { "a": 1 } } },
            { "updateOne": { "filter": { "a": 1 }, "update": { "$set": { "b": 1 } }, "upsert": true } },
            { "deleteMany": { "filter": {} } },
        ])], "bulkWrite").unwrap();
        assert_eq!(ops.len(), 3);
        assert!(matches!(
            ops[1],
            WriteOp::Update {
                upsert: true,
                multi: false,
                ..
            }
        ));
        let err = bulk_ops(
            &[json!([{ "updateOne": { "filter": {}, "update": { "$set": {} }, "multi": true } }])],
            "bulkWrite",
        )
        .unwrap_err();
        assert!(err.to_string().contains("\"multi\""), "{err}");
    }

    #[test]
    fn index_names_follow_the_server() {
        assert_eq!(index_name(&doc! { "a": 1, "b": -1 }), "a_1_b_-1");
        assert_eq!(index_name(&doc! { "t": "text" }), "t_text");
    }
}

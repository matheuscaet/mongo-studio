//! The built-in script engine: JavaScript on QuickJS with a mongosh-like
//! shell (`prelude.js`) whose calls to MongoDB go through one native,
//! `__native(op, db, coll, args)` (`ops`). Used by the console and by the
//! MCP server's run_script.

mod ops;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mongodb::bson::Bson;
use mongodb::Client;
use rquickjs::function::{Async, Opt};
use rquickjs::{Array, AsyncContext, AsyncRuntime, Ctx, Function, Object, Type, Value};
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::error::{AppError, AppResult};
use crate::state::CancelToken;

/// Scripts run until they finish or this timeout elapses, whichever comes
/// first. 60s comfortably covers slow aggregations without letting a runaway
/// `while(true){}` hang the app indefinitely.
const DEFAULT_TIMEOUT_MS: u64 = 60_000;

const PRELUDE: &str = include_str!("prelude.js");

/// What a script may do to the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScriptMode {
    #[default]
    Normal,
    /// Every write is refused, and stops the script.
    ReadOnly,
    /// Writes aren't made: the engine counts what each would touch and
    /// hands the script a made-up result.
    DryRun,
}

/// What a run changed, as the server reported it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteTally {
    pub inserted: u64,
    pub matched: u64,
    pub modified: u64,
    pub upserted: u64,
    pub deleted: u64,
    /// Other changes: `createIndex on shop.users`...
    pub other: Vec<String>,
}

impl WriteTally {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `3 inserted, 12 modified` or `nothing`.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = [
            (self.inserted, "inserted"),
            (self.modified, "modified"),
            (self.upserted, "upserted"),
            (self.deleted, "deleted"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect();
        if self.matched > self.modified && parts.iter().any(|p| p.ends_with("modified")) {
            parts.push(format!("{} matched", self.matched));
        }
        parts.extend(self.other.iter().cloned());
        if parts.is_empty() {
            "nothing".to_string()
        } else {
            parts.join(", ")
        }
    }
}

#[derive(Debug)]
pub struct ScriptExecutionResult {
    pub value: JsonValue,
    pub logs: Vec<String>,
    /// The database `db` pointed at when the script ended: the one it
    /// started on, unless a `use` line switched it.
    pub database: String,
    /// What the writes changed.
    pub writes: WriteTally,
    /// A dry run's writes, one line each: what each would have touched.
    pub planned: Vec<String>,
}

/// Runs `script` against `database` on a dedicated OS thread so the
/// single-threaded QuickJS runtime (its types aren't `Send`) never has to
/// cross into Tauri's command-dispatch future, which must be `Send`. The
/// dedicated thread reuses the *same* Tokio runtime handle as the rest of
/// the app (via `Handle::block_on`, not a brand new `Runtime`) so the
/// `mongodb::Client`'s pooled connections - registered with the main
/// runtime's I/O reactor - stay usable; a second independent runtime here
/// would panic the first time a socket read/write crossed reactors.
pub async fn run_script(
    client: Client,
    database: String,
    script: String,
    timeout_ms: Option<u64>,
    cancel: CancelToken,
    on_log: impl Fn(String) + Send + 'static,
) -> AppResult<ScriptExecutionResult> {
    run_script_in(
        client,
        database,
        script,
        timeout_ms,
        ScriptMode::Normal,
        cancel,
        on_log,
    )
    .await
}

/// `run_script` in a given mode.
pub async fn run_script_in(
    client: Client,
    database: String,
    script: String,
    timeout_ms: Option<u64>,
    mode: ScriptMode,
    cancel: CancelToken,
    on_log: impl Fn(String) + Send + 'static,
) -> AppResult<ScriptExecutionResult> {
    let timeout_ms = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    // The timeout stops runaway JavaScript; unlike Cancel it doesn't cut a
    // query short, so a long aggregation can still finish.
    let timed_out = Arc::new(AtomicBool::new(false));
    let watchdog_flag = timed_out.clone();
    let watchdog = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(timeout_ms)).await;
        watchdog_flag.store(true, Ordering::Relaxed);
    });

    let handle = tokio::runtime::Handle::current();
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let outcome = handle.block_on(execute(
            client, database, script, mode, cancel, timed_out, on_log,
        ));
        let _ = tx.send(outcome);
    });

    let outcome = rx
        .await
        .map_err(|_| AppError::InvalidInput("script execution thread panicked".to_string()))?;
    watchdog.abort();
    outcome
}

async fn execute(
    client: Client,
    database: String,
    script: String,
    mode: ScriptMode,
    cancel: CancelToken,
    timed_out: Arc<AtomicBool>,
    on_log: impl Fn(String) + Send + 'static,
) -> AppResult<ScriptExecutionResult> {
    let js_rt = AsyncRuntime::new().map_err(|e| AppError::InvalidInput(e.to_string()))?;
    let interrupt = cancel.flag();
    js_rt
        .set_interrupt_handler(Some(Box::new(move || {
            interrupt.load(Ordering::Relaxed) || timed_out.load(Ordering::Relaxed)
        })))
        .await;
    let ctx = AsyncContext::full(&js_rt)
        .await
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;

    let logs = Rc::new(RefCell::new(Vec::<String>::new()));
    let engine = Rc::new(ops::Engine::new(client, mode));

    let outcome: Result<(JsonValue, String), String> = ctx
        .async_with(async |ctx| {
            install_globals(
                &ctx,
                engine.clone(),
                database,
                cancel.clone(),
                logs.clone(),
                on_log,
            )
            .map_err(|e| describe_js_error(&ctx, e))?;
            let promise = ctx
                .eval_promise(rewrite_use(&script))
                .map_err(|e| describe_js_error(&ctx, e))?;
            let value: Value = promise
                .into_future()
                .await
                .map_err(|e| describe_js_error(&ctx, e))?;
            // `eval_promise` runs the script as an async function body (to
            // support top-level await), and QuickJS wraps that completion
            // value the same way an async function's return value is
            // wrapped: as `{ value: <result> }` rather than the bare value.
            let unwrapped = match value.as_object() {
                Some(obj) if obj.contains_key("value").unwrap_or(false) => {
                    obj.get::<_, Value>("value").unwrap_or(value.clone())
                }
                _ => value,
            };
            // A cursor left as the last expression is read, as mongosh
            // would print it; dates and ids become Extended JSON.
            let finish: Function = ctx
                .globals()
                .get("__finish")
                .map_err(|e| describe_js_error(&ctx, e))?;
            let finished: rquickjs::Promise = finish
                .call((unwrapped,))
                .map_err(|e| describe_js_error(&ctx, e))?;
            let finished: Value = finished
                .into_future()
                .await
                .map_err(|e| describe_js_error(&ctx, e))?;
            let value = js_to_json(&finished).map_err(|e| describe_js_error(&ctx, e))?;
            let database: String = ctx
                .globals()
                .get("__currentDb")
                .map_err(|e| describe_js_error(&ctx, e))?;
            Ok((relaxed(value), database))
        })
        .await;

    match outcome {
        Ok((value, database)) => Ok(ScriptExecutionResult {
            value,
            logs: logs.borrow().clone(),
            database,
            writes: engine.tally.borrow().clone(),
            planned: engine.planned.borrow().clone(),
        }),
        Err(message) => Err(AppError::InvalidInput(message)),
    }
}

/// Extended JSON as the app shows it (relaxed: plain numbers, ISO dates);
/// a value that isn't Extended JSON stays as it is.
fn relaxed(value: JsonValue) -> JsonValue {
    match Bson::try_from(value.clone()) {
        Ok(bson) => bson.into_relaxed_extjson(),
        Err(_) => value,
    }
}

/// mongosh's `use <name>` isn't JavaScript. A line holding only that (a
/// `;` and a `//` comment may follow) becomes `db = __use("<name>");`,
/// on the same line so error positions still match the text.
fn rewrite_use(script: &str) -> String {
    script
        .split('\n')
        .map(|line| use_target(line).map_or_else(|| line.to_string(), use_call))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The database a `use` line names, if the line is one. Also used by the
/// mongosh engine, which rewrites the same lines its own way.
pub(crate) fn use_target(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("use")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ';' || c == '/')
        .unwrap_or(rest.len());
    let (name, tail) = rest.split_at(end);
    let tail = tail.trim_start();
    let tail = tail.strip_prefix(';').unwrap_or(tail).trim();
    (!name.is_empty() && (tail.is_empty() || tail.starts_with("//"))).then_some(name)
}

fn use_call(name: &str) -> String {
    format!("db = __use({});", serde_json::Value::from(name))
}

fn describe_js_error(ctx: &Ctx<'_>, err: rquickjs::Error) -> String {
    if err.is_exception() {
        let caught = ctx.catch();
        // The natives throw their error message as a plain string.
        if let Some(message) = caught.as_string().and_then(|s| s.to_string().ok()) {
            return message;
        }
        if let Some(ex) = caught.into_object() {
            if let Ok(message) = ex.get::<_, rquickjs::String>("message") {
                if let Ok(s) = message.to_string() {
                    return s;
                }
            }
            if let Ok(json) = js_to_json(ex.as_value()) {
                return json.to_string();
            }
        }
    }
    err.to_string()
}

fn app_err_to_js<'js>(ctx: &Ctx<'js>, err: AppError) -> rquickjs::Error {
    let message = err.to_string();
    let value = rquickjs::String::from_str(ctx.clone(), &message)
        .map(|s| s.into_value())
        .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
    ctx.throw(value)
}

fn install_globals<'js>(
    ctx: &Ctx<'js>,
    engine: Rc<ops::Engine>,
    database: String,
    cancel: CancelToken,
    logs: Rc<RefCell<Vec<String>>>,
    on_log: impl Fn(String) + 'js,
) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    globals.set(
        "__log",
        Function::new(ctx.clone(), move |line: String| {
            logs.borrow_mut().push(line.clone());
            on_log(line);
        }),
    )?;
    globals.set(
        "__format",
        Function::new(
            ctx.clone(),
            move |value: Value<'js>, pretty: Opt<bool>| -> rquickjs::Result<String> {
                let json = relaxed(js_to_json(&value)?);
                Ok(if pretty.0.unwrap_or(false) {
                    serde_json::to_string_pretty(&json).unwrap_or_default()
                } else {
                    json.to_string()
                })
            },
        ),
    )?;
    globals.set(
        "__new_object_id",
        Function::new(ctx.clone(), || mongodb::bson::oid::ObjectId::new().to_hex()),
    )?;
    globals.set(
        "__new_uuid",
        Function::new(ctx.clone(), || uuid::Uuid::new_v4().to_string()),
    )?;
    // where `db` starts; `use` moves it
    globals.set("__currentDb", database)?;
    globals.set(
        "__native",
        Function::new(
            ctx.clone(),
            Async(
                move |ctx: Ctx<'js>, op: String, db: String, coll: String, args: Value<'js>| {
                    let engine = engine.clone();
                    let cancel = cancel.clone();
                    async move {
                        let args = match js_to_json(&args) {
                            Ok(JsonValue::Array(args)) => args,
                            Ok(_) => Vec::new(),
                            Err(e) => return Err(app_err_to_js(&ctx, invalid(e))),
                        };
                        let result = cancel
                            .guard(ops::dispatch(&engine, &op, &db, &coll, args))
                            .await
                            .map_err(|e| app_err_to_js(&ctx, e))?;
                        json_to_js(&ctx, &result)
                    }
                },
            ),
        )?,
    )?;
    ctx.eval_promise(PRELUDE)?;
    Ok(())
}

fn invalid(err: rquickjs::Error) -> AppError {
    AppError::InvalidInput(err.to_string())
}

/// A plain JS value as JSON. Objects keep their own enumerable properties;
/// the prelude has already turned dates, ids and the like into Extended
/// JSON.
fn js_to_json(value: &Value<'_>) -> rquickjs::Result<JsonValue> {
    match value.type_of() {
        Type::Uninitialized | Type::Undefined | Type::Null => Ok(JsonValue::Null),
        Type::Bool => Ok(JsonValue::Bool(value.as_bool().unwrap_or(false))),
        Type::Int => Ok(JsonValue::from(value.as_int().unwrap_or(0))),
        Type::Float => Ok(
            serde_json::Number::from_f64(value.as_float().unwrap_or(0.0))
                .map(JsonValue::Number)
                .unwrap_or(JsonValue::Null),
        ),
        Type::String => {
            let s = value.as_string().unwrap();
            Ok(JsonValue::String(s.to_string()?))
        }
        Type::Array => {
            let arr = value.as_array().unwrap();
            let mut out = Vec::with_capacity(arr.len());
            for item in arr.iter::<Value>() {
                out.push(js_to_json(&item?)?);
            }
            Ok(JsonValue::Array(out))
        }
        Type::Object | Type::Exception | Type::Proxy => {
            let obj = value.as_object().unwrap();
            let mut map = serde_json::Map::new();
            for prop in obj.props::<std::string::String, Value>() {
                let (k, v) = prop?;
                map.insert(k, js_to_json(&v)?);
            }
            Ok(JsonValue::Object(map))
        }
        // Functions, symbols, promises, etc. have no JSON representation.
        _ => Ok(JsonValue::Null),
    }
}

fn json_to_js<'js>(ctx: &Ctx<'js>, value: &JsonValue) -> rquickjs::Result<Value<'js>> {
    match value {
        JsonValue::Null => Ok(Value::new_null(ctx.clone())),
        JsonValue::Bool(b) => Ok(Value::new_bool(ctx.clone(), *b)),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                if let Ok(i32_val) = i32::try_from(i) {
                    return Ok(Value::new_int(ctx.clone(), i32_val));
                }
            }
            Ok(Value::new_float(ctx.clone(), n.as_f64().unwrap_or(0.0)))
        }
        JsonValue::String(s) => Ok(rquickjs::String::from_str(ctx.clone(), s)?.into_value()),
        JsonValue::Array(items) => {
            let arr = Array::new(ctx.clone())?;
            for (i, item) in items.iter().enumerate() {
                arr.set(i, json_to_js(ctx, item)?)?;
            }
            Ok(arr.into_value())
        }
        JsonValue::Object(map) => {
            let obj = Object::new(ctx.clone())?;
            for (k, v) in map {
                obj.set(k.as_str(), json_to_js(ctx, v)?)?;
            }
            Ok(obj.into_value())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_lines_become_database_switches() {
        assert_eq!(rewrite_use("use chat"), r#"db = __use("chat");"#);
        assert_eq!(
            rewrite_use("  use users;  // the users db"),
            r#"db = __use("users");"#
        );
        assert_eq!(
            rewrite_use("use chat\nawait db.collection('x').find({});"),
            "db = __use(\"chat\");\nawait db.collection('x').find({});"
        );
    }

    #[test]
    fn other_lines_are_left_alone() {
        for line in [
            "const use = 1;",
            "use = 2",
            "use(x)",
            "user.find()",
            "\"use strict\";",
            "use chat extra",
            "// use chat",
        ] {
            assert_eq!(rewrite_use(line), line);
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::connection::extract_uri_credentials;
    use crate::models::{
        ConnectionAdvancedOptions, ConnectionProfile, ConnectionSource, TlsOptions,
    };
    use crate::secrets::{InMemoryStore, SecretKind, SecretStore};
    use crate::ssh_tunnel::KnownHosts;
    use std::sync::Mutex;

    /// Connects to the live MongoDB instance named by `MONGO_STUDIO_TEST_URI`
    /// (see driver.rs's live_tests for the same pattern) and returns the
    /// client plus the database name from the URI path, if any.
    async fn live_client() -> (Client, String) {
        let uri = std::env::var("MONGO_STUDIO_TEST_URI")
            .expect("set MONGO_STUDIO_TEST_URI to run this test");
        let (stripped_uri, username, password) = extract_uri_credentials(&uri);
        let database = stripped_uri
            .rsplit('/')
            .next()
            .and_then(|tail| tail.split('?').next())
            .filter(|s| !s.is_empty())
            .unwrap_or("test")
            .to_string();

        let secrets = InMemoryStore::new();
        let profile_id = "live-script-test".to_string();
        if let Some(password) = &password {
            secrets
                .set(&profile_id, SecretKind::Password, password)
                .unwrap();
        }
        let profile = ConnectionProfile {
            id: profile_id,
            name: "live script test".to_string(),
            color: None,
            source: ConnectionSource::Uri { uri: stripped_uri },
            database: None,
            username,
            has_password: password.is_some(),
            tls: TlsOptions::default(),
            ssh_tunnel: None,
            advanced: ConnectionAdvancedOptions::default(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        let known_hosts = Arc::new(
            KnownHosts::load(&std::env::temp_dir().join("mongo-studio-script-test")).unwrap(),
        );
        let active = crate::driver::connect(&profile, &secrets, &known_hosts)
            .await
            .expect("connect should succeed");
        (active.client, database)
    }

    fn noop_log() -> impl Fn(String) + Send + 'static {
        |_| {}
    }

    #[tokio::test]
    #[ignore]
    async fn live_script_basic_expression() {
        let (client, database) = live_client().await;
        let result = run_script(
            client,
            database,
            "1 + 1".to_string(),
            Some(5_000),
            CancelToken::default(),
            noop_log(),
        )
        .await
        .unwrap();
        assert_eq!(result.value, JsonValue::from(2));
    }

    #[tokio::test]
    #[ignore]
    async fn live_script_console_log_is_captured() {
        let (client, database) = live_client().await;
        let logs = Arc::new(Mutex::new(Vec::<String>::new()));
        let logs_for_cb = logs.clone();
        let result = run_script(
            client,
            database,
            "console.log('hello', 42); 'done'".to_string(),
            Some(5_000),
            CancelToken::default(),
            move |m| logs_for_cb.lock().unwrap().push(m),
        )
        .await
        .unwrap();
        assert_eq!(result.value, JsonValue::String("done".to_string()));
        assert_eq!(result.logs, vec!["hello 42".to_string()]);
        assert_eq!(logs.lock().unwrap().clone(), vec!["hello 42".to_string()]);
    }

    #[tokio::test]
    #[ignore]
    async fn live_script_can_query_a_collection() {
        let (client, database) = live_client().await;
        let dbs = crate::driver::list_databases(&client).await.unwrap();
        let db_name = dbs
            .iter()
            .find(|d| d.name == database)
            .map(|d| d.name.clone())
            .unwrap_or(database);
        let collections = crate::driver::list_collections(&client, &db_name)
            .await
            .unwrap();
        let Some(collection) = collections.first() else {
            println!("database {db_name} has no collections, skipping");
            return;
        };
        let script = format!(
            "await db.collection(\"{}\").countDocuments({{}})",
            collection.name
        );
        let result = run_script(
            client,
            db_name,
            script,
            Some(10_000),
            CancelToken::default(),
            noop_log(),
        )
        .await
        .unwrap();
        assert!(result.value.is_number());
    }

    /// insertMany/updateMany/deleteMany against a throwaway collection:
    ///   cargo test --lib -- --ignored live_script_many_writes
    #[tokio::test]
    #[ignore]
    async fn live_script_many_writes() {
        let (client, _) = live_client().await;
        let coll = format!("script_many_{}", uuid::Uuid::new_v4().simple());
        let script = format!(
            r#"
            const c = db.collection("{coll}");
            const inserted = await c.insertMany([{{ n: 1 }}, {{ n: 2 }}, {{ n: 3 }}]);
            const updated = await c.updateMany({{ n: {{ $gte: 2 }} }}, {{ $set: {{ big: true }} }});
            const deleted = await c.deleteMany({{ big: true }});
            const left = await c.countDocuments({{}});
            ({{ inserted, updated, deleted, left }})
            "#
        );
        let result = run_script(
            client.clone(),
            "mongo_studio_test".to_string(),
            script,
            Some(10_000),
            CancelToken::default(),
            noop_log(),
        )
        .await;
        client
            .database("mongo_studio_test")
            .collection::<mongodb::bson::Document>(&coll)
            .drop()
            .await
            .unwrap();
        let value = result.unwrap().value;
        assert_eq!(value["inserted"]["insertedCount"], 3);
        let ids = value["inserted"]["insertedIds"].as_array().unwrap();
        assert_eq!(ids.len(), 3);
        assert!(ids[0]["$oid"].is_string(), "{value}");
        assert_eq!(value["updated"]["matchedCount"], 2);
        assert_eq!(value["updated"]["modifiedCount"], 2);
        assert_eq!(value["updated"]["upsertedId"], JsonValue::Null);
        assert_eq!(value["deleted"]["deletedCount"], 2);
        assert_eq!(value["left"], 1);
    }

    async fn run_in(
        client: &Client,
        script: &str,
        mode: ScriptMode,
    ) -> AppResult<ScriptExecutionResult> {
        run_script_in(
            client.clone(),
            "mongo_studio_test".to_string(),
            script.to_string(),
            Some(20_000),
            mode,
            CancelToken::default(),
            noop_log(),
        )
        .await
    }

    /// Dates, ids, longs, regexes and UUIDs reach the server as BSON types
    /// and come back as the same JS types.
    #[tokio::test]
    #[ignore]
    async fn live_script_bson_types_round_trip() {
        let (client, _) = live_client().await;
        let coll = format!("script_types_{}", uuid::Uuid::new_v4().simple());
        let script = format!(
            r#"
            const c = db["{coll}"];
            const when = new Date("2026-03-04T05:06:07.000Z");
            await c.insertOne({{ _id: 1, when, at: ISODate("2026-01-01"), n: NumberLong("9007199254740993"),
                                 i: NumberInt(7), d: NumberDecimal("1.10"), u: UUID("0123456789abcdef0123456789abcdef"),
                                 name: "Ana" }});
            const doc = await c.findOne({{ when: {{ $gte: new Date("2026-01-01") }}, name: /^an/i }});
            const types = await c.aggregate([{{ $project: {{ when: {{ $type: "$when" }}, n: {{ $type: "$n" }}, i: {{ $type: "$i" }}, d: {{ $type: "$d" }}, u: {{ $type: "$u" }} }} }}]).toArray();
            ({{ isDate: doc.when instanceof Date, ms: doc.when.getTime(), long: doc.n.toString(), u: doc.u.toString(), types: types[0], doc }})
            "#
        );
        let result = run_in(&client, &script, ScriptMode::Normal).await;
        client
            .database("mongo_studio_test")
            .collection::<mongodb::bson::Document>(&coll)
            .drop()
            .await
            .unwrap();
        let value = result.unwrap().value;
        assert_eq!(value["isDate"], true, "{value}");
        // a JS number past 32 bits is a double, as in mongosh
        assert_eq!(value["ms"].as_f64(), Some(1772600767000.0), "{value}");
        assert_eq!(value["long"], "9007199254740993");
        assert_eq!(value["u"], "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(value["types"]["when"], "date", "{value}");
        assert_eq!(value["types"]["n"], "long");
        assert_eq!(value["types"]["i"], "int");
        assert_eq!(value["types"]["d"], "decimal");
        assert_eq!(value["types"]["u"], "binData");
        // shown as relaxed Extended JSON, like before
        assert_eq!(
            value["doc"]["when"]["$date"], "2026-03-04T05:06:07Z",
            "{value}"
        );
    }

    /// mongosh's shapes: db.coll, chained cursors, toArray, forEach,
    /// distinct, bulkWrite, upsert options, getName.
    #[tokio::test]
    #[ignore]
    async fn live_script_speaks_more_mongosh() {
        let (client, _) = live_client().await;
        let coll = format!("script_shell_{}", uuid::Uuid::new_v4().simple());
        let script = format!(
            r#"
            const c = db.{coll};
            await c.insertMany(Array.from({{ length: 1200 }}, (_, i) => ({{ i, kind: i % 3 ? "a" : "b" }})));
            const top = await c.find({{ kind: "a" }}, {{ _id: 0, i: 1 }}).sort({{ i: -1 }}).limit(2).toArray();
            let seen = 0;
            await c.find().forEach(() => {{ seen++; }});
            const all = (await c.find().toArray()).length;
            const bare = (await c.find()).length;
            const kinds = await c.distinct("kind");
            const up = await c.updateOne({{ key: "x" }}, {{ $setOnInsert: {{ made: true }} }}, {{ upsert: true }});
            const bulk = await c.bulkWrite([
              {{ insertOne: {{ document: {{ i: -1 }} }} }},
              {{ updateMany: {{ filter: {{ kind: "b" }}, update: {{ $set: {{ flagged: true }} }} }} }},
              {{ updateOne: {{ filter: {{ key: "y" }}, update: {{ $set: {{ v: 1 }} }}, upsert: true }} }},
              {{ deleteOne: {{ filter: {{ i: 0 }} }} }},
            ]);
            let bad = null;
            try {{ await c.updateOne({{}}, {{ $set: {{ a: 1 }} }}, {{ multi: true }}); }} catch (e) {{ bad = String(e); }}
            const n = await c.countDocuments({{}}, {{ maxTimeMS: 5000, hint: {{ _id: 1 }} }});
            const plain = typeof n === "number" && n === 1202 && typeof (await c.estimatedDocumentCount()) === "number";
            ({{ top, seen, all, bare, kinds, up, bulk, bad, n, plain, name: db.getName(), full: c.getFullName() }})
            "#
        );
        let result = run_in(&client, &script, ScriptMode::Normal).await;
        client
            .database("mongo_studio_test")
            .collection::<mongodb::bson::Document>(&coll)
            .drop()
            .await
            .unwrap();
        let result = result.unwrap();
        let value = result.value;
        assert_eq!(
            value["top"],
            serde_json::json!([{ "i": 1199 }, { "i": 1198 }]),
            "{value}"
        );
        assert_eq!(value["seen"], 1200);
        assert_eq!(value["all"], 1200);
        assert_eq!(value["bare"], 1000);
        assert!(
            result
                .logs
                .iter()
                .any(|l| l.contains("showing the first 1000")),
            "{:?}",
            result.logs
        );
        assert_eq!(value["kinds"], serde_json::json!(["a", "b"]));
        assert_eq!(value["up"]["upsertedCount"], 1, "{value}");
        assert_eq!(value["bulk"]["insertedCount"], 1, "{value}");
        assert_eq!(value["bulk"]["modifiedCount"], 400, "{value}");
        assert_eq!(value["bulk"]["upsertedCount"], 1, "{value}");
        assert_eq!(value["bulk"]["deletedCount"], 1, "{value}");
        assert!(
            value["bad"].as_str().unwrap().contains("\"multi\""),
            "{value}"
        );
        assert_eq!(value["n"], 1202, "{value}");
        assert_eq!(value["plain"], true, "counts are plain numbers: {value}");
        assert_eq!(value["name"], "mongo_studio_test");
        assert_eq!(result.writes.inserted, 1201, "{:?}", result.writes);
        assert_eq!(result.writes.upserted, 2, "{:?}", result.writes);
        assert_eq!(result.writes.deleted, 1, "{:?}", result.writes);
    }

    /// Read-only refuses writes; a dry run counts them without making them.
    #[tokio::test]
    #[ignore]
    async fn live_script_read_only_and_dry_run() {
        let (client, _) = live_client().await;
        let coll = format!("script_modes_{}", uuid::Uuid::new_v4().simple());
        run_in(
            &client,
            &format!("await db.{coll}.insertMany([{{s: 1}}, {{s: 1}}, {{s: 2}}])"),
            ScriptMode::Normal,
        )
        .await
        .unwrap();
        let read = run_in(
            &client,
            &format!("await db.{coll}.countDocuments({{ s: 1 }})"),
            ScriptMode::ReadOnly,
        )
        .await;
        let refused = run_in(
            &client,
            &format!("print('before'); await db.{coll}.deleteMany({{}})"),
            ScriptMode::ReadOnly,
        )
        .await;
        let out = run_in(
            &client,
            &format!("await db.{coll}.aggregate([{{ $match: {{}} }}, {{ $out: 'copy_{coll}' }}]).toArray()"),
            ScriptMode::ReadOnly,
        )
        .await;
        let dry = run_in(
            &client,
            &format!(
                "const r = await db.{coll}.updateMany({{ s: 1 }}, {{ $set: {{ t: 1 }} }}); \
                 await db.{coll}.insertOne({{ s: 3 }}); await db.{coll}.deleteOne({{ s: 2 }}); \
                 await db.{coll}.createIndex({{ s: 1 }}); r.matchedCount"
            ),
            ScriptMode::DryRun,
        )
        .await;
        let left = run_in(
            &client,
            &format!(
                "await db.{coll}.countDocuments({{ t: 1 }}) + await db.{coll}.countDocuments({{}})"
            ),
            ScriptMode::Normal,
        )
        .await;
        client
            .database("mongo_studio_test")
            .collection::<mongodb::bson::Document>(&coll)
            .drop()
            .await
            .unwrap();
        assert_eq!(read.unwrap().value, 2);
        let refused = refused.unwrap_err().to_string();
        assert!(
            refused.contains("read-only") && refused.contains("deleteMany"),
            "{refused}"
        );
        assert!(out.unwrap_err().to_string().contains("read-only"));
        let dry = dry.unwrap();
        assert_eq!(dry.value, 2);
        assert!(dry.writes.is_empty(), "{:?}", dry.writes);
        assert_eq!(dry.planned.len(), 4, "{:?}", dry.planned);
        assert!(
            dry.planned[0].contains("2 documents would change"),
            "{:?}",
            dry.planned
        );
        assert!(
            dry.planned[2].contains("1 document would be deleted"),
            "{:?}",
            dry.planned
        );
        // nothing changed: no t, still 3 documents
        assert_eq!(left.unwrap().value, 3);
    }

    #[tokio::test]
    #[ignore]
    async fn live_script_insert_many_rejects_a_non_array() {
        let (client, _) = live_client().await;
        let err = run_script(
            client,
            "mongo_studio_test".to_string(),
            "await db.collection(\"x\").insertMany({ n: 1 })".to_string(),
            Some(5_000),
            CancelToken::default(),
            noop_log(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("array"), "unexpected error: {err}");
    }

    #[tokio::test]
    #[ignore]
    async fn live_script_thrown_error_is_reported() {
        let (client, database) = live_client().await;
        let err = run_script(
            client,
            database,
            "throw new Error('boom')".to_string(),
            Some(5_000),
            CancelToken::default(),
            noop_log(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("boom"), "unexpected error: {err}");
    }

    #[tokio::test]
    #[ignore]
    async fn live_script_timeout_kills_infinite_loop() {
        let (client, database) = live_client().await;
        let started = std::time::Instant::now();
        let err = run_script(
            client,
            database,
            "while (true) {}".to_string(),
            Some(300),
            CancelToken::default(),
            noop_log(),
        )
        .await
        .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout took too long to take effect"
        );
        println!("timeout error: {err}");
    }

    #[tokio::test]
    #[ignore]
    async fn live_script_cancel_stops_a_query_in_flight() {
        let (client, database) = live_client().await;
        let coll = format!("cancel_probe_{}", uuid::Uuid::new_v4().simple());
        let setup = format!(
            "await db.collection('{coll}').insertMany([{{a: 1}}, {{a: 2}}, {{a: 3}}]); true"
        );
        run_script(
            client.clone(),
            database.clone(),
            setup,
            Some(5_000),
            CancelToken::default(),
            noop_log(),
        )
        .await
        .unwrap();

        // each document sleeps 3s on the server: the query takes 9s unless cut short
        let cancel = CancelToken::default();
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            canceller.cancel();
        });
        let started = std::time::Instant::now();
        let slow =
            format!("await db.collection('{coll}').find({{ $where: 'sleep(3000) || true' }})");
        let err = run_script(
            client.clone(),
            database.clone(),
            slow,
            Some(60_000),
            cancel,
            noop_log(),
        )
        .await
        .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cancel took {:?}",
            started.elapsed()
        );
        assert!(
            err.to_string().contains("cancelled"),
            "unexpected error: {err}"
        );

        let drop = format!("await db.collection('{coll}').deleteMany({{}}); true");
        run_script(
            client,
            database,
            drop,
            Some(5_000),
            CancelToken::default(),
            noop_log(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn live_script_use_switches_the_database() {
        let (client, database) = live_client().await;
        let other = format!("{database}_use_probe");
        let script = format!(
            "use {other}\nawait db.collection('x').insertOne({{ a: 1 }});\nconst n = await db.collection('x').countDocuments({{}});\nconst back = await db.getSiblingDB('{database}').collection('x').countDocuments({{ a: 1 }});\nawait db.collection('x').deleteMany({{}});\n[db.getName(), n, back]"
        );
        let result = run_script(
            client,
            database.clone(),
            script,
            Some(10_000),
            CancelToken::default(),
            noop_log(),
        )
        .await
        .unwrap();
        assert_eq!(result.database, other);
        assert_eq!(result.value[0], JsonValue::from(other.as_str()));
        assert_eq!(result.value[1], JsonValue::from(1));
        assert_eq!(result.logs, vec![format!("switched to db {other}")]);
    }
}

//! Running a console script through mongosh as a child process.
//!
//! mongosh runs with `--nodb --quiet --json=canonical` and two `--eval`s:
//! a prelude that connects (the credentialed URI comes in the child's
//! environment, never on its command line), sends `print`/`console.*` to
//! stderr and lets a cursor left as the result come back as documents; then
//! the script itself. stdout carries only the final value as Extended JSON,
//! or the error as JSON when mongosh exits with 1.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value as JsonValue;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

use crate::error::{AppError, AppResult};
use crate::models::ScriptResult;
use crate::state::CancelToken;

/// Same as the built-in engine's.
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// How many documents a cursor left as the result brings back, like the
/// built-in engine's default `find` limit.
const CURSOR_LIMIT: usize = 1000;

pub(super) const AWAIT_HINT: &str =
    "mongosh awaits for you: remove `await` (or wrap the code in an async function)";
const CURSOR_HINT: &str =
    "The result holds a cursor mongosh can't print: end the script with the cursor on its own, \
     or call .toArray() on it";

/// The first `--eval`. `__MARKER__` becomes a per-run marker for the line
/// that reports the database the script ended on.
///
/// A cursor that is the script's value can't be turned into JSON by
/// mongosh. Cursors get a `then` that only works once the script's last
/// statement has run (`__msDone`, set by the line `script_eval` appends)
/// and only for cursors the script itself already handled, so mongosh's
/// own awaits inside the script still see cursors, while the await on the
/// final value reads the documents.
const PRELUDE: &str = r#"db = connect(process.env.MS_URI);
db = db.getSiblingDB(process.env.MS_DB);
globalThis.__msDone = false;
globalThis.__msReportDb = () => {
  try { process.stderr.write("__MARKER__" + db.getName() + "\n"); } catch (e) {}
};
process.on("exit", () => __msReportDb());
(() => {
  const text = (x) => {
    if (typeof x === "string") return x;
    try { return EJSON.stringify(x, { relaxed: true }); } catch (e) { return String(x); }
  };
  const write = (...args) => { process.stderr.write(args.map(text).join(" ") + "\n"); };
  globalThis.print = write;
  globalThis.printjson = write;
  console.log = write;
  console.info = write;
  console.warn = write;
  console.error = write;
  console.debug = write;
})();
(() => {
  let proto = Object.getPrototypeOf(db.getCollection("__mongo_studio_probe").find());
  while (proto && !Object.prototype.hasOwnProperty.call(proto, "toArray")) {
    proto = Object.getPrototypeOf(proto);
  }
  if (!proto) return;
  const seen = new WeakSet();
  Object.defineProperty(proto, "then", {
    configurable: true,
    get() {
      if (!globalThis.__msDone) { seen.add(this); return undefined; }
      if (!seen.has(this)) return undefined;
      seen.delete(this);
      return function (resolve, reject) {
        const docs = [];
        try {
          while (docs.length < __LIMIT__ && this.hasNext()) docs.push(this.next());
          if (this.hasNext()) print("Showing the first __LIMIT__ documents.");
        } catch (e) { reject(e); return; }
        resolve(docs);
      };
    },
  });
})();
"#;

/// Appended to the script. A `var` statement leaves the script's last
/// value as the eval's value.
const TRAILER: &str = "\n;var __msEnd = (globalThis.__msDone = true, __msReportDb());";

pub fn prelude(marker: &str) -> String {
    PRELUDE
        .replace("__MARKER__", marker)
        .replace("__LIMIT__", &CURSOR_LIMIT.to_string())
}

pub fn script_eval(script: &str) -> String {
    format!("{}{TRAILER}", rewrite_shell_commands(script))
}

/// mongosh's `use x` and `show x` shell commands aren't allowed in
/// `--eval`, but its `use("x")` and `show("x")` functions are. A line
/// holding only such a command (a `;` and a `//` comment may follow)
/// becomes the call, on the same line so error positions still match.
pub fn rewrite_shell_commands(script: &str) -> String {
    script
        .split('\n')
        .map(|line| {
            if let Some(name) = crate::scripting::use_target(line) {
                format!("use({});", JsonValue::from(name))
            } else if let Some(args) = show_args(line) {
                let args: Vec<String> = args
                    .iter()
                    .map(|a| JsonValue::from(*a).to_string())
                    .collect();
                format!("show({});", args.join(", "))
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a `show` line asks for: `show dbs` -> `["dbs"]`,
/// `show log global` -> `["log", "global"]`.
fn show_args(line: &str) -> Option<Vec<&str>> {
    let rest = line.trim_start().strip_prefix("show")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.split_once("//").map_or(rest, |(code, _)| code).trim();
    let rest = rest.strip_suffix(';').unwrap_or(rest).trim_end();
    let args: Vec<&str> = rest.split_whitespace().collect();
    let word = |w: &&str| {
        w.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    };
    let first_is_name = args
        .first()
        .is_some_and(|w| w.starts_with(|c: char| c.is_ascii_alphabetic()));
    (first_is_name && args.len() <= 2 && args.iter().all(word)).then_some(args)
}

/// Splits extra command-line arguments the way a POSIX shell would:
/// whitespace separates, single quotes keep everything, double quotes keep
/// everything but `\"` and `\\`, a backslash outside quotes escapes the
/// next character.
pub fn split_args(text: &str) -> AppResult<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_arg = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_arg {
                    args.push(std::mem::take(&mut current));
                    in_arg = false;
                }
            }
            '\'' => {
                in_arg = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return Err(unclosed("'")),
                    }
                }
            }
            '"' => {
                in_arg = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => current.push(c),
                            Some(c) => {
                                current.push('\\');
                                current.push(c);
                            }
                            None => return Err(unclosed("\"")),
                        },
                        Some(c) => current.push(c),
                        None => return Err(unclosed("\"")),
                    }
                }
            }
            '\\' => {
                in_arg = true;
                if let Some(c) = chars.next() {
                    current.push(c);
                }
            }
            c => {
                in_arg = true;
                current.push(c);
            }
        }
    }
    if in_arg {
        args.push(current);
    }
    Ok(args)
}

fn unclosed(quote: &str) -> AppError {
    AppError::InvalidInput(format!(
        "the extra mongosh arguments have an unclosed {quote} quote"
    ))
}

/// The script's value from mongosh's stdout, in the shape the built-in
/// engine returns: relaxed Extended JSON, as `bson_to_json` writes it.
/// Nothing printed means the script ended on `undefined`.
pub fn parse_value(stdout: &str) -> JsonValue {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return JsonValue::Null;
    }
    match serde_json::from_str::<JsonValue>(trimmed) {
        Ok(value) => relax(unwrap_shell_result(value)),
        // Not JSON after all (something else wrote to stdout): show it as text.
        Err(_) => JsonValue::String(trimmed.to_string()),
    }
}

/// mongosh's `show(...)` results print as `{ type: "...Result", value }`;
/// the value is what the views can show.
fn unwrap_shell_result(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(mut map)
            if map.len() == 2
                && map.contains_key("value")
                && map
                    .get("type")
                    .and_then(JsonValue::as_str)
                    .is_some_and(|t| t.ends_with("Result")) =>
        {
            map.remove("value").unwrap_or(JsonValue::Null)
        }
        other => other,
    }
}

/// Canonical Extended JSON to relaxed, through BSON. A part that isn't
/// valid Extended JSON (say, a document with a `$`-prefixed key of its
/// own) is kept as it is, with its children converted.
fn relax(value: JsonValue) -> JsonValue {
    match bson::Bson::try_from(value.clone()) {
        Ok(bson) => bson.into_relaxed_extjson(),
        Err(_) => match value {
            JsonValue::Object(map) => {
                JsonValue::Object(map.into_iter().map(|(k, v)| (k, relax(v))).collect())
            }
            JsonValue::Array(items) => JsonValue::Array(items.into_iter().map(relax).collect()),
            other => other,
        },
    }
}

/// The message for a failed run: the error mongosh printed on stdout, or
/// failing that, what it wrote last on stderr.
pub fn error_message(stdout: &str, stderr_tail: Option<&str>, status: &str) -> String {
    let message = match serde_json::from_str::<JsonValue>(stdout.trim()) {
        Ok(err @ JsonValue::Object(_)) if err.get("message").is_some() => describe_error(&err),
        _ => {
            let stdout = stdout.trim();
            if !stdout.is_empty() {
                stdout.to_string()
            } else if let Some(line) = stderr_tail {
                line.to_string()
            } else {
                format!("mongosh exited with {status}")
            }
        }
    };
    crate::logging::scrub_credentials(&message)
}

/// mongosh's error JSON (`{ message, name, code?, codeName?, reasonCode? }`)
/// spelled the way its shell prints errors: `MongoServerError[BadValue]: …`.
fn describe_error(err: &JsonValue) -> String {
    let message = match &err["message"] {
        JsonValue::String(s) => s.clone(),
        other => other.to_string(),
    };
    if err["reasonCode"] == "AwaitNotInAsyncContext" {
        return AWAIT_HINT.to_string();
    }
    if message.starts_with("Converting circular structure to EJSON") && message.contains("_mongo") {
        return CURSOR_HINT.to_string();
    }
    match err["name"]
        .as_str()
        .filter(|n| !n.is_empty() && *n != "Error")
    {
        Some(name) => match err["codeName"].as_str() {
            Some(code) => format!("{name}[{code}]: {message}"),
            None => format!("{name}: {message}"),
        },
        None if err["code"] == "BABEL_PARSE_ERROR" => format!("SyntaxError: {message}"),
        None => message,
    }
}

/// Runs mongosh (`program`) on `script` and collects what it reports.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    program: &Path,
    uri: String,
    database: String,
    script: String,
    extra_args: &str,
    load_rc: bool,
    timeout_ms: Option<u64>,
    cancel: CancelToken,
    on_log: impl Fn(String) + Send + Sync + 'static,
) -> AppResult<ScriptResult> {
    let marker = format!("@@mongo-studio-db-{}:", uuid::Uuid::new_v4().simple());
    let mut command = Command::new(program);
    command.args(["--nodb", "--quiet", "--json=canonical"]);
    if !load_rc {
        command.arg("--norc");
    }
    command
        .args(split_args(extra_args)?)
        .arg("--eval")
        .arg(prelude(&marker))
        .arg("--eval")
        .arg(script_eval(&script))
        .env("MS_URI", uri)
        .env("MS_DB", &database)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW: no console window flashing up for each run.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| AppError::InvalidInput(format!("couldn't start mongosh: {e}")))?;

    // Both pipes are drained as the script runs, so a big result can't fill
    // one up and stall mongosh.
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    });
    let stderr = child.stderr.take().expect("stderr is piped");
    let stderr_marker = marker.clone();
    let stderr_task = tokio::spawn(async move {
        let mut logs = Vec::new();
        let mut final_db = None;
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(name) = line.strip_prefix(&stderr_marker) {
                final_db = Some(name.to_string());
                continue;
            }
            on_log(line.clone());
            logs.push(line);
        }
        (logs, final_db)
    });

    let timeout_ms = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let status = tokio::select! {
        status = child.wait() => status?,
        () = cancel.cancelled() => {
            let _ = child.kill().await;
            return Err(AppError::Cancelled);
        }
        () = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {
            let _ = child.kill().await;
            return Err(AppError::InvalidInput(format!(
                "the script didn't finish within {} and was stopped",
                describe_ms(timeout_ms)
            )));
        }
    };
    let stdout = stdout_task.await.unwrap_or_default();
    let (logs, final_db) = stderr_task.await.unwrap_or_default();

    if !status.success() {
        let tail = logs.iter().rev().find(|l| !l.trim().is_empty());
        return Err(AppError::InvalidInput(error_message(
            &stdout,
            tail.map(String::as_str),
            &status.to_string(),
        )));
    }
    Ok(ScriptResult {
        value: parse_value(&stdout),
        logs,
        database: final_db.filter(|d| !d.is_empty()).unwrap_or(database),
    })
}

pub(crate) fn describe_ms(ms: u64) -> String {
    if ms.is_multiple_of(1000) {
        format!("{} s", ms / 1000)
    } else {
        format!("{ms} ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn use_and_show_lines_become_calls() {
        assert_eq!(rewrite_shell_commands("use shop"), r#"use("shop");"#);
        assert_eq!(
            rewrite_shell_commands("  use users;  // the users db"),
            r#"use("users");"#
        );
        assert_eq!(rewrite_shell_commands("show dbs"), r#"show("dbs");"#);
        assert_eq!(
            rewrite_shell_commands("show collections; // here"),
            r#"show("collections");"#
        );
        assert_eq!(
            rewrite_shell_commands("show log global"),
            r#"show("log", "global");"#
        );
        assert_eq!(
            rewrite_shell_commands("use shop\ndb.orders.find()\nshow dbs"),
            "use(\"shop\");\ndb.orders.find()\nshow(\"dbs\");"
        );
    }

    #[test]
    fn other_lines_are_left_alone() {
        for line in [
            "const show = 1;",
            "show = 2",
            "show(\"dbs\")",
            "shows dbs",
            "// show dbs",
            "show a b c",
            "show 'dbs'",
            "show",
            "use(x)",
            "use chat extra",
        ] {
            assert_eq!(rewrite_shell_commands(line), line);
        }
    }

    #[test]
    fn the_script_is_followed_by_the_end_marker_on_its_own_line() {
        let eval = script_eval("db.x.find() // last");
        assert!(
            eval.starts_with("db.x.find() // last\n;var __msEnd"),
            "{eval}"
        );
    }

    #[test]
    fn the_prelude_takes_the_marker_and_limit() {
        let p = prelude("@@m:");
        assert!(p.contains("\"@@m:\" + db.getName()"));
        assert!(p.contains("docs.length < 1000"));
        assert!(!p.contains("__MARKER__") && !p.contains("__LIMIT__"));
    }

    #[test]
    fn extra_args_split_like_a_shell() {
        assert_eq!(split_args("").unwrap(), Vec::<String>::new());
        assert_eq!(
            split_args("  --tlsUseSystemCA   --apiVersion 1 ").unwrap(),
            vec!["--tlsUseSystemCA", "--apiVersion", "1"]
        );
        assert_eq!(
            split_args(r#"--file 'my dir/a.js' "x \"y\" \\ \z" a\ b ''"#).unwrap(),
            vec!["--file", "my dir/a.js", r#"x "y" \ \z"#, "a b", ""]
        );
        assert!(split_args("'open").is_err());
        assert!(split_args("\"open").is_err());
    }

    #[test]
    fn canonical_values_come_back_relaxed() {
        assert_eq!(parse_value(""), JsonValue::Null);
        assert_eq!(parse_value("\n"), JsonValue::Null);
        assert_eq!(parse_value(r#"{ "$numberInt": "5" }"#), json!(5));
        assert_eq!(parse_value(r#""text""#), json!("text"));
        assert_eq!(parse_value("true"), json!(true));
        let docs = parse_value(
            r#"[
              { "_id": { "$oid": "6ab072d14abe768149d80644" },
                "qty": { "$numberInt": "3" },
                "total": { "$numberDouble": "12.5" },
                "big": { "$numberLong": "9007199254740993" },
                "at": { "$date": { "$numberLong": "1767225600000" } },
                "price": { "$numberDecimal": "10.25" },
                "tags": ["a", { "$numberInt": "1" }] }
            ]"#,
        );
        assert_eq!(
            docs,
            json!([{
                "_id": { "$oid": "6ab072d14abe768149d80644" },
                "qty": 3,
                "total": 12.5,
                "big": 9007199254740993_i64,
                "at": { "$date": "2026-01-01T00:00:00Z" },
                "price": { "$numberDecimal": "10.25" },
                "tags": ["a", 1]
            }])
        );
    }

    #[test]
    fn values_match_what_the_built_in_engine_sends() {
        let doc = bson::doc! {
            "_id": bson::oid::ObjectId::parse_str("6ab072d14abe768149d80644").unwrap(),
            "n": 1_i32,
            "at": bson::DateTime::from_millis(1_767_225_600_123),
        };
        let canonical = bson::Bson::Document(doc.clone()).into_canonical_extjson();
        assert_eq!(
            parse_value(&canonical.to_string()),
            crate::ejson::document_to_json(doc)
        );
    }

    #[test]
    fn show_results_are_unwrapped() {
        let value = parse_value(
            r#"{ "type": "ShowDatabasesResult",
                 "value": [{ "name": "shop", "sizeOnDisk": { "$numberInt": "40960" }, "empty": false }] }"#,
        );
        assert_eq!(
            value,
            json!([{ "name": "shop", "sizeOnDisk": 40960, "empty": false }])
        );
        // an object of the user's own with other keys stays as it is
        assert_eq!(
            parse_value(r#"{ "type": "x", "value": 1, "more": true }"#),
            json!({ "type": "x", "value": 1, "more": true })
        );
    }

    #[test]
    fn documents_that_are_not_extended_json_keep_their_shape() {
        assert_eq!(
            parse_value(r#"{ "op": { "$set": { "n": { "$numberInt": "2" } } } }"#),
            json!({ "op": { "$set": { "n": 2 } } })
        );
    }

    #[test]
    fn non_json_output_is_shown_as_text() {
        assert_eq!(parse_value("hello\n"), json!("hello"));
    }

    #[test]
    fn errors_read_like_mongosh_prints_them() {
        let thrown = r#"{ "message": "boom", "stack": "Error: boom\n at x", "name": "Error" }"#;
        assert_eq!(error_message(thrown, None, "exit status: 1"), "boom");
        let type_error = r#"{ "message": "x is not a function", "name": "TypeError" }"#;
        assert_eq!(
            error_message(type_error, None, ""),
            "TypeError: x is not a function"
        );
        let server = r#"{ "errorLabelSet": {}, "ok": 0, "code": { "$numberInt": "2" },
            "codeName": "BadValue", "message": "unknown operator: $x",
            "name": "MongoServerError" }"#;
        assert_eq!(
            error_message(server, None, ""),
            "MongoServerError[BadValue]: unknown operator: $x"
        );
    }

    #[test]
    fn top_level_await_gets_the_hint() {
        let err = r#"{ "code": "BABEL_PARSE_ERROR", "reasonCode": "AwaitNotInAsyncContext",
            "message": "'await' is only allowed within async functions and at the top levels of modules. (1:0)" }"#;
        assert_eq!(error_message(err, None, ""), AWAIT_HINT);
        let other = r#"{ "code": "BABEL_PARSE_ERROR", "reasonCode": "UnexpectedToken",
            "message": "Unexpected token (1:3)" }"#;
        assert_eq!(
            error_message(other, None, ""),
            "SyntaxError: Unexpected token (1:3)"
        );
    }

    #[test]
    fn a_cursor_that_cannot_be_printed_gets_the_hint() {
        let err = r#"{ "message": "Converting circular structure to EJSON:\n    (root) -> _mongo -> __serviceProvider -> mongoClient", "name": "BSONError" }"#;
        assert_eq!(error_message(err, None, ""), CURSOR_HINT);
    }

    #[test]
    fn errors_without_json_fall_back_to_stderr_then_status() {
        assert_eq!(
            error_message("", Some("Error parsing command line"), "exit status: 1"),
            "Error parsing command line"
        );
        assert_eq!(
            error_message("", None, "exit status: 2"),
            "mongosh exited with exit status: 2"
        );
    }

    #[test]
    fn credentials_never_reach_the_message() {
        let err = r#"{ "message": "bad auth for mongodb://bob:hunter2@db:27017/x", "name": "MongoServerError" }"#;
        let message = error_message(err, None, "");
        assert!(!message.contains("hunter2"), "{message}");
    }
}

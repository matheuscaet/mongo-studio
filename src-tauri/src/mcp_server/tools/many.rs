//! run_script_on_connections: one script on several connections, one after
//! the other, approved once for all of them.

use serde_json::Value;

use super::{
    code_on, invalid, json_of, run_script, script_mode, script_of, Call, Done, ToolOutput,
    MAX_CODE, SCRIPT_READ_ONLY_HINT,
};
use crate::assistant::{bool_arg, plural, str_arg, truncate};
use crate::error::{AppError, AppResult};
use crate::logging;
use crate::mcp_server::policy::{self, Answer, Cap, Decision, Request};
use crate::mcp_server::tasks::Phase;
use crate::mcp_server::Level;
use crate::models::ConnectionProfile;
use crate::scripting::{ScriptMode, WriteTally};

/// Connections per call.
const MAX_CONNECTIONS: usize = 20;

fn connection_ids(args: &Value) -> AppResult<Vec<String>> {
    let ids: Vec<String> = match json_of(args, "connection_ids")? {
        Some(Value::Array(ids)) => ids
            .iter()
            .map(|id| {
                id.as_str()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| invalid("connection_ids must be ids from list_connections."))
            })
            .collect::<AppResult<_>>()?,
        _ => {
            return Err(invalid(
                "connection_ids is required: an array of ids from list_connections.",
            ))
        }
    };
    if ids.is_empty() {
        return Err(invalid("connection_ids is empty."));
    }
    if ids.len() > MAX_CONNECTIONS {
        return Err(invalid(format!(
            "At most {MAX_CONNECTIONS} connections per call."
        )));
    }
    let mut unique = Vec::new();
    for id in ids {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    Ok(unique)
}

pub(super) async fn run(call: &mut Call<'_>, args: &Value) -> ToolOutput {
    let ids = match connection_ids(args) {
        Ok(ids) => ids,
        Err(e) => return call.fail(e),
    };
    let Some(db) = str_arg(args, "database") else {
        return call.fail(invalid("database is required."));
    };
    call.target.database = Some(db.clone());
    call.target.label = Some(plural(ids.len() as u64, "connection", "connections"));
    let script = match script_of(args) {
        Ok(script) => script,
        Err(e) => return call.fail(e),
    };

    let config = call.inner.config();
    let mode = script_mode(args);
    let need = match mode {
        ScriptMode::Normal => Level::Write,
        _ => Level::Read,
    };
    let decisions: Vec<Decision> = ids
        .iter()
        .map(|id| policy::decide(&config, Cap::Script, Some(id), need))
        .collect();
    if decisions.contains(&Decision::Off) {
        return call.refuse("Refused: turned off", policy::REFUSED_OFF);
    }
    let store = &call.inner.state().connection_store;
    let mut profiles: Vec<ConnectionProfile> = Vec::with_capacity(ids.len());
    for id in &ids {
        match store.get(id) {
            Ok(profile) => profiles.push(profile),
            Err(_) => {
                return call.fail(AppError::NotFound(format!(
                    "no saved connection has the id {id}; call list_connections"
                )))
            }
        }
    }
    let refused: Vec<&ConnectionProfile> = profiles
        .iter()
        .zip(&decisions)
        .filter(|(_, d)| **d == Decision::NoAccess)
        .map(|(p, _)| p)
        .collect();
    if !refused.is_empty() {
        let names: Vec<String> = refused.iter().map(|p| p.name.clone()).collect();
        let read_only = mode == ScriptMode::Normal
            && refused
                .iter()
                .all(|p| policy::level_of(&config, &p.id) == Level::Read);
        let why = if read_only {
            SCRIPT_READ_ONLY_HINT
        } else {
            policy::REFUSED_NO_ACCESS
        };
        let text = format!("Nothing ran. On {}: {why}", names.join(", "));
        return call.refuse("Refused: no access", &text);
    }

    let names: Vec<String> = profiles.iter().map(|p| p.name.clone()).collect();
    let label = if decisions.contains(&Decision::Ask) {
        call.task.set_phase(Phase::Approval);
        let verb = match mode {
            ScriptMode::Normal => "Run a script",
            ScriptMode::ReadOnly => "Run a read-only script",
            ScriptMode::DryRun => "Dry-run a script (nothing is written)",
        };
        let request = Request {
            tool: "run_script_on_connections",
            capability: Cap::Script,
            connection_id: None,
            connection_name: Some(names.join(", ")),
            database: Some(db.clone()),
            collection: None,
            title: format!(
                "{verb} on {} ({db})",
                plural(ids.len() as u64, "connection", "connections")
            ),
            code: truncate(code_on(&db, None, &script), MAX_CODE),
            dry_run: None,
            connection_ids: ids.clone(),
        };
        match policy::ask(call.inner, call.client, &request).await {
            Answer::Once => "Approved by you",
            Answer::Always => "Approved by you (always)",
            Answer::Deny => return call.refuse("Denied by you", policy::DENIED),
            Answer::Timeout => return call.refuse("Not answered", policy::NOT_ANSWERED),
            Answer::Closed => return call.refuse("Closed", policy::CLOSED),
        }
    } else if decisions.iter().all(|d| *d == Decision::AllowAlways) {
        "Allowed (always)"
    } else {
        "Allowed"
    };
    call.task.set_phase(Phase::Running);

    let stop_on_error = bool_arg(args, "stop_on_error");
    let mut sections = Vec::with_capacity(profiles.len());
    let mut writes = WriteTally::default();
    let (mut ok, mut failed) = (0usize, 0usize);
    let mut stopped = false;
    for profile in &profiles {
        let heading = format!("## {} ({})", profile.name, profile.id);
        if stopped {
            sections.push(format!(
                "{heading}\nNot run: stopped after a failure (stop_on_error)."
            ));
            continue;
        }
        let client = match call
            .inner
            .pool
            .client(call.inner.host.as_ref(), profile)
            .await
        {
            Ok(client) => client,
            Err(e) => {
                failed += 1;
                stopped = stop_on_error;
                sections.push(format!("{heading}\nFailed to connect: {}", message(&e)));
                continue;
            }
        };
        let outcome = run_script(call, &client, &db, args, Some(profile.name.clone())).await;
        match outcome.result {
            Ok(ran) => {
                ok += 1;
                add(&mut writes, &ran.result.writes);
                sections.push(format!("{heading}\n{}", ran.text(&db)));
            }
            Err(e) => {
                failed += 1;
                stopped = stop_on_error;
                sections.push(format!("{heading}\nFailed: {}", message(&e)));
            }
        }
    }

    let n = profiles.len();
    let mut summary = format!(
        "Ran the script on {db} on {ok} of {}",
        plural(n as u64, "connection", "connections")
    );
    if failed > 0 {
        summary.push_str(&format!("; it failed on {failed}"));
    }
    if !writes.is_empty() {
        summary.push_str(&format!(". Changed in all: {}", writes.summary()));
    }
    summary.push('.');
    let text = format!("{summary}\n\n{}", sections.join("\n\n"));
    let meta = format!("{ok}/{n} ok · {}", writes.summary());
    let label = if failed > 0 {
        format!("{label} · {failed} failed")
    } else {
        label.to_string()
    };
    call.log(&label);
    let result = if ok == 0 { "err" } else { "ok" };
    let done = Done { text, meta };
    call.finish(result, label, done.meta, done.text)
}

fn message(error: &AppError) -> String {
    logging::scrub_credentials(&error.to_string())
}

fn add(total: &mut WriteTally, more: &WriteTally) {
    total.inserted += more.inserted;
    total.matched += more.matched;
    total.modified += more.modified;
    total.upserted += more.upserted;
    total.deleted += more.deleted;
    total.other.extend(more.other.iter().cloned());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn connection_ids_are_checked_and_deduplicated() {
        assert_eq!(
            connection_ids(&json!({ "connection_ids": ["a", "b", "a"] })).unwrap(),
            ["a", "b"]
        );
        assert_eq!(
            connection_ids(&json!({ "connection_ids": "[\"a\"]" })).unwrap(),
            ["a"]
        );
        assert!(connection_ids(&json!({ "connection_ids": [] })).is_err());
        assert!(connection_ids(&json!({})).is_err());
        let many: Vec<String> = (0..21).map(|i| i.to_string()).collect();
        assert!(connection_ids(&json!({ "connection_ids": many })).is_err());
    }
}

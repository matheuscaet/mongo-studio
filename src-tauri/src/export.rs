use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures_util::TryStreamExt;
use mongodb::bson::Document;
use mongodb::Client;
use serde_json::Value as JsonValue;

use crate::ejson::{document_to_json, json_to_document, json_to_pipeline};
use crate::error::{AppError, AppResult};
use crate::models::{
    ExportFormat, ExportNestedMode, ExportOptions, ExportQueryInput, ExportSummary,
};

const DEFAULT_SAMPLE_SIZE: usize = 500;
const PROGRESS_EVERY: u64 = 200;

/// Streams a find or an aggregation to `dest_path` as CSV or JSON, the whole
/// result rather than one page, reporting progress and honouring cancel.
#[allow(clippy::too_many_arguments)]
pub async fn export_query(
    client: &Client,
    db: &str,
    collection: &str,
    query: ExportQueryInput,
    options: ExportOptions,
    dest_path: &Path,
    cancel_flag: Arc<AtomicBool>,
    on_progress: impl Fn(u64) + Send + 'static,
) -> AppResult<ExportSummary> {
    let coll = client.database(db).collection::<Document>(collection);

    let mut cursor = if let Some(pipeline) = query.pipeline {
        let stages = json_to_pipeline(pipeline)?;
        coll.aggregate(stages).await?
    } else {
        let filter = json_to_document(query.filter)?;
        let mut find = coll.find(filter);
        if let Some(sort) = query.sort {
            find = find.sort(json_to_document(sort)?);
        }
        if let Some(projection) = query.projection {
            find = find.projection(json_to_document(projection)?);
        }
        if let Some(limit) = query.limit {
            find = find.limit(limit);
        }
        find.await?
    };

    if options.format == ExportFormat::Json {
        let mut out = std::io::BufWriter::new(std::fs::File::create(dest_path)?);
        let mut rows_written: u64 = 0;
        loop {
            if cancel_flag.load(Ordering::Relaxed) {
                out.flush()?;
                return Err(AppError::InvalidInput("export cancelled".to_string()));
            }
            match cursor.try_next().await? {
                Some(doc) => {
                    write_json_item(&mut out, &document_to_json(doc), rows_written == 0)?;
                    rows_written += 1;
                    if rows_written.is_multiple_of(PROGRESS_EVERY) {
                        on_progress(rows_written);
                        out.flush()?;
                        tokio::task::yield_now().await;
                    }
                }
                None => break,
            }
        }
        finish_json_array(&mut out, rows_written)?;
        on_progress(rows_written);
        return Ok(ExportSummary {
            rows_written,
            columns: Vec::new(),
        });
    }

    let sample_size = options.sample_size.unwrap_or(DEFAULT_SAMPLE_SIZE);
    let mut sample: Vec<JsonValue> = Vec::with_capacity(sample_size.min(1024));
    while sample.len() < sample_size {
        match cursor.try_next().await? {
            Some(doc) => sample.push(document_to_json(doc)),
            None => break,
        }
    }

    let mut columns = Vec::new();
    let mut seen = HashSet::new();
    for doc in &sample {
        collect_columns(doc, options.nested_mode, &mut columns, &mut seen);
    }

    let file = std::fs::File::create(dest_path)?;
    let mut writer = csv::Writer::from_writer(file);
    writer.write_record(&columns).map_err(csv_err)?;

    let mut rows_written: u64 = 0;
    for doc in sample {
        write_row(&mut writer, &doc, &columns, options.nested_mode)?;
        rows_written += 1;
    }

    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            writer.flush()?;
            return Err(AppError::InvalidInput("export cancelled".to_string()));
        }
        match cursor.try_next().await? {
            Some(doc) => {
                let json = document_to_json(doc);
                write_row(&mut writer, &json, &columns, options.nested_mode)?;
                rows_written += 1;
                if rows_written.is_multiple_of(PROGRESS_EVERY) {
                    on_progress(rows_written);
                    writer.flush()?;
                    tokio::task::yield_now().await;
                }
            }
            None => break,
        }
    }
    writer.flush()?;
    on_progress(rows_written);

    Ok(ExportSummary {
        rows_written,
        columns,
    })
}

/// Writes a value already in memory - a script console's result - to
/// `dest_path`. An array exports one row / element per item; anything else
/// exports as a single row / value. In CSV, items that aren't documents
/// (numbers, strings...) go in a "value" column.
pub fn export_value(
    value: &JsonValue,
    options: &ExportOptions,
    dest_path: &Path,
) -> AppResult<ExportSummary> {
    let items: Vec<&JsonValue> = match value {
        JsonValue::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    let rows_written = items.len() as u64;

    if options.format == ExportFormat::Json {
        let mut out = std::io::BufWriter::new(std::fs::File::create(dest_path)?);
        serde_json::to_writer_pretty(&mut out, value)
            .map_err(|e| AppError::InvalidInput(format!("json write error: {e}")))?;
        out.write_all(b"\n")?;
        out.flush()?;
        return Ok(ExportSummary {
            rows_written,
            columns: Vec::new(),
        });
    }

    let rows: Vec<JsonValue> = items
        .into_iter()
        .map(|item| {
            if item.is_object() && ejson_leaf(item).is_none() {
                item.clone()
            } else {
                serde_json::json!({ "value": item })
            }
        })
        .collect();
    let mut columns = Vec::new();
    let mut seen = HashSet::new();
    for row in &rows {
        collect_columns(row, options.nested_mode, &mut columns, &mut seen);
    }
    let mut writer = csv::Writer::from_writer(std::fs::File::create(dest_path)?);
    writer.write_record(&columns).map_err(csv_err)?;
    for row in &rows {
        write_row(&mut writer, row, &columns, options.nested_mode)?;
    }
    writer.flush()?;
    Ok(ExportSummary {
        rows_written,
        columns,
    })
}

/// One element of a streamed JSON array, indented under its "[".
fn write_json_item<W: Write>(out: &mut W, doc: &JsonValue, first: bool) -> AppResult<()> {
    let pretty = serde_json::to_string_pretty(doc)
        .map_err(|e| AppError::InvalidInput(format!("json write error: {e}")))?;
    out.write_all(if first { b"[\n  " } else { b",\n  " })?;
    out.write_all(pretty.replace('\n', "\n  ").as_bytes())?;
    Ok(())
}

/// Closes a streamed JSON array, or writes "[]" when nothing came.
fn finish_json_array<W: Write>(out: &mut W, rows_written: u64) -> AppResult<()> {
    out.write_all(if rows_written == 0 { b"[]\n" } else { b"\n]\n" })?;
    out.flush()?;
    Ok(())
}

fn collect_columns(
    doc: &JsonValue,
    mode: ExportNestedMode,
    columns: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let JsonValue::Object(map) = doc else {
        return;
    };
    for (key, value) in map {
        match mode {
            ExportNestedMode::Stringify => insert_column(key.clone(), columns, seen),
            ExportNestedMode::Flatten => flatten_keys(key, value, columns, seen),
        }
    }
}

fn insert_column(name: String, columns: &mut Vec<String>, seen: &mut HashSet<String>) {
    if seen.insert(name.clone()) {
        columns.push(name);
    }
}

fn flatten_keys(
    prefix: &str,
    value: &JsonValue,
    columns: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    if ejson_leaf(value).is_some() {
        insert_column(prefix.to_string(), columns, seen);
        return;
    }
    match value {
        JsonValue::Object(map) if !map.is_empty() => {
            for (k, v) in map {
                flatten_keys(&format!("{prefix}.{k}"), v, columns, seen);
            }
        }
        JsonValue::Array(items) if !items.is_empty() => {
            for (i, v) in items.iter().enumerate() {
                flatten_keys(&format!("{prefix}.{i}"), v, columns, seen);
            }
        }
        _ => insert_column(prefix.to_string(), columns, seen),
    }
}

/// Recognizes our EJSON type-tag shapes (`{"$oid": ...}`, `{"$date": ...}`,
/// etc.) so flattening treats them as a single scalar column instead of
/// exploding into e.g. `_id.$oid`.
fn ejson_leaf(value: &JsonValue) -> Option<String> {
    let JsonValue::Object(map) = value else {
        return None;
    };
    if map.len() != 1 {
        return None;
    }
    let (key, inner) = map.iter().next()?;
    if key.starts_with('$') {
        Some(cell_string(inner))
    } else {
        None
    }
}

fn cell_string(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => String::new(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::String(s) => s.clone(),
        JsonValue::Array(_) | JsonValue::Object(_) => {
            ejson_leaf(value).unwrap_or_else(|| serde_json::to_string(value).unwrap_or_default())
        }
    }
}

fn get_flattened(doc: &JsonValue, path: &str) -> Option<JsonValue> {
    let mut current = doc;
    for part in path.split('.') {
        current = match current {
            JsonValue::Object(map) => map.get(part)?,
            JsonValue::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current.clone())
}

fn write_row<W: std::io::Write>(
    writer: &mut csv::Writer<W>,
    doc: &JsonValue,
    columns: &[String],
    mode: ExportNestedMode,
) -> AppResult<()> {
    let mut record = Vec::with_capacity(columns.len());
    for col in columns {
        let value = match mode {
            ExportNestedMode::Stringify => doc.get(col.as_str()).cloned(),
            ExportNestedMode::Flatten => get_flattened(doc, col),
        };
        record.push(value.map(|v| cell_string(&v)).unwrap_or_default());
    }
    writer.write_record(&record).map_err(csv_err)
}

fn csv_err(e: csv::Error) -> AppError {
    AppError::InvalidInput(format!("csv write error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flattens_nested_objects_and_arrays() {
        let doc = json!({
            "name": "Ada",
            "address": { "city": "London", "zip": "E1" },
            "tags": ["a", "b"],
        });
        let mut columns = Vec::new();
        let mut seen = HashSet::new();
        collect_columns(&doc, ExportNestedMode::Flatten, &mut columns, &mut seen);
        assert_eq!(
            columns,
            vec!["name", "address.city", "address.zip", "tags.0", "tags.1"]
        );
    }

    #[test]
    fn ejson_tagged_values_stay_a_single_column() {
        let doc = json!({ "_id": { "$oid": "507f1f77bcf86cd799439011" } });
        let mut columns = Vec::new();
        let mut seen = HashSet::new();
        collect_columns(&doc, ExportNestedMode::Flatten, &mut columns, &mut seen);
        assert_eq!(columns, vec!["_id"]);
        assert_eq!(
            get_flattened(&doc, "_id").map(|v| cell_string(&v)),
            Some("507f1f77bcf86cd799439011".to_string())
        );
    }

    fn options(format: ExportFormat) -> ExportOptions {
        ExportOptions {
            format,
            nested_mode: ExportNestedMode::Flatten,
            sample_size: None,
        }
    }

    fn temp_file(ext: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "mongo-studio-export-{}.{ext}",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn streamed_json_is_a_valid_array_for_any_count() {
        for docs in [
            vec![],
            vec![json!({"a": 1})],
            vec![json!({"a": 1}), json!({"b": {"c": [1, 2]}})],
        ] {
            let mut out = Vec::new();
            for (i, doc) in docs.iter().enumerate() {
                write_json_item(&mut out, doc, i == 0).unwrap();
            }
            finish_json_array(&mut out, docs.len() as u64).unwrap();
            let parsed: JsonValue = serde_json::from_slice(&out).unwrap();
            assert_eq!(parsed, JsonValue::Array(docs));
        }
    }

    #[test]
    fn a_console_result_exports_as_csv_rows() {
        let value = json!([
            { "_id": "delivered", "orders": 71, "revenue": { "$numberDecimal": "68412.30" } },
            { "_id": "paid", "orders": 38, "extra": { "note": "x" } },
        ]);
        let path = temp_file("csv");
        let summary = export_value(&value, &options(ExportFormat::Csv), &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(summary.rows_written, 2);
        assert_eq!(
            summary.columns,
            vec!["_id", "orders", "revenue", "extra.note"]
        );
        assert_eq!(
            text,
            "_id,orders,revenue,extra.note\ndelivered,71,68412.30,\npaid,38,,x\n"
        );
    }

    #[test]
    fn a_console_result_of_plain_values_exports_to_a_value_column() {
        let path = temp_file("csv");
        let summary =
            export_value(&json!([1, "two", null]), &options(ExportFormat::Csv), &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(summary.columns, vec!["value"]);
        // a lone empty field is quoted, or the row would read as a blank line
        assert_eq!(text, "value\n1\ntwo\n\"\"\n");
    }

    #[test]
    fn a_console_result_exports_as_json_unchanged() {
        let value = json!({ "count": 3, "ids": [{ "$oid": "507f1f77bcf86cd799439011" }] });
        let path = temp_file("json");
        let summary = export_value(&value, &options(ExportFormat::Json), &path).unwrap();
        let parsed: JsonValue =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(summary.rows_written, 1);
        assert_eq!(parsed, value);
    }

    #[test]
    fn stringify_mode_only_uses_top_level_keys() {
        let doc = json!({ "name": "Ada", "address": { "city": "London" } });
        let mut columns = Vec::new();
        let mut seen = HashSet::new();
        collect_columns(&doc, ExportNestedMode::Stringify, &mut columns, &mut seen);
        assert_eq!(columns, vec!["name", "address"]);
        assert_eq!(
            cell_string(doc.get("address").unwrap()),
            "{\"city\":\"London\"}"
        );
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

    /// Exports a real collection from the live Atlas cluster to a temp CSV
    /// file and sanity-checks the output. Skipped by default; run with:
    ///   set -a; source ../.env.local; set +a
    ///   cargo test --lib -- --ignored live_export_writes_csv
    #[tokio::test]
    #[ignore]
    async fn live_export_writes_csv() {
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
        let profile_id = "live-export-test".to_string();
        if let Some(password) = &password {
            secrets
                .set(&profile_id, SecretKind::Password, password)
                .unwrap();
        }
        let profile = ConnectionProfile {
            id: profile_id,
            name: "live export test".to_string(),
            color: None,
            environment: None,
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
            KnownHosts::load(&std::env::temp_dir().join("mongo-studio-export-test")).unwrap(),
        );
        let active = crate::driver::connect(&profile, &secrets, &known_hosts)
            .await
            .expect("connect should succeed");

        let collections = crate::driver::list_collections(&active.client, &database)
            .await
            .unwrap();
        let Some(collection) = collections.first() else {
            println!("database {database} has no collections, skipping");
            return;
        };

        let dest =
            std::env::temp_dir().join(format!("mongo-studio-export-{}.csv", uuid::Uuid::new_v4()));
        let summary = export_query(
            &active.client,
            &database,
            &collection.name,
            ExportQueryInput {
                filter: JsonValue::Object(Default::default()),
                sort: None,
                projection: None,
                pipeline: None,
                limit: None,
            },
            ExportOptions {
                format: ExportFormat::Csv,
                nested_mode: ExportNestedMode::Flatten,
                sample_size: Some(50),
            },
            &dest,
            Arc::new(AtomicBool::new(false)),
            |n| println!("progress: {n} rows"),
        )
        .await
        .expect("export should succeed");

        println!(
            "exported {} rows, {} columns to {}",
            summary.rows_written,
            summary.columns.len(),
            dest.display()
        );

        let contents = std::fs::read_to_string(&dest).unwrap();
        let line_count = contents.lines().count() as u64;
        // header + one line per row (rows_written could be 0 for an empty collection)
        assert_eq!(line_count, summary.rows_written + 1);
        std::fs::remove_file(&dest).ok();

        // The same query as JSON: an array of every document, as stored.
        let dest =
            std::env::temp_dir().join(format!("mongo-studio-export-{}.json", uuid::Uuid::new_v4()));
        let json_summary = export_query(
            &active.client,
            &database,
            &collection.name,
            ExportQueryInput {
                filter: JsonValue::Object(Default::default()),
                sort: None,
                projection: None,
                pipeline: None,
                limit: None,
            },
            ExportOptions {
                format: ExportFormat::Json,
                nested_mode: ExportNestedMode::Flatten,
                sample_size: None,
            },
            &dest,
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .expect("json export should succeed");
        let parsed: JsonValue = serde_json::from_str(&std::fs::read_to_string(&dest).unwrap())
            .expect("the export should be valid JSON");
        assert_eq!(
            parsed.as_array().map(|a| a.len() as u64),
            Some(json_summary.rows_written)
        );
        assert_eq!(json_summary.rows_written, summary.rows_written);
        std::fs::remove_file(&dest).ok();
    }
}

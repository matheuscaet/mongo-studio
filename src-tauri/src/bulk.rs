//! Many writes in few round trips, on any server version: consecutive
//! writes of a kind go out as one `insert`, `update` or `delete` command,
//! in batches. Used by the script engine's bulkWrite and by the MCP
//! server's upsert_documents.

use mongodb::bson::{doc, oid::ObjectId, Bson, Document};
use mongodb::Client;

use crate::error::{AppError, AppResult};

/// Entries per command, under the server's 100,000.
const BATCH_COUNT: usize = 1000;
/// Bytes per command, well under the 16 MiB message limit.
const BATCH_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub enum WriteOp {
    Insert(Document),
    /// An update (operators or a pipeline) or a replacement document.
    Update {
        filter: Document,
        update: Bson,
        multi: bool,
        upsert: bool,
        /// arrayFilters, hint, collation, as the update command takes them.
        extra: Document,
    },
    Delete {
        filter: Document,
        multi: bool,
        /// hint, collation.
        extra: Document,
    },
}

impl WriteOp {
    fn kind(&self) -> &'static str {
        match self {
            WriteOp::Insert(_) => "insert",
            WriteOp::Update { .. } => "update",
            WriteOp::Delete { .. } => "delete",
        }
    }

    /// The entry the command carries.
    fn entry(&self) -> Document {
        match self {
            WriteOp::Insert(doc) => doc.clone(),
            WriteOp::Update {
                filter,
                update,
                multi,
                upsert,
                extra,
            } => {
                let mut entry = doc! { "q": filter.clone(), "u": update.clone(), "multi": *multi, "upsert": *upsert };
                entry.extend(extra.clone());
                entry
            }
            WriteOp::Delete {
                filter,
                multi,
                extra,
            } => {
                let mut entry = doc! { "q": filter.clone(), "limit": if *multi { 0 } else { 1 } };
                entry.extend(extra.clone());
                entry
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct BulkResult {
    pub inserted: u64,
    pub matched: u64,
    pub modified: u64,
    pub deleted: u64,
    pub upserted: u64,
    /// (index of the operation, `_id`)
    pub inserted_ids: Vec<(usize, Bson)>,
    pub upserted_ids: Vec<(usize, Bson)>,
}

impl BulkResult {
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        for (n, what) in [
            (self.inserted, "inserted"),
            (self.matched, "matched"),
            (self.modified, "modified"),
            (self.upserted, "upserted"),
            (self.deleted, "deleted"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {what}"));
            }
        }
        if parts.is_empty() {
            "nothing changed".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// Runs `ops` in order. Ordered, it stops at the first failed write;
/// unordered, it runs them all. Either way a failure is an error that says
/// which operations failed and what the others did.
pub async fn bulk_write(
    client: &Client,
    db: &str,
    coll: &str,
    mut ops: Vec<WriteOp>,
    ordered: bool,
    bypass_document_validation: Option<bool>,
) -> AppResult<BulkResult> {
    // Inserts get their _id here, so the result can name them.
    for op in &mut ops {
        if let WriteOp::Insert(doc) = op {
            if !doc.contains_key("_id") {
                let mut with_id = doc! { "_id": ObjectId::new() };
                with_id.extend(std::mem::take(doc));
                *doc = with_id;
            }
        }
    }
    let database = client.database(db);
    let mut result = BulkResult::default();
    let mut failures: Vec<String> = Vec::new();
    let mut start = 0;
    while start < ops.len() {
        let kind = ops[start].kind();
        let mut end = start;
        let mut bytes = 0;
        let mut entries = Vec::new();
        while end < ops.len()
            && ops[end].kind() == kind
            && entries.len() < BATCH_COUNT
            && (entries.is_empty() || bytes < BATCH_BYTES)
        {
            let entry = ops[end].entry();
            bytes += entry_size(&entry);
            entries.push(Bson::Document(entry));
            end += 1;
        }
        let list = match kind {
            "insert" => "documents",
            "update" => "updates",
            _ => "deletes",
        };
        let mut command = doc! { kind: coll, list: entries, "ordered": ordered };
        if let Some(bypass) = bypass_document_validation {
            command.insert("bypassDocumentValidation", bypass);
        }
        let reply = database.run_command(command).await?;
        let n = number(reply.get("n"));
        let mut failed_here: Vec<(usize, String)> = Vec::new();
        if let Ok(errors) = reply.get_array("writeErrors") {
            for error in errors.iter().filter_map(Bson::as_document) {
                let index = start + number(error.get("index")) as usize;
                let message = error.get_str("errmsg").unwrap_or("write failed");
                let code = number(error.get("code"));
                failed_here.push((index, format!("#{index}: {message} (code {code})")));
            }
        }
        match kind {
            "insert" => {
                result.inserted += n;
                for (i, op) in ops[start..end].iter().enumerate() {
                    let index = start + i;
                    if failed_here.iter().any(|(f, _)| *f == index) {
                        continue;
                    }
                    if ordered && failed_here.first().is_some_and(|(f, _)| index > *f) {
                        break;
                    }
                    if let WriteOp::Insert(doc) = op {
                        if let Some(id) = doc.get("_id") {
                            result.inserted_ids.push((index, id.clone()));
                        }
                    }
                }
            }
            "update" => {
                let upserted: Vec<(usize, Bson)> = reply
                    .get_array("upserted")
                    .map(|list| {
                        list.iter()
                            .filter_map(Bson::as_document)
                            .map(|u| {
                                (
                                    start + number(u.get("index")) as usize,
                                    u.get("_id").cloned().unwrap_or(Bson::Null),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                result.upserted += upserted.len() as u64;
                result.matched += n.saturating_sub(upserted.len() as u64);
                result.modified += number(reply.get("nModified"));
                result.upserted_ids.extend(upserted);
            }
            _ => result.deleted += n,
        }
        if let Ok(concern) = reply.get_document("writeConcernError") {
            failures.push(format!(
                "write concern: {}",
                concern.get_str("errmsg").unwrap_or("not satisfied")
            ));
        }
        let stop = ordered && !failed_here.is_empty();
        failures.extend(failed_here.into_iter().map(|(_, m)| m));
        if stop {
            break;
        }
        start = end;
    }
    if failures.is_empty() {
        return Ok(result);
    }
    let shown: Vec<&str> = failures.iter().take(5).map(String::as_str).collect();
    let more = match failures.len() {
        n if n > 5 => format!(" (and {} more)", n - 5),
        _ => String::new(),
    };
    let how = if ordered {
        "It stopped at the first failure; the operations before it were applied"
    } else {
        "The other operations were applied"
    };
    Err(AppError::WriteFailed(format!(
        "{} of {} write operations failed: {}{more}. {how}: {}.",
        failures.len(),
        ops.len(),
        shown.join("; "),
        result.summary()
    )))
}

fn number(value: Option<&Bson>) -> u64 {
    match value {
        Some(Bson::Int32(n)) => (*n).max(0) as u64,
        Some(Bson::Int64(n)) => (*n).max(0) as u64,
        Some(Bson::Double(n)) => n.max(0.0) as u64,
        _ => 0,
    }
}

fn entry_size(entry: &Document) -> usize {
    entry.to_vec().map(|bytes| bytes.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_read_like_the_write_commands() {
        let update = WriteOp::Update {
            filter: doc! { "a": 1 },
            update: Bson::Document(doc! { "$set": { "b": 2 } }),
            multi: false,
            upsert: true,
            extra: doc! { "hint": "a_1" },
        };
        assert_eq!(
            update.entry(),
            doc! { "q": { "a": 1 }, "u": { "$set": { "b": 2 } }, "multi": false, "upsert": true, "hint": "a_1" }
        );
        let delete = WriteOp::Delete {
            filter: doc! {},
            multi: true,
            extra: doc! {},
        };
        assert_eq!(delete.entry(), doc! { "q": {}, "limit": 0 });
    }

    #[test]
    fn summaries_name_what_changed() {
        let result = BulkResult {
            inserted: 2,
            modified: 3,
            matched: 3,
            ..Default::default()
        };
        assert_eq!(result.summary(), "2 inserted, 3 matched, 3 modified");
        assert_eq!(BulkResult::default().summary(), "nothing changed");
    }
}

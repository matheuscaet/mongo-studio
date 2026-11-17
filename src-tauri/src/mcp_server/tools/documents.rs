//! upsert_documents, and what dry runs of document writes check.

use std::collections::{HashMap, HashSet};

use futures_util::TryStreamExt;
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::FindOptions;
use mongodb::Client;
use serde_json::Value;

use super::{invalid, json_of, Tool};
use crate::assistant::{bool_arg, plural};
use crate::bulk::WriteOp;
use crate::ejson::json_to_document;
use crate::error::AppResult;

/// Documents per call, as insert_documents.
const MAX_UPSERT: usize = 1000;
/// Key tuples per `$or` when looking for existing documents.
const LOOKUP_CHUNK: usize = 200;
/// Problems listed by number before "and N more".
const SHOWN: usize = 10;

#[derive(Debug)]
pub(super) struct Upsert {
    pub keys: Vec<String>,
    pub docs: Vec<Document>,
    pub only_insert: bool,
}

pub(super) fn upsert_args(args: &Value) -> AppResult<Upsert> {
    // A single key may come as a bare name.
    let single = args
        .get("keys")
        .and_then(Value::as_str)
        .filter(|k| !k.trim_start().starts_with('['))
        .map(|k| Value::String(k.to_string()));
    let keys_arg = match single {
        Some(key) => Some(key),
        None => json_of(args, "keys")?,
    };
    let keys: Vec<String> = match keys_arg {
        Some(Value::Array(keys)) => keys
            .iter()
            .map(|k| {
                k.as_str()
                    .map(str::trim)
                    .filter(|k| !k.is_empty() && !k.starts_with('$'))
                    .map(str::to_string)
                    .ok_or_else(|| {
                        invalid("keys must be field names, e.g. [\"subDomain\", \"product\"].")
                    })
            })
            .collect::<AppResult<_>>()?,
        Some(Value::String(key)) if !key.trim().is_empty() => vec![key.trim().to_string()],
        _ => {
            return Err(invalid(
                "keys is required: the fields that identify a document, e.g. [\"email\"].",
            ))
        }
    };
    if keys.is_empty() {
        return Err(invalid("keys is empty."));
    }
    let docs: Vec<Document> = match json_of(args, "documents")? {
        Some(Value::Array(docs)) => docs
            .into_iter()
            .map(json_to_document)
            .collect::<AppResult<_>>()?,
        Some(doc @ Value::Object(_)) => vec![json_to_document(doc)?],
        _ => return Err(invalid("documents is required: an array of documents.")),
    };
    if docs.is_empty() {
        return Err(invalid("documents is empty."));
    }
    if docs.len() > MAX_UPSERT {
        return Err(invalid(format!(
            "At most {MAX_UPSERT} documents per call; split them."
        )));
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (i, doc) in docs.iter().enumerate() {
        if let Some(missing) = keys.iter().find(|k| get_path(doc, k).is_none()) {
            return Err(invalid(format!(
                "Document #{i} has no {missing}: every document needs all the keys."
            )));
        }
        let tuple = tuple_of(doc, &keys);
        if let Some(first) = seen.insert(tuple, i) {
            return Err(invalid(format!(
                "Documents #{first} and #{i} have the same keys: send each once."
            )));
        }
    }
    Ok(Upsert {
        keys,
        docs,
        only_insert: bool_arg(args, "only_insert"),
    })
}

impl Upsert {
    fn filter(&self, doc: &Document) -> Document {
        let mut filter = Document::new();
        for key in &self.keys {
            if let Some(value) = get_path(doc, key) {
                filter.insert(key.clone(), value.clone());
            }
        }
        filter
    }

    /// One updateOne-with-upsert per document: `$setOnInsert` the whole
    /// document when only inserting, else `$set` its fields (its `_id`
    /// only on insert, since an existing one can't change).
    fn update(&self, doc: &Document) -> Document {
        if self.only_insert {
            return doc! { "$setOnInsert": doc.clone() };
        }
        let mut set = doc.clone();
        let mut update = Document::new();
        if let Some(id) = set.remove("_id") {
            update.insert("$setOnInsert", doc! { "_id": id });
        }
        if !set.is_empty() {
            update.insert("$set", set);
        }
        update
    }

    pub fn ops(&self) -> Vec<WriteOp> {
        self.docs
            .iter()
            .map(|doc| WriteOp::Update {
                filter: self.filter(doc),
                update: Bson::Document(self.update(doc)),
                multi: false,
                upsert: true,
                extra: Document::new(),
            })
            .collect()
    }

    /// The bulkWrite mongosh would run, for the approval card.
    pub fn code(&self) -> String {
        let ops: Vec<Value> = self
            .docs
            .iter()
            .map(|doc| {
                serde_json::json!({ "updateOne": {
                    "filter": Bson::Document(self.filter(doc)).into_relaxed_extjson(),
                    "update": Bson::Document(self.update(doc)).into_relaxed_extjson(),
                    "upsert": true,
                } })
            })
            .collect();
        format!("bulkWrite({}, {{\"ordered\":false}})", Value::Array(ops))
    }

    /// How many documents already exist (by their keys).
    pub async fn existing(&self, client: &Client, db: &str, coll: &str) -> AppResult<usize> {
        let tuples = existing_tuples(client, db, coll, &self.keys, &self.docs, false).await?;
        Ok(self
            .docs
            .iter()
            .filter(|d| tuples.contains(&tuple_of(d, &self.keys)))
            .count())
    }

    pub fn dry_run_text(&self, existing: usize) -> String {
        let new = self.docs.len() - existing;
        let inserted = format!(
            "{} would be inserted",
            plural(new as u64, "document", "documents")
        );
        if existing == 0 {
            return inserted;
        }
        let what = if self.only_insert {
            "would be left as they are (only_insert)"
        } else {
            "would be updated"
        };
        format!(
            "{inserted}; {} already {} and {what}",
            plural(existing as u64, "document", "documents"),
            if existing == 1 { "exists" } else { "exist" }
        )
    }
}

/// The value at a dotted path.
fn get_path<'a>(doc: &'a Document, path: &str) -> Option<&'a Bson> {
    let mut parts = path.split('.');
    let mut value = doc.get(parts.next()?)?;
    for part in parts {
        value = value.as_document()?.get(part)?;
    }
    Some(value)
}

/// A comparable text of the values at `keys`. Numbers compare by value,
/// as the server does (5, 5.0 and NumberLong(5) are the same key).
fn tuple_of(doc: &Document, keys: &[String]) -> String {
    let values: Vec<Value> = keys
        .iter()
        .map(|k| match get_path(doc, k) {
            None | Some(Bson::Null) => Value::Null,
            Some(Bson::Int32(n)) => Value::String(format!("#{}", *n as f64)),
            Some(Bson::Int64(n)) => Value::String(format!("#{}", *n as f64)),
            Some(Bson::Double(n)) => Value::String(format!("#{n}")),
            Some(other) => other.clone().into_canonical_extjson(),
        })
        .collect();
    Value::Array(values).to_string()
}

/// The key tuples of `docs` that some document in the collection has.
/// `sparse` leaves out documents that have none of the keys.
async fn existing_tuples(
    client: &Client,
    db: &str,
    coll: &str,
    keys: &[String],
    docs: &[Document],
    sparse: bool,
) -> AppResult<HashSet<String>> {
    let c = client.database(db).collection::<Document>(coll);
    let mut projection = doc! { "_id": 0 };
    for key in keys {
        projection.insert(key.clone(), 1);
    }
    let filters: Vec<Bson> = docs
        .iter()
        .filter(|d| !sparse || keys.iter().any(|k| get_path(d, k).is_some()))
        .map(|d| {
            let mut filter = Document::new();
            for key in keys {
                filter.insert(key.clone(), get_path(d, key).cloned().unwrap_or(Bson::Null));
            }
            Bson::Document(filter)
        })
        .collect();
    let mut found = HashSet::new();
    for chunk in filters.chunks(LOOKUP_CHUNK) {
        let mut options = FindOptions::default();
        options.projection = Some(projection.clone());
        let docs: Vec<Document> = c
            .find(doc! { "$or": chunk.to_vec() })
            .with_options(options)
            .await?
            .try_collect()
            .await?;
        found.extend(docs.iter().map(|d| tuple_of(d, keys)));
    }
    Ok(found)
}

/// What would stop an insert: an `_id` sent twice or already taken, and
/// values a unique index already holds. Partial unique indexes aren't
/// checked.
pub(super) async fn insert_problems(
    client: &Client,
    db: &str,
    coll: &str,
    docs: &[Document],
) -> AppResult<Vec<String>> {
    let mut problems = Vec::new();
    let mut indexes: Vec<(String, Vec<String>, bool)> =
        vec![("_id".to_string(), vec!["_id".to_string()], false)];
    let models: Vec<_> = client
        .database(db)
        .collection::<Document>(coll)
        .list_indexes()
        .await?
        .try_collect()
        .await
        .unwrap_or_default();
    let mut unchecked = Vec::new();
    for model in models {
        let Some(options) = model.options else {
            continue;
        };
        if options.unique != Some(true) {
            continue;
        }
        let name = options.name.clone().unwrap_or_default();
        if options.partial_filter_expression.is_some() {
            unchecked.push(name);
            continue;
        }
        let keys: Vec<String> = model.keys.keys().cloned().collect();
        indexes.push((name, keys, options.sparse == Some(true)));
    }
    for (name, keys, sparse) in indexes {
        let ids_only = name == "_id";
        let relevant: Vec<(usize, &Document)> = docs
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                if ids_only {
                    d.contains_key("_id")
                } else {
                    !sparse || keys.iter().any(|k| get_path(d, k).is_some())
                }
            })
            .collect();
        if relevant.is_empty() {
            continue;
        }
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut twice = Vec::new();
        for (i, d) in &relevant {
            if seen.insert(tuple_of(d, &keys), *i).is_some() {
                twice.push(*i);
            }
        }
        let wanted: Vec<Document> = relevant.iter().map(|(_, d)| (*d).clone()).collect();
        let taken = existing_tuples(client, db, coll, &keys, &wanted, sparse).await?;
        let clashing: Vec<usize> = relevant
            .iter()
            .filter(|(_, d)| taken.contains(&tuple_of(d, &keys)))
            .map(|(i, _)| *i)
            .collect();
        let what = if ids_only {
            "an _id".to_string()
        } else {
            format!("a value of the unique index {name} ({})", keys.join(", "))
        };
        if !clashing.is_empty() {
            problems.push(format!(
                "{} {} {what} that the collection already has: {}",
                plural(clashing.len() as u64, "document", "documents"),
                if clashing.len() == 1 { "has" } else { "have" },
                numbers(&clashing)
            ));
        }
        if !twice.is_empty() {
            problems.push(format!(
                "{} {} {what} of an earlier document in the list: {}",
                plural(twice.len() as u64, "document", "documents"),
                if twice.len() == 1 {
                    "repeats"
                } else {
                    "repeat"
                },
                numbers(&twice)
            ));
        }
    }
    if !unchecked.is_empty() {
        problems.push(format!(
            "(the partial unique {} {} not checked)",
            if unchecked.len() == 1 {
                "index"
            } else {
                "indexes"
            },
            unchecked.join(", ")
        ));
    }
    Ok(problems)
}

/// `#3, #7, #12 and 4 more`.
fn numbers(indexes: &[usize]) -> String {
    let shown: Vec<String> = indexes
        .iter()
        .take(SHOWN)
        .map(|i| format!("#{i}"))
        .collect();
    match indexes.len().saturating_sub(SHOWN) {
        0 => shown.join(", "),
        more => format!("{} and {more} more", shown.join(", ")),
    }
}

/// Whether the call asks for a dry run of a document write.
pub(super) fn is_dry_run(tool: Tool, args: &Value) -> bool {
    matches!(
        tool,
        Tool::InsertDocuments
            | Tool::UpdateDocuments
            | Tool::DeleteDocuments
            | Tool::UpsertDocuments
    ) && bool_arg(args, "dry_run")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn upserts_set_fields_or_only_insert() {
        let upsert = upsert_args(&json!({
            "keys": ["subDomain", "product"],
            "documents": [{ "_id": 1, "subDomain": "acme", "product": "chat", "credits": 5 }],
        }))
        .unwrap();
        assert!(!upsert.only_insert);
        let doc = &upsert.docs[0];
        assert_eq!(
            upsert.filter(doc),
            doc! { "subDomain": "acme", "product": "chat" }
        );
        assert_eq!(
            upsert.update(doc),
            doc! { "$setOnInsert": { "_id": 1 }, "$set": { "subDomain": "acme", "product": "chat", "credits": 5 } }
        );
        let only = upsert_args(
            &json!({ "keys": "email", "documents": [{ "email": "a@x" }], "only_insert": "true" }),
        )
        .unwrap();
        assert_eq!(
            only.update(&only.docs[0]),
            doc! { "$setOnInsert": { "email": "a@x" } }
        );
        assert!(only.code().contains("\"upsert\":true"));
    }

    #[test]
    fn upserts_need_every_key_once() {
        let missing = upsert_args(&json!({ "keys": ["a", "b"], "documents": [{ "a": 1 }] }));
        assert!(missing.unwrap_err().to_string().contains("no b"));
        let twice = upsert_args(&json!({ "keys": ["a"], "documents": [{ "a": 1 }, { "a": 1.0 }] }));
        assert!(twice.unwrap_err().to_string().contains("#0 and #1"));
        let nested =
            upsert_args(&json!({ "keys": ["owner.id"], "documents": [{ "owner": { "id": 7 } }] }))
                .unwrap();
        assert_eq!(nested.filter(&nested.docs[0]), doc! { "owner.id": 7 });
    }

    #[test]
    fn dry_run_summaries_say_what_happens_to_existing_documents() {
        let upsert = upsert_args(
            &json!({ "keys": ["a"], "documents": [{ "a": 1 }, { "a": 2 }, { "a": 3 }] }),
        )
        .unwrap();
        assert_eq!(upsert.dry_run_text(0), "3 documents would be inserted");
        assert_eq!(
            upsert.dry_run_text(1),
            "2 documents would be inserted; 1 document already exists and would be updated"
        );
        assert_eq!(
            numbers(&(0..12).collect::<Vec<_>>()),
            "#0, #1, #2, #3, #4, #5, #6, #7, #8, #9 and 2 more"
        );
    }
}

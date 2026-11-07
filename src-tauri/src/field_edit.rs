//! Edits that change a document's shape rather than one scalar value:
//! removing a field or an array item, renaming a field, inserting into an
//! array, and reading a field back exactly as stored.
//!
//! The frontend holds documents as relaxed Extended JSON, where an Int32, a
//! small Int64 and the Double `5.0` all read as `5`. So nothing here writes
//! back data the frontend sent unless the user typed it: an array that loses
//! an item, or an object whose field is renamed inside an array, is rebuilt
//! from the BSON read from the server, and written with a filter that holds
//! that same BSON. If the document changed in between, nothing is written
//! and the user is told to run the query again.

use mongodb::bson::{doc, Bson, Document};
use mongodb::options::ReturnDocument;
use mongodb::{Client, Collection};
use serde::{Deserialize, Serialize};

use crate::driver::field_path;
use crate::ejson::{document_to_json, json_to_document};
use crate::error::{AppError, AppResult};

/// One change to a document's shape. `path` is the field's segments from
/// the document root; array items are their index, e.g. `["items", "2"]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FieldEdit {
    /// Removes a field, or an item of an array (the items after it move up).
    /// `expected` is the value as the user saw it (relaxed Extended JSON):
    /// if the field now holds something else - another item moved into that
    /// index, say - nothing is removed.
    Unset {
        path: Vec<String>,
        expected: Option<serde_json::Value>,
    },
    /// Gives a field another name in the same object.
    Rename { path: Vec<String>, to: String },
    /// Adds `value` to the array at `path`: at `position`, or at the end.
    Insert {
        path: Vec<String>,
        value: serde_json::Value,
        position: Option<u32>,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldEditResult {
    /// The document as stored after the edit (relaxed Extended JSON).
    pub document: serde_json::Value,
    /// What the edit removed, as canonical Extended JSON, so an undo can
    /// write back exactly the types that were there.
    pub previous: Option<serde_json::Value>,
}

const CHANGED: &str = "The document changed since it was loaded. Run the query again and retry.";

/// Whether a stored value is still the one the user saw. Both sides are
/// relaxed Extended JSON; numbers compare by value, since the frontend sends
/// the Double 5.0 back as 5.
pub(crate) fn same_as_seen(stored: &Bson, seen: &serde_json::Value) -> bool {
    fn eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
        use serde_json::Value::*;
        match (a, b) {
            (Number(x), Number(y)) => x == y || x.as_f64() == y.as_f64(),
            (Array(x), Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| eq(a, b)),
            (Object(x), Object(y)) => {
                x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| eq(v, w)))
            }
            _ => a == b,
        }
    }
    eq(&stored.clone().into_relaxed_extjson(), seen)
}

fn gone() -> AppError {
    AppError::NotFound("the document no longer exists in the collection".to_string())
}

fn field_gone(path: &[String]) -> AppError {
    AppError::Conflict(format!(
        "{} is no longer in the document. Run the query again and retry.",
        path.join(".")
    ))
}

/// The value at `path`, following array items by index.
pub(crate) fn value_at<'a>(doc: &'a Document, path: &[String]) -> Option<&'a Bson> {
    let (first, rest) = path.split_first()?;
    let mut current = doc.get(first)?;
    for segment in rest {
        current = match current {
            Bson::Document(d) => d.get(segment)?,
            Bson::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// Whether any field holding the one at `path` is an array: `$rename`
/// can't reach a field inside one.
pub(crate) fn crosses_array(doc: &Document, path: &[String]) -> bool {
    (1..path.len()).any(|n| matches!(value_at(doc, &path[..n]), Some(Bson::Array(_))))
}

/// A field name `$set` and `$rename` take as one plain field.
pub(crate) fn check_field_name(name: &str) -> AppResult<()> {
    if name.is_empty() {
        return Err(AppError::InvalidInput("a field needs a name".to_string()));
    }
    if name.contains('.') || name.starts_with('$') {
        return Err(AppError::InvalidInput(format!(
            "{name:?}: a field name can't contain a dot or start with $"
        )));
    }
    Ok(())
}

/// Removes item `index` of `items`, as a new array.
pub(crate) fn without_item(items: &[Bson], index: usize) -> AppResult<Vec<Bson>> {
    if index >= items.len() {
        return Err(AppError::Conflict(CHANGED.to_string()));
    }
    let mut next = items.to_vec();
    next.remove(index);
    Ok(next)
}

/// `holder` with its field `from` called `to`, in the same place.
pub(crate) fn renamed(holder: &Document, from: &str, to: &str) -> Document {
    holder
        .iter()
        .map(|(k, v)| {
            (
                if k == from { to.to_string() } else { k.clone() },
                v.clone(),
            )
        })
        .collect()
}

async fn load(coll: &Collection<Document>, id_filter: &Document) -> AppResult<Document> {
    coll.find_one(id_filter.clone()).await?.ok_or_else(gone)
}

/// Runs `update` on the document `filter` matches and returns it as stored
/// afterwards; no match means it changed (or went) since it was read.
async fn apply(
    coll: &Collection<Document>,
    id_filter: &Document,
    filter: Document,
    update: Document,
    conflict: &str,
) -> AppResult<Document> {
    if let Some(updated) = coll
        .find_one_and_update(filter, update)
        .return_document(ReturnDocument::After)
        .await?
    {
        return Ok(updated);
    }
    // tell a deleted document from one that no longer matches
    load(coll, id_filter).await?;
    Err(AppError::Conflict(conflict.to_string()))
}

fn with(mut filter: Document, key: String, value: Bson) -> Document {
    filter.insert(key, doc! { "$eq": value });
    filter
}

/// The value at `path` in the document with this `_id`, as canonical
/// Extended JSON: every number keeps its BSON type, for an editor that
/// writes the value back.
pub async fn read_field(
    client: &Client,
    db: &str,
    collection: &str,
    id: serde_json::Value,
    path: &[String],
) -> AppResult<serde_json::Value> {
    let coll = client.database(db).collection::<Document>(collection);
    let id_filter = json_to_document(serde_json::json!({ "_id": id }))?;
    let current = load(&coll, &id_filter).await?;
    let value = value_at(&current, path).ok_or_else(|| field_gone(path))?;
    Ok(value.clone().into_canonical_extjson())
}

pub async fn edit_field(
    client: &Client,
    db: &str,
    collection: &str,
    id: serde_json::Value,
    edit: FieldEdit,
) -> AppResult<FieldEditResult> {
    let coll = client.database(db).collection::<Document>(collection);
    let id_filter = json_to_document(serde_json::json!({ "_id": id }))?;

    match edit {
        FieldEdit::Unset { path, expected } => {
            let joined = field_path(&path)?;
            let current = load(&coll, &id_filter).await?;
            let previous = value_at(&current, &path)
                .cloned()
                .ok_or_else(|| field_gone(&path))?;
            if expected.is_some_and(|seen| !same_as_seen(&previous, &seen)) {
                return Err(AppError::Conflict(CHANGED.to_string()));
            }
            let (holder_path, last) = path.split_at(path.len() - 1);
            let holder = value_at(&current, holder_path);
            let (filter, update) = match holder {
                // an array item: rebuild the array without it
                Some(Bson::Array(items)) if !holder_path.is_empty() => {
                    let index = last[0]
                        .parse::<usize>()
                        .map_err(|_| AppError::Conflict(CHANGED.to_string()))?;
                    let next = without_item(items, index)?;
                    let holder_joined = holder_path.join(".");
                    (
                        with(
                            id_filter.clone(),
                            holder_joined.clone(),
                            Bson::Array(items.clone()),
                        ),
                        doc! { "$set": { holder_joined: next } },
                    )
                }
                _ => (
                    with(id_filter.clone(), joined.clone(), previous.clone()),
                    doc! { "$unset": { joined: "" } },
                ),
            };
            let updated = apply(&coll, &id_filter, filter, update, CHANGED).await?;
            Ok(FieldEditResult {
                document: document_to_json(updated),
                previous: Some(previous.into_canonical_extjson()),
            })
        }

        FieldEdit::Rename { path, to } => {
            let joined = field_path(&path)?;
            check_field_name(&to)?;
            let current = load(&coll, &id_filter).await?;
            if value_at(&current, &path).is_none() {
                return Err(field_gone(&path));
            }
            let (holder_path, last) = path.split_at(path.len() - 1);
            let from = &last[0];
            if from == &to {
                return Ok(FieldEditResult {
                    document: document_to_json(current),
                    previous: None,
                });
            }
            let exists = format!("There is already a field named {to:?} here.");
            let updated = if crosses_array(&current, &path) {
                // $rename can't reach into an array: rewrite the object
                // holding the field, which keeps the field in its place
                let Some(Bson::Document(holder)) = value_at(&current, holder_path) else {
                    return Err(AppError::InvalidInput(
                        "an array item has no name to change".to_string(),
                    ));
                };
                if holder.contains_key(&to) {
                    return Err(AppError::Conflict(exists));
                }
                let holder_joined = holder_path.join(".");
                let next = renamed(holder, from, &to);
                apply(
                    &coll,
                    &id_filter,
                    with(
                        id_filter.clone(),
                        holder_joined.clone(),
                        Bson::Document(holder.clone()),
                    ),
                    doc! { "$set": { holder_joined: next } },
                    CHANGED,
                )
                .await?
            } else {
                if !holder_path.is_empty()
                    && !matches!(value_at(&current, holder_path), Some(Bson::Document(_)))
                {
                    return Err(AppError::InvalidInput(
                        "an array item has no name to change".to_string(),
                    ));
                }
                let target: Vec<String> = holder_path
                    .iter()
                    .cloned()
                    .chain(std::iter::once(to.clone()))
                    .collect();
                let target = target.join(".");
                // $rename would silently overwrite a field already there
                let mut filter = id_filter.clone();
                filter.insert(joined.clone(), doc! { "$exists": true });
                filter.insert(target.clone(), doc! { "$exists": false });
                apply(
                    &coll,
                    &id_filter,
                    filter,
                    doc! { "$rename": { joined: target } },
                    &exists,
                )
                .await?
            };
            Ok(FieldEditResult {
                document: document_to_json(updated),
                previous: None,
            })
        }

        FieldEdit::Insert {
            path,
            value,
            position,
        } => {
            let joined = field_path(&path)?;
            let value = json_to_document(serde_json::json!({ "value": value }))?
                .remove("value")
                .ok_or_else(|| AppError::InvalidInput("missing value".to_string()))?;
            let mut each = doc! { "$each": [value] };
            if let Some(position) = position {
                each.insert("$position", i64::from(position));
            }
            let mut filter = id_filter.clone();
            filter.insert(joined.clone(), doc! { "$type": "array" });
            let updated = apply(
                &coll,
                &id_filter,
                filter,
                doc! { "$push": { joined: each } },
                &format!(
                    "{} is no longer an array. Run the query again and retry.",
                    path.join(".")
                ),
            )
            .await?;
            Ok(FieldEditResult {
                document: document_to_json(updated),
                previous: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<String> {
        s.split('.').map(str::to_string).collect()
    }

    fn sample() -> Document {
        doc! {
            "_id": 1,
            "items": [ { "sku": "A", "qty": 1 }, { "sku": "B", "tags": ["x", "y"] } ],
            "shipping": { "city": "Recife", "state": "PE" },
            "n": 5i64,
        }
    }

    #[test]
    fn value_at_follows_objects_and_array_items() {
        let d = sample();
        assert_eq!(
            value_at(&d, &p("shipping.city")),
            Some(&Bson::String("Recife".into()))
        );
        assert_eq!(
            value_at(&d, &p("items.1.tags.0")),
            Some(&Bson::String("x".into()))
        );
        assert_eq!(value_at(&d, &p("n")), Some(&Bson::Int64(5)));
        assert_eq!(value_at(&d, &p("items.9")), None);
        assert_eq!(value_at(&d, &p("items.x")), None);
        assert_eq!(value_at(&d, &p("n.x")), None);
        assert_eq!(value_at(&d, &[]), None);
    }

    #[test]
    fn crosses_array_only_below_an_array() {
        let d = sample();
        assert!(crosses_array(&d, &p("items.0.sku")));
        assert!(crosses_array(&d, &p("items.0")));
        assert!(!crosses_array(&d, &p("items")));
        assert!(!crosses_array(&d, &p("shipping.city")));
    }

    #[test]
    fn field_names_reject_dots_dollars_and_empty() {
        assert!(check_field_name("note").is_ok());
        assert!(check_field_name("").is_err());
        assert!(check_field_name("a.b").is_err());
        assert!(check_field_name("$set").is_err());
        assert!(check_field_name("price$").is_ok());
    }

    #[test]
    fn without_item_removes_by_index_and_checks_bounds() {
        let items = vec![Bson::Int32(1), Bson::Int32(2), Bson::Int32(3)];
        assert_eq!(
            without_item(&items, 1).unwrap(),
            vec![Bson::Int32(1), Bson::Int32(3)]
        );
        assert!(without_item(&items, 3).is_err());
    }

    #[test]
    fn renamed_keeps_the_field_in_place() {
        let holder = doc! { "a": 1, "b": 2, "c": 3 };
        let next = renamed(&holder, "b", "z");
        let keys: Vec<_> = next.keys().cloned().collect();
        assert_eq!(keys, vec!["a", "z", "c"]);
        assert_eq!(next.get_i32("z").unwrap(), 2);
    }

    #[test]
    fn same_as_seen_compares_numbers_by_value() {
        let stored =
            Bson::Document(doc! { "price": 5.0f64, "qty": 2i32, "n": 7i64, "tags": ["x"] });
        let seen = serde_json::json!({ "qty": 2, "price": 5, "n": 7, "tags": ["x"] });
        assert!(same_as_seen(&stored, &seen));
        assert!(!same_as_seen(
            &stored,
            &serde_json::json!({ "qty": 3, "price": 5, "n": 7, "tags": ["x"] })
        ));
        assert!(!same_as_seen(
            &stored,
            &serde_json::json!({ "qty": 2, "price": 5, "n": 7 })
        ));
        assert!(!same_as_seen(
            &Bson::String("a".into()),
            &serde_json::json!("b")
        ));
    }

    #[test]
    fn edits_deserialize_from_the_frontend_shape() {
        let edit: FieldEdit = serde_json::from_value(serde_json::json!({
            "kind": "insert", "path": ["items"], "value": { "sku": "C" }, "position": 1
        }))
        .unwrap();
        assert!(matches!(
            edit,
            FieldEdit::Insert {
                position: Some(1),
                ..
            }
        ));
        let edit: FieldEdit = serde_json::from_value(serde_json::json!({
            "kind": "rename", "path": ["notes"], "to": "note"
        }))
        .unwrap();
        assert!(matches!(edit, FieldEdit::Rename { .. }));
    }
}

/// Against a real server. Skipped by default; run with:
///   MONGO_STUDIO_TEST_URI=mongodb://127.0.0.1:27017 cargo test --lib -- --ignored live_field_edits
#[cfg(test)]
mod live_tests {
    use super::*;
    use mongodb::bson::{spec::BinarySubtype, Binary};

    fn p(s: &str) -> Vec<String> {
        s.split('.').map(str::to_string).collect()
    }

    #[tokio::test]
    #[ignore]
    async fn live_field_edits() {
        let uri = std::env::var("MONGO_STUDIO_TEST_URI").expect("set MONGO_STUDIO_TEST_URI");
        let client = Client::with_uri_str(&uri).await.unwrap();
        let db = "mongo_studio_test";
        let name = format!("field_edit_{}", uuid::Uuid::new_v4().simple());
        let coll = client.database(db).collection::<Document>(&name);
        let oid = mongodb::bson::oid::ObjectId::new();
        let uuid = Binary {
            subtype: BinarySubtype::Uuid,
            bytes: uuid::Uuid::new_v4().as_bytes().to_vec(),
        };
        let original = doc! {
            "_id": oid,
            "items": [
                { "sku": "A", "price": 5.0f64, "qty": 2i32, "big": 7i64 },
                { "sku": "B", "price": 2.5f64, "ref": uuid.clone() },
            ],
            "n": 5i64,
            "ratio": 3.0f64,
            "shipping": { "city": "Recife", "state": "PE" },
            "notes": "x",
        };
        coll.insert_one(original.clone()).await.unwrap();
        let id = serde_json::json!({ "$oid": oid.to_hex() });
        let stored = || async { coll.find_one(doc! { "_id": oid }).await.unwrap().unwrap() };

        // read_field keeps every number's type
        let read = read_field(&client, db, &name, id.clone(), &p("items.0"))
            .await
            .unwrap();
        assert_eq!(read["price"], serde_json::json!({ "$numberDouble": "5.0" }));
        assert_eq!(read["big"], serde_json::json!({ "$numberLong": "7" }));

        // removing an array item rebuilds the array from the server's BSON
        let r = edit_field(
            &client,
            db,
            &name,
            id.clone(),
            FieldEdit::Unset {
                expected: None,
                path: p("items.0"),
            },
        )
        .await
        .unwrap();
        let after = stored().await;
        let items = after.get_array("items").unwrap();
        assert_eq!(items.len(), 1);
        let left = items[0].as_document().unwrap();
        assert_eq!(left.get("price"), Some(&Bson::Double(2.5)));
        assert_eq!(left.get("ref"), Some(&Bson::Binary(uuid.clone())));
        // ... and undoing it puts the same BSON back in the same place
        let previous = r.previous.unwrap();
        edit_field(
            &client,
            db,
            &name,
            id.clone(),
            FieldEdit::Insert {
                path: p("items"),
                value: previous,
                position: Some(0),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            stored().await.get("items"),
            original.get("items"),
            "types and order restored"
        );

        // removing a field returns it canonically; writing it back keeps Int64
        let r = edit_field(
            &client,
            db,
            &name,
            id.clone(),
            FieldEdit::Unset {
                expected: None,
                path: p("n"),
            },
        )
        .await
        .unwrap();
        assert!(stored().await.get("n").is_none());
        assert_eq!(
            r.previous.clone().unwrap(),
            serde_json::json!({ "$numberLong": "5" })
        );
        crate::driver::update_field(&client, db, &name, id.clone(), &p("n"), r.previous.unwrap())
            .await
            .unwrap();
        assert_eq!(stored().await.get("n"), Some(&Bson::Int64(5)));

        // renaming a plain field moves it, as $rename does, and won't clobber
        edit_field(
            &client,
            db,
            &name,
            id.clone(),
            FieldEdit::Rename {
                path: p("shipping.city"),
                to: "town".into(),
            },
        )
        .await
        .unwrap();
        let shipping = stored().await.get_document("shipping").unwrap().clone();
        assert_eq!(
            shipping.keys().cloned().collect::<Vec<_>>(),
            vec!["state", "town"]
        );
        let clash = edit_field(
            &client,
            db,
            &name,
            id.clone(),
            FieldEdit::Rename {
                path: p("shipping.state"),
                to: "town".into(),
            },
        )
        .await;
        assert!(matches!(clash, Err(AppError::Conflict(_))), "{clash:?}");
        assert_eq!(
            stored().await.get_document("shipping").unwrap(),
            &shipping,
            "nothing written"
        );

        // renaming inside an array item rewrites the item, in place
        edit_field(
            &client,
            db,
            &name,
            id.clone(),
            FieldEdit::Rename {
                path: p("items.1.sku"),
                to: "code".into(),
            },
        )
        .await
        .unwrap();
        let item = stored().await.get_array("items").unwrap()[1]
            .as_document()
            .unwrap()
            .clone();
        assert_eq!(
            item.keys().cloned().collect::<Vec<_>>(),
            vec!["code", "price", "ref"]
        );
        assert_eq!(item.get("ref"), Some(&Bson::Binary(uuid)));

        // an item index past the end means the array changed: nothing written
        let stale = edit_field(
            &client,
            db,
            &name,
            id.clone(),
            FieldEdit::Unset {
                expected: None,
                path: p("items.5"),
            },
        )
        .await;
        assert!(stale.is_err());
        // an item moved into the index the user removes isn't removed
        let seen = stored().await.get_array("items").unwrap()[0]
            .clone()
            .into_relaxed_extjson();
        coll.update_one(
            doc! { "_id": oid },
            doc! { "$push": { "items": { "$each": ["front"], "$position": 0 } } },
        )
        .await
        .unwrap();
        let moved = edit_field(
            &client,
            db,
            &name,
            id.clone(),
            FieldEdit::Unset {
                path: p("items.0"),
                expected: Some(seen.clone()),
            },
        )
        .await;
        assert!(matches!(moved, Err(AppError::Conflict(_))), "{moved:?}");
        assert_eq!(
            stored().await.get_array("items").unwrap()[0],
            Bson::String("front".into())
        );
        // ... while the item the user saw, where they saw it, goes
        edit_field(
            &client,
            db,
            &name,
            id.clone(),
            FieldEdit::Unset {
                path: p("items.1"),
                expected: Some(seen),
            },
        )
        .await
        .unwrap();
        assert_eq!(stored().await.get_array("items").unwrap().len(), 2);
        // a field changed by someone else in between isn't removed
        let before = stored().await;
        coll.update_one(doc! { "_id": oid }, doc! { "$set": { "notes": "changed" } })
            .await
            .unwrap();
        // simulate the stale read: the filter holds the old value
        let raced = apply(
            &coll,
            &doc! { "_id": oid },
            with(
                doc! { "_id": oid },
                "notes".into(),
                before.get("notes").unwrap().clone(),
            ),
            doc! { "$unset": { "notes": "" } },
            CHANGED,
        )
        .await;
        assert!(matches!(raced, Err(AppError::Conflict(_))));
        assert_eq!(stored().await.get_str("notes").unwrap(), "changed");

        // inserting into something that isn't an array is refused
        let wrong = edit_field(
            &client,
            db,
            &name,
            id.clone(),
            FieldEdit::Insert {
                path: p("ratio"),
                value: serde_json::json!(1),
                position: None,
            },
        )
        .await;
        assert!(matches!(wrong, Err(AppError::Conflict(_))));
        assert_eq!(stored().await.get("ratio"), Some(&Bson::Double(3.0)));

        // deleting by _id
        let other = mongodb::bson::oid::ObjectId::new();
        coll.insert_one(doc! { "_id": other }).await.unwrap();
        let n = crate::driver::delete_documents(
            &client,
            db,
            &name,
            vec![
                id,
                serde_json::json!({ "$oid": other.to_hex() }),
                serde_json::json!({ "$oid": oid.to_hex() }),
            ],
        )
        .await
        .unwrap();
        assert_eq!(n, 2);
        assert_eq!(coll.count_documents(doc! {}).await.unwrap(), 0);
        coll.drop().await.unwrap();
    }
}

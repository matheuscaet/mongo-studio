use bson::{Bson, Document};
use serde_json::Value;

use crate::error::{AppError, AppResult};

/// Converts any BSON value to relaxed Extended JSON, e.g. `ObjectId` becomes
/// `{"$oid": "..."}` and `DateTime` becomes `{"$date": "..."}`, so type
/// fidelity survives the trip to the frontend instead of silently degrading
/// to plain JSON strings/numbers.
pub fn bson_to_json(bson: Bson) -> Value {
    bson.into_relaxed_extjson()
}

/// Convenience wrapper of [`bson_to_json`] for a whole document.
pub fn document_to_json(doc: Document) -> Value {
    bson_to_json(Bson::Document(doc))
}

/// Parses a JSON object (optionally containing Extended JSON type tags) back
/// into a BSON document, e.g. for a user-submitted query filter.
pub fn json_to_document(value: Value) -> AppResult<Document> {
    match value {
        Value::Null => Ok(Document::new()),
        Value::Object(map) => Document::try_from(map)
            .map_err(|e| AppError::InvalidInput(format!("invalid document: {e}"))),
        _ => Err(AppError::InvalidInput("expected a JSON object".to_string())),
    }
}

/// Parses a JSON array of pipeline stages into BSON documents.
pub fn json_to_pipeline(value: Value) -> AppResult<Vec<Document>> {
    match value {
        Value::Array(items) => items.into_iter().map(json_to_document).collect(),
        _ => Err(AppError::InvalidInput(
            "expected a JSON array of pipeline stages".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bson::oid::ObjectId;
    use bson::{doc, DateTime};

    #[test]
    fn round_trips_object_id_and_date() {
        let oid = ObjectId::new();
        let date = DateTime::now();
        let original = doc! { "_id": oid, "createdAt": date, "count": 3i32 };

        let json = document_to_json(original.clone());
        assert_eq!(json["_id"]["$oid"], oid.to_hex());

        let back = json_to_document(json).unwrap();
        assert_eq!(back.get_object_id("_id").unwrap(), oid);
        assert_eq!(back.get_i32("count").unwrap(), 3);
    }

    /// Every shape the query fields' parser (src/lib/queryText.ts) turns
    /// mongosh helpers into must read back as its BSON type here.
    #[test]
    fn reads_the_shapes_the_query_parser_emits() {
        let value = serde_json::json!({
            "oid": { "$oid": "65f0c3a2e4b0a1b2c3d4e5f6" },
            "date": { "$date": "2026-01-31T14:30:00.000Z" },
            "oldDate": { "$date": { "$numberLong": "-1000" } },
            "long": { "$numberLong": "9007199254740993" },
            "int": { "$numberInt": "5" },
            "decimal": { "$numberDecimal": "10.25" },
            "uuid": { "$uuid": "3b241101-e2bb-4255-8caf-4136c566a962" },
            "regex": { "$regularExpression": { "pattern": "^ac", "options": "i" } },
            "ts": { "$timestamp": { "t": 1700000000, "i": 3 } },
            "bin": { "$binary": { "base64": "AAECAwQFBgcICQoLDA0ODw==", "subType": "04" } },
            "min": { "$minKey": 1 },
            "max": { "$maxKey": 1 },
            "nan": { "$numberDouble": "NaN" },
            "inf": { "$numberDouble": "-Infinity" },
        });
        let doc = json_to_document(value).unwrap();
        use bson::spec::{BinarySubtype, ElementType};
        let types: Vec<ElementType> = doc.values().map(Bson::element_type).collect();
        assert_eq!(
            types,
            vec![
                ElementType::ObjectId,
                ElementType::DateTime,
                ElementType::DateTime,
                ElementType::Int64,
                ElementType::Int32,
                ElementType::Decimal128,
                ElementType::Binary,
                ElementType::RegularExpression,
                ElementType::Timestamp,
                ElementType::Binary,
                ElementType::MinKey,
                ElementType::MaxKey,
                ElementType::Double,
                ElementType::Double,
            ]
        );
        assert_eq!(
            doc.get_datetime("oldDate").unwrap().timestamp_millis(),
            -1000
        );
        assert_eq!(doc.get_i64("long").unwrap(), 9_007_199_254_740_993);
        let Some(Bson::Binary(uuid)) = doc.get("uuid") else {
            panic!("uuid")
        };
        assert_eq!(uuid.subtype, BinarySubtype::Uuid);
        let Some(Bson::RegularExpression(regex)) = doc.get("regex") else {
            panic!("regex")
        };
        assert_eq!(
            (regex.pattern.as_str(), regex.options.as_str()),
            ("^ac", "i")
        );
    }

    /// The result views read a UUID from this shape (src/lib/bsonValue.ts:
    /// uuidText), and a `UUID("…")` filter must find the same value.
    #[test]
    fn sends_a_uuid_as_a_subtype_4_binary() {
        let uuid = bson::Uuid::parse_str("18e8acbb-14b3-416d-b074-dcd59b0587df").unwrap();
        let json = document_to_json(doc! { "key": uuid });
        assert_eq!(
            json["key"],
            serde_json::json!({ "$binary": { "base64": "GOisuxSzQW2wdNzVmwWH3w==", "subType": "04" } })
        );
        let filter = json_to_document(
            serde_json::json!({ "key": { "$uuid": "18e8acbb-14b3-416d-b074-dcd59b0587df" } }),
        )
        .unwrap();
        assert_eq!(filter.get("key"), Some(&Bson::from(uuid)));
    }

    #[test]
    fn rejects_non_object_filter() {
        let err = json_to_document(Value::String("nope".to_string()));
        assert!(err.is_err());
    }
}

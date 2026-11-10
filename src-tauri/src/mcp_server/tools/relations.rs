//! The relation tools: the user's saved links between collections - across
//! databases and connections - that clients may list, join through
//! (`find_related`), check, and propose as drafts the user reviews in the app.
//!
//! A relation tool can read two connections (the source and the other side,
//! or both sides of a suggestion); `prepare` resolves the other one and
//! checks it may be read before anything runs, like `policy::decide` does
//! for the connection the call is about.

use mongodb::Client;
use serde_json::{json, Map, Value};

use super::{
    bson_to_json, code_on, collection, invalid, js, json_lines, json_of, plural, str_arg, truncate,
    Call, Done, Plan, Tool, MAX_DOCS, MAX_OUTPUT, MAX_TIME,
};
use crate::error::{AppError, AppResult};
use crate::mcp_server::policy;
use crate::mcp_server::Level;
use crate::relations::bridge::{collection_for, placeholders, tenant_of};
use crate::relations::engine::{self, Source};
use crate::relations::sampling::{self, SampleSide};
use crate::relations::{self, pairing, Aggregate, Dir, Draft, Op, Relation};

/// Found documents shown per source document.
const DOCS_PER_SOURCE: usize = 5;
const DEFAULT_SOURCE_DOCS: i64 = 20;

/// The tool definitions, in `Tool::ALL` order.
pub(super) fn definitions() -> Vec<Value> {
    let relation_id =
        json!({ "type": "string", "description": "Id of a saved relation, from list_relations." });
    let connection_id = json!({ "type": "string", "description": "Id of a saved connection, from list_connections." });
    let side = |what: &str| {
        json!({
            "type": "object",
            "description": what,
            "properties": {
                "connection_id": connection_id,
                "database": { "type": "string" },
                "collection": { "type": "string", "description": "A collection name, or a pattern with {tenant} such as {tenant}_orders (a few matching collections are sampled)." },
            },
            "required": ["connection_id", "database", "collection"],
        })
    };
    let relation = json!({
        "type": "object",
        "description": "The relation, as get_relation shows it: name; from and to sides ({conn, db, coll}: from.conn is \"any\" or a connection id, to.conn is \"cluster\" (the connection to the same cluster holding to.db), \"same\" (the From connection) or a connection id; coll may hold {tenant}, e.g. {tenant}_orders); rules (alternative rules, tried in order per document; each a list of conditions that must all hold: {from, op, to, as} where from is a From field path or a template like {customerId}-{key}, op is eq, prefix (the To value starts with the From value) or in (the From field is an array), to is a To field path, as is as-stored, uuid-string, guid-string, oid-string or number-string); scope (filters always applied to the To side, e.g. [{\"to\": \"store\", \"value\": \"{tenant}\"}]); fromName (what the To documents are called from a From document, e.g. \"shipments\") and toName (what the From document is called from a To document, e.g. \"order\").",
    });
    let read_only =
        json!({ "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false });
    let writes = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false });
    let destructive =
        json!({ "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false });
    vec![
        json!({
            "name": "list_relations",
            "description": "List the relations saved in Mongo Studio: the user's links between collections in different databases or connections (MongoDB's $lookup can't cross databases, so Mongo Studio joins them itself). Each line has the id, name, From and To sides, a summary of the rules and whether it is a draft proposed by a client. Use find_related to join through one.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read_only,
        }),
        json!({
            "name": "get_relation",
            "description": "Show a saved relation in full (sides, rules, scope filters, names, connection pairs) as JSON.",
            "inputSchema": { "type": "object", "properties": { "relation_id": relation_id }, "required": ["relation_id"] },
            "annotations": read_only,
        }),
        json!({
            "name": "suggest_relation_links",
            "description": "Find how two collections link: Mongo Studio samples id-like fields on both sides (small samples, time limits) and returns candidate field pairs with how many sampled values matched and how to compare them (operator and conversion, e.g. UUID binary to text). Returns no document values. Use it before create_relation. Needs read access to both connections.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": side("The side holding the reference's source, e.g. the orders."),
                    "to": side("The side that points back, e.g. the shipments."),
                },
                "required": ["from", "to"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "create_relation",
            "description": "Propose a new relation. It is saved as a draft: the user sees it in Mongo Studio's Relations with your reason, and nothing uses it until they review and save it. Check it with test_relation after creating it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "relation": relation,
                    "why": { "type": "string", "description": "One or two sentences on why the sides link this way, shown to the user." },
                },
                "required": ["relation", "why"],
            },
            "annotations": writes,
        }),
        json!({
            "name": "update_relation",
            "description": "Change a draft relation proposed over MCP (relations the user saved can't be changed). Pass only the top-level fields that change (e.g. rules); they replace the draft's. It stays a draft for the user to review.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "relation_id": relation_id,
                    "relation": relation,
                    "why": { "type": "string", "description": "Why it changed, shown to the user." },
                },
                "required": ["relation_id", "relation"],
            },
            "annotations": writes,
        }),
        json!({
            "name": "test_relation",
            "description": "Check a relation on live data: samples From documents (a few tenants for {tenant} patterns), reports how many each rule matches and how many find nothing, the cardinality, and whether the indexes the rules need exist on the To side. connection_id is the From side's connection; the To connection is picked like the app does (same cluster).",
            "inputSchema": {
                "type": "object",
                "properties": { "relation_id": relation_id, "connection_id": connection_id },
                "required": ["relation_id", "connection_id"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "find_related",
            "description": "Join through a saved relation: reads documents on one side (filter, up to 100) and returns, per document, its _id, how many related documents it has on the other side, the aggregates you ask for (count, sum of a field, exists, first value of a field - each with an optional where) and up to 5 related documents. Answers questions like \"this customer placed 10 orders but 14 shipments went out\" in one call. side is the side the documents are read on: \"from\" (find their To documents) or \"to\" (find their From documents). Needs read access to both connections.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "relation_id": relation_id,
                    "side": { "type": "string", "enum": ["from", "to"], "default": "from" },
                    "connection_id": connection_id,
                    "collection": { "type": "string", "description": "The collection to read on that side; for a {tenant} pattern pass this or tenant." },
                    "tenant": { "type": "string", "description": "Fills the side's {tenant} pattern, e.g. berlin for {tenant}_orders." },
                    "filter": { "type": "object", "description": "Filter on the side's documents in MongoDB Extended JSON. Omit for the first documents." },
                    "aggregates": {
                        "type": "array",
                        "description": "Per document, over its related documents.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "Your key for the result." },
                                "kind": { "type": "string", "enum": ["count", "sum", "exists", "first"] },
                                "field": { "type": "string", "description": "For sum and first." },
                                "whereField": { "type": "string" },
                                "whereValue": { "description": "Only related documents whose whereField equals this." },
                            },
                            "required": ["id", "kind"],
                        },
                    },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_DOCS, "default": DEFAULT_SOURCE_DOCS, "description": "How many documents to read on the side." },
                },
                "required": ["relation_id", "connection_id"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "delete_relation",
            "description": "Delete a draft relation proposed over MCP. Relations the user saved can't be deleted this way.",
            "inputSchema": { "type": "object", "properties": { "relation_id": relation_id }, "required": ["relation_id"] },
            "annotations": destructive,
        }),
    ]
}

/// Whether the other connections a call reads may be read.
pub(super) enum Access {
    Granted,
    NoAccess,
}

fn relation_of(call: &Call<'_>, args: &Value) -> AppResult<Relation> {
    let id = str_arg(args, "relation_id").ok_or_else(|| invalid("relation_id is required."))?;
    call.inner
        .host
        .relations()
        .get(&id)
        .map_err(|_| AppError::NotFound("no relation has this id; call list_relations".to_string()))
}

fn readable(call: &Call<'_>, connection_id: &str) -> bool {
    policy::level_of(&call.inner.config(), connection_id) >= Level::Read
}

fn connection_exists(call: &Call<'_>, connection_id: &str) -> AppResult<()> {
    call.inner
        .state()
        .connection_store
        .get(connection_id)
        .map(|_| ())
        .map_err(|_| {
            AppError::NotFound(format!(
                "no saved connection has the id {connection_id}; call list_connections"
            ))
        })
}

/// One side of a suggestion.
struct SuggestSide {
    connection_id: String,
    side: SampleSide,
}

fn suggest_side(args: &Value, key: &str) -> AppResult<SuggestSide> {
    let side = json_of(args, key)?.ok_or_else(|| invalid(format!("{key} is required.")))?;
    let get = |name: &str| str_arg(&side, name);
    let connection_id = get("connection_id")
        .or_else(|| get("connection"))
        .ok_or_else(|| invalid(format!("{key}.connection_id is required.")))?;
    let database =
        get("database").ok_or_else(|| invalid(format!("{key}.database is required.")))?;
    let collection =
        get("collection").ok_or_else(|| invalid(format!("{key}.collection is required.")))?;
    Ok(SuggestSide {
        connection_id,
        side: SampleSide {
            database,
            collection,
        },
    })
}

/// A find_related call, resolved.
struct FindArgs {
    relation: Relation,
    dir: Dir,
    database: String,
    collection: String,
    target_connection: String,
    aggregates: Vec<Aggregate>,
    limit: i64,
}

fn find_args(call: &Call<'_>, args: &Value) -> AppResult<FindArgs> {
    let relation = relation_of(call, args)?;
    let dir = match str_arg(args, "side").as_deref() {
        None | Some("from") => Dir::From,
        Some("to") => Dir::To,
        Some(_) => return Err(invalid("side must be \"from\" or \"to\".")),
    };
    let side = match dir {
        Dir::From => &relation.from,
        Dir::To => &relation.to,
    };
    let collection = match (str_arg(args, "collection"), str_arg(args, "tenant")) {
        (Some(coll), _) => coll,
        (None, Some(tenant)) => collection_for(&side.coll, &tenant),
        (None, None) if !side.coll.contains("{tenant}") => side.coll.clone(),
        (None, None) => {
            return Err(invalid(format!(
                "The {} side is one collection per tenant ({}): pass collection or tenant.",
                if dir == Dir::From { "From" } else { "To" },
                side.coll
            )))
        }
    };
    if tenant_of(&side.coll, &collection).is_none() {
        return Err(invalid(format!(
            "{collection} isn't a collection of this side ({}).",
            side.coll
        )));
    }
    let source = call.target.connection_id.as_deref().unwrap_or_default();
    let target_connection =
        pairing::target_connection(&call.inner.state().connection_store, &relation, dir, source)
            .ok_or_else(|| {
                invalid(format!(
            "No saved connection holds the other side ({}) for this connection; the user can \
             pick one in the relation's Connections in Mongo Studio.",
            match dir {
                Dir::From => &relation.to.db,
                Dir::To => &relation.from.db,
            }
        ))
            })?;
    let aggregates = match json_of(args, "aggregates")? {
        None => Vec::new(),
        Some(value) => {
            serde_json::from_value(value).map_err(|e| invalid(format!("aggregates: {e}")))?
        }
    };
    Ok(FindArgs {
        database: side.db.clone(),
        collection,
        dir,
        target_connection,
        aggregates,
        limit: super::int_arg(args, "limit", DEFAULT_SOURCE_DOCS, MAX_DOCS),
        relation,
    })
}

/// The To connection of a check, from the From connection the call is about.
fn test_target(call: &Call<'_>, relation: &Relation) -> AppResult<String> {
    let source = call.target.connection_id.as_deref().unwrap_or_default();
    pairing::target_connection(
        &call.inner.state().connection_store,
        relation,
        Dir::From,
        source,
    )
    .ok_or_else(|| {
        invalid(format!(
            "No saved connection holds the To side ({}) for this connection.",
            relation.to.db
        ))
    })
}

/// Resolves what the call is about and checks the other connections it
/// reads, before the user is asked or anything runs.
pub(super) fn prepare(call: &mut Call<'_>, args: &Value) -> AppResult<Access> {
    let mut others: Vec<String> = Vec::new();
    match call.tool {
        Tool::ListRelations => call.target.label = Some("relations".to_string()),
        Tool::GetRelation | Tool::UpdateRelation | Tool::DeleteRelation => {
            call.target.label = Some(relation_of(call, args)?.name);
        }
        Tool::CreateRelation => {
            call.target.label = json_of(args, "relation")?
                .and_then(|r| r.get("name").and_then(Value::as_str).map(str::to_string));
        }
        Tool::SuggestRelationLinks => {
            let (from, to) = (suggest_side(args, "from")?, suggest_side(args, "to")?);
            call.target.label = Some(format!(
                "{}.{} → {}.{}",
                from.side.database, from.side.collection, to.side.database, to.side.collection
            ));
            others = vec![from.connection_id, to.connection_id];
        }
        Tool::TestRelation => {
            let relation = relation_of(call, args)?;
            others.push(test_target(call, &relation)?);
            call.target.label = Some(relation.name);
        }
        Tool::FindRelated => {
            let find = find_args(call, args)?;
            call.target.database = Some(find.database);
            call.target.collection = Some(find.collection);
            others.push(find.target_connection);
        }
        _ => {}
    }
    for id in &others {
        if !readable(call, id) {
            return Ok(Access::NoAccess);
        }
        connection_exists(call, id)?;
    }
    Ok(Access::Granted)
}

/// A relation from a client's JSON: missing names default to the other
/// side's collection, the id and dates are the store's.
fn relation_from_json(value: Value) -> AppResult<Relation> {
    let Value::Object(mut map) = value else {
        return Err(invalid("relation must be a JSON object."));
    };
    for key in ["draft", "createdAt", "updatedAt"] {
        map.remove(key);
    }
    map.entry("id").or_insert(Value::String(String::new()));
    let coll_of = |map: &Map<String, Value>, side: &str| {
        map.get(side)
            .and_then(|s| s.get("coll"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let (from_coll, to_coll) = (coll_of(&map, "from"), coll_of(&map, "to"));
    map.entry("fromName").or_insert(Value::String(to_coll));
    map.entry("toName").or_insert(Value::String(from_coll));
    serde_json::from_value(Value::Object(map)).map_err(|e| invalid(format!("relation: {e}")))
}

fn template_ok(expr: &str) -> bool {
    expr.matches('{').count() == expr.matches('}').count()
        && placeholders(expr).iter().all(|p| !p.trim().is_empty())
}

/// Checks a client's relation is complete and points at real connections.
fn validate(call: &Call<'_>, r: &Relation) -> AppResult<()> {
    if r.name.trim().is_empty() {
        return Err(invalid("relation.name is required."));
    }
    for (label, side) in [("from", &r.from), ("to", &r.to)] {
        if side.db.trim().is_empty() || side.coll.trim().is_empty() {
            return Err(invalid(format!("relation.{label} needs db and coll.")));
        }
        if !template_ok(&side.coll) || placeholders(&side.coll).iter().any(|p| p != "tenant") {
            return Err(invalid(format!(
                "relation.{label}.coll is a name or a pattern with {{tenant}} only."
            )));
        }
    }
    match r.from.conn.as_str() {
        "any" => {}
        "" => return Err(invalid("relation.from.conn is \"any\" or a connection id.")),
        id => connection_exists(call, id)?,
    }
    match r.to.conn.as_str() {
        "cluster" | "same" => {}
        "" => {
            return Err(invalid(
                "relation.to.conn is \"cluster\", \"same\" or a connection id.",
            ))
        }
        id => connection_exists(call, id)?,
    }
    if r.to.coll.contains("{tenant}") && !r.from.coll.contains("{tenant}") {
        return Err(invalid(
            "relation.to.coll has {tenant} but relation.from.coll doesn't: the tenant comes from the From collection's name.",
        ));
    }
    if r.rules.is_empty() || r.rules.iter().any(Vec::is_empty) {
        return Err(invalid(
            "relation.rules needs at least one rule, each with at least one condition.",
        ));
    }
    for c in r.rules.iter().flatten() {
        if c.from.trim().is_empty() || c.to.trim().is_empty() || !template_ok(&c.from) {
            return Err(invalid(
                "every condition needs a From field (or a {field} template) and a To field.",
            ));
        }
        if c.to.contains('{') {
            return Err(invalid(
                "a condition's To side is a field path, not a template.",
            ));
        }
    }
    let tenanted = r.from.coll.contains("{tenant}") || r.to.coll.contains("{tenant}");
    for s in &r.scope {
        if s.to.trim().is_empty() || s.value.trim().is_empty() || !template_ok(&s.value) {
            return Err(invalid("every scope filter needs a To field and a value."));
        }
        if s.value.contains("{tenant}") && !tenanted {
            return Err(invalid(
                "a scope value uses {tenant} but neither side's collection has {tenant}.",
            ));
        }
    }
    Ok(())
}

fn op_text(op: Op) -> &'static str {
    match op {
        Op::Eq => "=",
        Op::Prefix => "is a prefix of",
        Op::In => "(array) contains",
    }
}

fn rule_text(rule: &[relations::Condition]) -> String {
    rule.iter()
        .map(|c| {
            let bridge = serde_json::to_value(c.bridge)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let compare = if bridge == "as-stored" {
                String::new()
            } else {
                format!(" ({bridge})")
            };
            format!("{} {} {}{compare}", c.from, op_text(c.op), c.to)
        })
        .collect::<Vec<_>>()
        .join(" and ")
}

fn summary(r: &Relation) -> Value {
    json!({
        "id": r.id,
        "name": r.name,
        "from": r.from,
        "to": r.to,
        "rules": r.rules.iter().map(|rule| rule_text(rule)).collect::<Vec<_>>(),
        "scope": r.scope.iter().map(|s| format!("{} = {}", s.to, s.value)).collect::<Vec<_>>(),
        "fromName": r.from_name,
        "toName": r.to_name,
        "draft": r.draft.is_some(),
    })
}

fn pretty(value: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// The approval card.
pub(super) fn plan(call: &Call<'_>, args: &Value) -> AppResult<Plan> {
    let plan = |title: String, code: String| Plan {
        title,
        code,
        dry_run: None,
    };
    Ok(match call.tool {
        Tool::ListRelations => plan(
            "List the saved relations".to_string(),
            "list_relations".to_string(),
        ),
        Tool::GetRelation => {
            let r = relation_of(call, args)?;
            plan(format!("Read the relation \"{}\"", r.name), pretty(&r))
        }
        Tool::SuggestRelationLinks => {
            let (from, to) = (suggest_side(args, "from")?, suggest_side(args, "to")?);
            plan(
                format!(
                    "Sample {}.{} and {}.{} for link suggestions",
                    from.side.database, from.side.collection, to.side.database, to.side.collection
                ),
                format!(
                    "from: {}.{}\nto: {}.{}\n(small samples; no document values are returned)",
                    from.side.database, from.side.collection, to.side.database, to.side.collection
                ),
            )
        }
        Tool::CreateRelation => {
            let r = new_relation(call, args)?;
            plan(format!("Propose the relation \"{}\"", r.name), pretty(&r))
        }
        Tool::UpdateRelation => {
            let r = changed_relation(call, args)?;
            plan(
                format!("Change the draft relation \"{}\"", r.name),
                pretty(&r),
            )
        }
        Tool::DeleteRelation => {
            let r = draft_relation(call, args)?;
            plan(
                format!("Delete the draft relation \"{}\"", r.name),
                pretty(&summary(&r)),
            )
        }
        Tool::TestRelation => {
            let r = relation_of(call, args)?;
            plan(
                format!("Check the relation \"{}\" on samples", r.name),
                pretty(&summary(&r)),
            )
        }
        Tool::FindRelated => {
            let find = find_args(call, args)?;
            let filter = match json_of(args, "filter")? {
                Some(f) => js(&f),
                None => "{}".to_string(),
            };
            let (other_db, other_coll) = match find.dir {
                Dir::From => (&find.relation.to.db, &find.relation.to.coll),
                Dir::To => (&find.relation.from.db, &find.relation.from.coll),
            };
            plan(
                format!("find_related through \"{}\"", find.relation.name),
                format!(
                    "{}\n// then the related documents in {other_db}.{other_coll} on connection {}",
                    code_on(
                        &find.database,
                        Some(&find.collection),
                        &format!("find({filter}).limit({})", find.limit)
                    ),
                    connection_name(call, &find.target_connection)
                ),
            )
        }
        _ => unreachable!("relation tools only"),
    })
}

fn connection_name(call: &Call<'_>, id: &str) -> String {
    call.inner
        .state()
        .connection_store
        .get(id)
        .map(|p| p.name)
        .unwrap_or_else(|_| id.to_string())
}

fn why_arg(args: &Value) -> Option<String> {
    str_arg(args, "why").map(|w| w.chars().take(2000).collect())
}

fn new_relation(call: &Call<'_>, args: &Value) -> AppResult<Relation> {
    let value = json_of(args, "relation")?.ok_or_else(|| invalid("relation is required."))?;
    let why = why_arg(args).ok_or_else(|| {
        invalid("why is required: say in a sentence or two why the sides link this way.")
    })?;
    let mut r = relation_from_json(value)?;
    r.id = String::new();
    validate(call, &r)?;
    r.draft = Some(Draft {
        by: call.client.name.clone(),
        at: relations::store::now_iso(),
        why,
    });
    Ok(r)
}

/// The relation named by relation_id, when it's a client's draft.
fn draft_relation(call: &Call<'_>, args: &Value) -> AppResult<Relation> {
    let r = relation_of(call, args)?;
    if r.draft.is_none() {
        return Err(invalid(
            "This relation was saved by the user; only drafts proposed over MCP can be changed \
             or deleted. Propose a new one with create_relation instead.",
        ));
    }
    Ok(r)
}

fn changed_relation(call: &Call<'_>, args: &Value) -> AppResult<Relation> {
    let existing = draft_relation(call, args)?;
    let Some(Value::Object(changes)) = json_of(args, "relation")? else {
        return Err(invalid(
            "relation must be a JSON object with the fields that change.",
        ));
    };
    let Value::Object(mut map) =
        serde_json::to_value(&existing).map_err(|e| AppError::InvalidInput(e.to_string()))?
    else {
        unreachable!("a relation serializes to an object");
    };
    for (key, value) in changes {
        if !matches!(key.as_str(), "id" | "draft" | "createdAt" | "updatedAt") {
            map.insert(key, value);
        }
    }
    let mut r = relation_from_json(Value::Object(map))?;
    r.id = existing.id.clone();
    r.created_at = existing.created_at.clone();
    validate(call, &r)?;
    let old_why = existing.draft.map(|d| d.why).unwrap_or_default();
    r.draft = Some(Draft {
        by: call.client.name.clone(),
        at: relations::store::now_iso(),
        why: why_arg(args).unwrap_or(old_why),
    });
    Ok(r)
}

fn announce(call: &Call<'_>) -> AppResult<()> {
    let list = call.inner.host.relations().list()?;
    call.inner.emit(relations::CHANGED_EVENT, &list);
    Ok(())
}

async fn client_for(call: &Call<'_>, connection_id: &str) -> AppResult<Client> {
    let profile = call.inner.state().connection_store.get(connection_id)?;
    call.inner
        .pool
        .client(call.inner.host.as_ref(), &profile)
        .await
}

/// Runs the call, after the policy (and the user) allowed it.
pub(super) async fn execute(
    call: &Call<'_>,
    args: &Value,
    mongo: Option<&Client>,
) -> AppResult<Done> {
    let store = call.inner.host.relations();
    match call.tool {
        Tool::ListRelations => {
            let list = store.list()?;
            let count = plural(list.len() as u64, "relation", "relations");
            let text = if list.is_empty() {
                "No relations are saved in Mongo Studio yet. Propose one with create_relation \
                 (suggest_relation_links finds how two collections link)."
                    .to_string()
            } else {
                format!(
                    "{count} (one JSON object per line; a draft was proposed by a client and \
                     isn't saved by the user yet):\n{}",
                    json_lines(list.iter().map(summary).collect())
                )
            };
            Ok(Done { text, meta: count })
        }
        Tool::GetRelation => {
            let r = relation_of(call, args)?;
            Ok(Done {
                text: pretty(&r),
                meta: r.name,
            })
        }
        Tool::CreateRelation => {
            let saved = store.upsert(new_relation(call, args)?)?;
            announce(call)?;
            log::info!(
                "MCP {}: proposed the relation \"{}\" ({})",
                call.client.name,
                saved.name,
                saved.id
            );
            Ok(Done {
                text: format!(
                    "Saved the draft relation \"{}\" with id {}. The user reviews it in Mongo \
                     Studio's Relations; nothing uses it until they save it. Check it with \
                     test_relation.\n{}",
                    saved.name,
                    saved.id,
                    pretty(&saved)
                ),
                meta: format!("{} · draft", saved.name),
            })
        }
        Tool::UpdateRelation => {
            let saved = store.upsert(changed_relation(call, args)?)?;
            announce(call)?;
            log::info!(
                "MCP {}: changed the draft relation \"{}\" ({})",
                call.client.name,
                saved.name,
                saved.id
            );
            Ok(Done {
                text: format!(
                    "Updated the draft relation \"{}\"; it is still a draft for the user to \
                     review.\n{}",
                    saved.name,
                    pretty(&saved)
                ),
                meta: format!("{} · draft", saved.name),
            })
        }
        Tool::DeleteRelation => {
            let r = draft_relation(call, args)?;
            store.delete(&r.id)?;
            announce(call)?;
            log::info!(
                "MCP {}: deleted the draft relation \"{}\" ({})",
                call.client.name,
                r.name,
                r.id
            );
            Ok(Done {
                text: format!("Deleted the draft relation \"{}\".", r.name),
                meta: r.name,
            })
        }
        Tool::SuggestRelationLinks => {
            let (from, to) = (suggest_side(args, "from")?, suggest_side(args, "to")?);
            let from_client = client_for(call, &from.connection_id).await?;
            let to_client = client_for(call, &to.connection_id).await?;
            let suggestions =
                sampling::suggest_links(&from_client, &from.side, &to_client, &to.side).await?;
            let count = plural(suggestions.len() as u64, "suggestion", "suggestions");
            let lines: Vec<Value> = suggestions
                .iter()
                .map(|s| {
                    let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
                    if let Value::Object(map) = &mut v {
                        let rate = if s.sampled == 0 {
                            0.0
                        } else {
                            (s.matched as f64 / s.sampled as f64 * 100.0).round() / 100.0
                        };
                        map.insert("matchRate".into(), json!(rate));
                    }
                    v
                })
                .collect();
            let text = if lines.is_empty() {
                format!(
                    "No links found between {}.{} and {}.{} in the samples.",
                    from.side.database, from.side.collection, to.side.database, to.side.collection
                )
            } else {
                format!(
                    "{count}, best first (from/to are field paths; op and as go into a \
                     condition as they are; matched of sampled From values were found on the \
                     To side):\n{}",
                    json_lines(lines)
                )
            };
            Ok(Done { text, meta: count })
        }
        Tool::TestRelation => {
            let r = relation_of(call, args)?;
            let from_id = call.target.connection_id.clone().unwrap_or_default();
            let to_id = test_target(call, &r)?;
            let from_client = mongo.expect("test_relation uses mongo").clone();
            let to_client = if to_id == from_id {
                from_client.clone()
            } else {
                client_for(call, &to_id).await?
            };
            let report = sampling::check_relation(&from_client, &to_client, &r, &from_id).await?;
            let mut text = format!(
                "Checked \"{}\" from {} (To side on {}):\n",
                r.name,
                connection_name(call, &from_id),
                connection_name(call, &to_id)
            );
            for line in &report.lines {
                let kind = serde_json::to_value(line.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                text.push_str(&format!("[{kind}] {}\n", line.text));
            }
            let missing: Vec<_> = report
                .lines
                .iter()
                .filter_map(|l| l.index.as_ref())
                .collect();
            text.push_str(
                &json!({
                    "ruleMatches": report.rule_matches,
                    "sampled": report.sampled,
                    "none": report.none,
                    "missingIndexes": missing,
                })
                .to_string(),
            );
            Ok(Done {
                text,
                meta: format!("{} sampled · {} found nothing", report.sampled, report.none),
            })
        }
        Tool::FindRelated => {
            find_related(call, args, mongo.expect("find_related uses mongo")).await
        }
        _ => unreachable!("relation tools only"),
    }
}

async fn find_related(call: &Call<'_>, args: &Value, source_client: &Client) -> AppResult<Done> {
    let find = find_args(call, args)?;
    let source_id = call.target.connection_id.clone().unwrap_or_default();
    let filter = super::doc_of(args, "filter")?;
    let docs: Vec<mongodb::bson::Document> = {
        use futures_util::TryStreamExt;
        collection(source_client, &find.database, &find.collection)
            .find(filter)
            .limit(find.limit)
            .max_time(MAX_TIME)
            .await?
            .try_collect()
            .await?
    };
    let target_client = if find.target_connection == source_id {
        source_client.clone()
    } else {
        client_for(call, &find.target_connection).await?
    };
    let source = Source {
        connection_id: source_id,
        database: find.database.clone(),
        collection: find.collection.clone(),
    };
    let out = engine::find_related(
        &target_client,
        &find.relation,
        find.dir,
        &source,
        &docs,
        &find.aggregates,
        DOCS_PER_SOURCE,
    )
    .await?;

    let related_name = match find.dir {
        Dir::From => &find.relation.from_name,
        Dir::To => &find.relation.to_name,
    };
    let with_related = out.results.iter().filter(|f| f.count > 0).count() as u64;
    let lines = |docs_kept: usize| -> Vec<Value> {
        docs.iter()
            .zip(&out.results)
            .map(|(doc, found)| {
                json!({
                    "_id": doc.get("_id").cloned().map(bson_to_json).unwrap_or(Value::Null),
                    "count": found.count,
                    "rule": found.rule.map(|r| r + 1),
                    "aggregates": found.aggregates,
                    "docs": found.docs.iter().take(docs_kept).collect::<Vec<_>>(),
                })
            })
            .collect()
    };
    let mut header = format!(
        "Read {} from {}.{}; {} {} related {related_name} in {}.{} on {} ({} read in {} ms).",
        plural(docs.len() as u64, "document", "documents"),
        find.database,
        find.collection,
        with_related,
        if with_related == 1 { "has" } else { "have" },
        out.target_database,
        out.target_collection,
        connection_name(call, &find.target_connection),
        plural(out.matched, "document", "documents"),
        out.ms
    );
    if out.truncated {
        header.push_str(&format!(
            " The read stopped at {} related documents, so counts and aggregates may be low: \
             narrow the filter or lower the limit.",
            engine::MATCH_CAP
        ));
    }
    header.push_str(
        " One JSON object per document: count of related documents, the rule that found them \
         (1 = first), your aggregates, and up to 5 of them (relaxed Extended JSON).",
    );
    // Fewer documents per line until it fits.
    let mut body = String::new();
    for kept in [DOCS_PER_SOURCE, 2, 1, 0] {
        body = json_lines(lines(kept));
        if header.len() + body.len() + 200 < MAX_OUTPUT {
            if kept < DOCS_PER_SOURCE {
                header.push_str(&format!(
                    " To fit, only {kept} related document(s) per line are shown."
                ));
            }
            break;
        }
    }
    let text = if docs.is_empty() {
        header
    } else {
        truncate(format!("{header}\n{body}"), MAX_OUTPUT)
    };
    Ok(Done {
        text,
        meta: format!(
            "{} · {with_related} with {related_name}",
            plural(docs.len() as u64, "document", "documents")
        ),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;

    use super::super::super::test_support::{add_session, inner, set_policy, TestHost};
    use super::super::super::{McpConfig, Policy, ACTIVITY_EVENT};
    use super::super::tests::profile;
    use super::super::{call, ToolOutput};
    use super::*;
    use crate::mcp_server::Inner;

    fn config(caps: &[(&str, Policy)], connections: &[(&str, Level)]) -> McpConfig {
        McpConfig {
            enabled: true,
            port: 0,
            caps: caps.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            connections: connections
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
            always: HashMap::new(),
            alerts: Default::default(),
        }
    }

    async fn run(inner: &Arc<Inner>, name: &str, args: Value) -> ToolOutput {
        let client = add_session(inner, "s1", "Claude Code");
        call(inner, &client, name, args).await.unwrap()
    }

    fn proposal() -> Value {
        json!({
            "name": "Order shipments",
            "from": { "conn": "any", "db": "shop", "coll": "{tenant}_orders" },
            "to": { "conn": "cluster", "db": "logistics", "coll": "shipments" },
            "rules": [[
                { "from": "customerId", "op": "eq", "to": "customerId", "as": "uuid-string" },
                { "from": "key", "op": "prefix", "to": "orderRef", "as": "uuid-string" }
            ]],
            "scope": [{ "to": "store", "value": "{tenant}" }],
        })
    }

    fn relation_changes(host: &TestHost) -> Vec<Value> {
        host.events_named(relations::CHANGED_EVENT)
    }

    #[tokio::test]
    async fn drafts_are_created_listed_changed_and_deleted() {
        let (host, inner) = inner(Duration::from_secs(1));
        set_policy(
            &inner,
            config(
                &[("relRead", Policy::Allow), ("relEdit", Policy::Allow)],
                &[],
            ),
        );
        let out = run(&inner, "list_relations", json!({})).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("No relations"));

        // `why` is required, and so is a valid relation.
        let out = run(&inner, "create_relation", json!({ "relation": proposal() })).await;
        assert!(out.is_error);
        assert!(out.text.contains("why is required"), "{}", out.text);
        let mut bad = proposal();
        bad["rules"] = json!([]);
        let out = run(
            &inner,
            "create_relation",
            json!({ "relation": bad, "why": "x" }),
        )
        .await;
        assert!(out.is_error);
        assert!(out.text.contains("at least one rule"), "{}", out.text);
        let mut bad = proposal();
        bad["from"]["coll"] = json!("orders");
        let out = run(
            &inner,
            "create_relation",
            json!({ "relation": bad, "why": "x" }),
        )
        .await;
        assert!(out.text.contains("{tenant}"), "{}", out.text);
        let mut bad = proposal();
        bad["to"]["conn"] = json!("nope");
        let out = run(
            &inner,
            "create_relation",
            json!({ "relation": bad, "why": "x" }),
        )
        .await;
        assert!(out.text.contains("no saved connection"), "{}", out.text);
        assert!(relation_changes(&host).is_empty());

        let out = run(
            &inner,
            "create_relation",
            json!({ "relation": proposal(), "why": "Shipments carry the order key as text." }),
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        let saved = host.relations.list().unwrap();
        assert_eq!(saved.len(), 1);
        let r = &saved[0];
        assert_eq!(r.name, "Order shipments");
        assert_eq!(r.from_name, "shipments");
        assert_eq!(r.to_name, "{tenant}_orders");
        let draft = r.draft.as_ref().unwrap();
        assert_eq!(draft.by, "Claude Code");
        assert_eq!(draft.why, "Shipments carry the order key as text.");
        assert!(out.text.contains(&r.id));
        let event = relation_changes(&host).pop().unwrap();
        assert_eq!(event.as_array().unwrap().len(), 1);
        assert_eq!(event[0]["id"], r.id.as_str());
        let activity = host.events_named(ACTIVITY_EVENT).pop().unwrap();
        assert_eq!(activity["tool"], "create_relation");
        assert_eq!(activity["target"], "Order shipments");
        assert_eq!(activity["meta"], "Order shipments · draft");

        let out = run(&inner, "list_relations", json!({})).await;
        assert!(out.text.starts_with("1 relation"), "{}", out.text);
        let line: Value = serde_json::from_str(out.text.lines().nth(1).unwrap()).unwrap();
        assert_eq!(line["draft"], true);
        assert_eq!(
            line["rules"][0],
            "customerId = customerId (uuid-string) and key is a prefix of orderRef (uuid-string)"
        );
        assert_eq!(line["scope"][0], "store = {tenant}");

        let out = run(&inner, "get_relation", json!({ "relation_id": r.id })).await;
        let full: Value = serde_json::from_str(&out.text).unwrap();
        assert_eq!(full["rules"][0][1]["op"], "prefix");

        let out = run(
            &inner,
            "update_relation",
            json!({ "relation_id": r.id, "relation": { "name": "Shipments of a order", "id": "hijack" } }),
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        let changed = host.relations.get(&r.id).unwrap();
        assert_eq!(changed.name, "Shipments of a order");
        assert_eq!(changed.created_at, r.created_at);
        assert_eq!(changed.rules, r.rules);
        assert_eq!(
            changed.draft.as_ref().unwrap().why,
            "Shipments carry the order key as text."
        );
        assert_eq!(relation_changes(&host).len(), 2);

        // What the user saved is out of reach.
        let mut user = changed.clone();
        user.id = String::new();
        user.draft = None;
        let user = host.relations.upsert(user).unwrap();
        for (tool, args) in [
            (
                "update_relation",
                json!({ "relation_id": user.id, "relation": { "name": "x" } }),
            ),
            ("delete_relation", json!({ "relation_id": user.id })),
        ] {
            let out = run(&inner, tool, args).await;
            assert!(out.is_error);
            assert!(out.text.contains("saved by the user"), "{}", out.text);
        }

        let out = run(&inner, "delete_relation", json!({ "relation_id": r.id })).await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(host.relations.list().unwrap().len(), 1);
        assert_eq!(
            relation_changes(&host)
                .pop()
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let out = run(&inner, "get_relation", json!({ "relation_id": r.id })).await;
        assert!(out.text.contains("no relation has this id"), "{}", out.text);
    }

    #[tokio::test]
    async fn relation_tools_follow_the_capabilities_and_both_connections_levels() {
        let (host, inner) = inner(Duration::from_secs(1));
        let store = &host.state.connection_store;
        store
            .upsert(profile("shop", "003 shop", "mongodb://db3:27017/shop"))
            .unwrap();
        store
            .upsert(profile(
                "logistics",
                "003 logistics",
                "mongodb://db3:27017/logistics",
            ))
            .unwrap();
        let mut r = relation_from_json(proposal()).unwrap();
        r.draft = None;
        let r = host.relations.upsert(r).unwrap();

        // Off by default: not listed, refused.
        let names: Vec<String> = super::super::definitions(&config(&[], &[]))
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.iter().any(|n| n.contains("relation")));
        set_policy(&inner, config(&[], &[("shop", Level::Read)]));
        let out = run(&inner, "list_relations", json!({})).await;
        assert_eq!(out.text, policy::REFUSED_OFF);

        let find =
            json!({ "relation_id": r.id, "connection_id": "shop", "collection": "berlin_orders" });
        // find_related needs `read` too.
        set_policy(
            &inner,
            config(
                &[("relRead", Policy::Allow)],
                &[("shop", Level::Read), ("logistics", Level::Read)],
            ),
        );
        let out = run(&inner, "find_related", find.clone()).await;
        assert_eq!(out.text, policy::REFUSED_OFF);

        // The other side's connection must be readable.
        set_policy(
            &inner,
            config(
                &[("relRead", Policy::Allow), ("read", Policy::Allow)],
                &[("shop", Level::Read)],
            ),
        );
        let out = run(&inner, "find_related", find.clone()).await;
        assert_eq!(out.text, policy::REFUSED_NO_ACCESS);
        assert_eq!(
            host.events_named(ACTIVITY_EVENT).pop().unwrap()["target"],
            "shop.berlin_orders"
        );
        let out = run(
            &inner,
            "test_relation",
            json!({ "relation_id": r.id, "connection_id": "shop" }),
        )
        .await;
        assert_eq!(out.text, policy::REFUSED_NO_ACCESS);
        set_policy(
            &inner,
            config(&[("relEdit", Policy::Allow)], &[("shop", Level::Read)]),
        );
        let out = run(
            &inner,
            "suggest_relation_links",
            json!({
                "from": { "connection_id": "shop", "database": "shop", "collection": "{tenant}_orders" },
                "to": { "connection_id": "logistics", "database": "logistics", "collection": "shipments" },
            }),
        )
        .await;
        assert_eq!(out.text, policy::REFUSED_NO_ACCESS);

        // Bad arguments fail before anything is read.
        set_policy(
            &inner,
            config(
                &[("relRead", Policy::Allow), ("read", Policy::Allow)],
                &[("shop", Level::Read), ("logistics", Level::Read)],
            ),
        );
        let out = run(
            &inner,
            "find_related",
            json!({ "relation_id": r.id, "connection_id": "shop" }),
        )
        .await;
        assert!(
            out.text.contains("pass collection or tenant"),
            "{}",
            out.text
        );
        let out = run(
            &inner,
            "find_related",
            json!({ "relation_id": r.id, "connection_id": "shop", "collection": "berlin_customers" }),
        )
        .await;
        assert!(
            out.text.contains("isn't a collection of this side"),
            "{}",
            out.text
        );
    }
}

#[cfg(test)]
mod live_tests {
    //! find_related through MCP against `MONGO_STUDIO_TEST_URI`, on the
    //! engine's fixture databases. Run with:
    //!   MONGO_STUDIO_TEST_URI=mongodb://localhost:27017 \
    //!     cargo test --lib -- --ignored relations
    use std::collections::HashMap;
    use std::time::Duration;

    use serde_json::json;

    use super::super::super::test_support::{add_session, inner, set_policy};
    use super::super::super::{McpConfig, Policy};
    use super::super::call;
    use super::super::tests::profile;
    use super::*;
    use crate::relations::engine::live_tests as fx;

    #[tokio::test]
    #[ignore]
    async fn live_relations_find_related_through_mcp() {
        let uri = fx::uri();
        let seed = Client::with_uri_str(&uri).await.unwrap();
        let (shop, logistics) = fx::fixtures(&seed).await;
        let (host, inner) = inner(Duration::from_secs(1));
        let store = &host.state.connection_store;
        // Two saved connections to one server, one per database: the
        // cluster match picks the logistics one by its default database.
        let mut shop_profile = profile("shopc", "Local shop", &uri);
        shop_profile.database = Some(shop.clone());
        let mut logistics_profile = profile("logisticsc", "Local billing", &uri);
        logistics_profile.database = Some(logistics.clone());
        store.upsert(shop_profile).unwrap();
        store.upsert(logistics_profile).unwrap();
        let mut relation = fx::shipments_relation(&shop, &logistics);
        relation.id = String::new();
        let relation = host.relations.upsert(relation).unwrap();
        set_policy(
            &inner,
            McpConfig {
                enabled: true,
                port: 0,
                caps: [("relRead", Policy::Allow), ("read", Policy::Allow)]
                    .map(|(c, p)| (c.to_string(), p))
                    .into_iter()
                    .collect(),
                connections: HashMap::from([
                    ("shopc".to_string(), Level::Read),
                    ("logisticsc".to_string(), Level::Read),
                ]),
                always: HashMap::new(),
                alerts: Default::default(),
            },
        );
        let client = add_session(&inner, "s1", "Live test");
        let out = call(
            &inner,
            &client,
            "find_related",
            json!({
                "relation_id": relation.id,
                "connection_id": "shopc",
                "tenant": "berlin",
                "filter": { "_id": { "$in": [1, 3] } },
                "aggregates": [
                    { "id": "delivered", "kind": "count", "whereField": "status", "whereValue": "DELIVERED" },
                    { "id": "weight", "kind": "sum", "field": "weight" }
                ],
            }),
        )
        .await
        .unwrap();
        let back = call(
            &inner,
            &client,
            "find_related",
            json!({
                "relation_id": relation.id,
                "side": "to",
                "connection_id": "logisticsc",
                "filter": { "_id": 3 },
            }),
        )
        .await
        .unwrap();
        inner
            .config
            .lock()
            .unwrap()
            .caps
            .insert("relEdit".to_string(), Policy::Allow);
        let suggest = call(
            &inner,
            &client,
            "suggest_relation_links",
            json!({
                "from": { "connection_id": "shopc", "database": shop, "collection": "{tenant}_orders" },
                "to": { "connection_id": "logisticsc", "database": logistics, "collection": "shipments" },
            }),
        )
        .await
        .unwrap();
        let test = call(
            &inner,
            &client,
            "test_relation",
            json!({ "relation_id": relation.id, "connection_id": "shopc" }),
        )
        .await
        .unwrap();
        inner.pool.close_all("test over").await;
        fx::drop_fixtures(&seed, &shop, &logistics).await;
        println!("{}\n\n{}", suggest.text, test.text);
        assert!(!suggest.is_error, "{}", suggest.text);
        assert!(suggest.text.contains("\"matchRate\""), "{}", suggest.text);
        assert!(
            !suggest.text.contains("eed4b0b7"),
            "no values: {}",
            suggest.text
        );
        assert!(!test.is_error, "{}", test.text);
        assert!(test.text.contains("\"ruleMatches\""), "{}", test.text);

        println!("{}\n\n{}", out.text, back.text);
        assert!(!out.is_error, "{}", out.text);
        let mut lines = out.text.lines();
        let header = lines.next().unwrap();
        assert!(header.starts_with(&format!(
            "Read 2 documents from {shop}.berlin_orders; 1 has related shipments in {logistics}.shipments on Local billing"
        )), "{header}");
        let first: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(first["_id"], 1);
        assert_eq!(first["count"], 3);
        assert_eq!(first["rule"], 1);
        assert_eq!(first["aggregates"]["delivered"], 2);
        assert_eq!(first["aggregates"]["weight"], 5.5);
        assert_eq!(first["docs"].as_array().unwrap().len(), 3);
        let none: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(none["_id"], 3);
        assert_eq!(none["count"], 0);
        assert_eq!(none["rule"], Value::Null);

        // Backwards: the suffixed shipment finds its order on the shop connection.
        assert!(!back.is_error, "{}", back.text);
        assert!(back.text.contains("on Local shop"), "{}", back.text);
        let line: Value = serde_json::from_str(back.text.lines().nth(1).unwrap()).unwrap();
        assert_eq!(line["count"], 1);
        assert_eq!(line["docs"][0]["text"], "hi");
    }
}

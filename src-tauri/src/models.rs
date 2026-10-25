use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub type ConnectionId = String;
pub type SessionId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionSource {
    /// A full connection string, e.g. pasted from Atlas (mongodb+srv://...).
    /// Any username/password embedded in it is extracted into the secret
    /// store before this variant is persisted, so `uri` never carries a
    /// plaintext password to disk.
    Uri { uri: String },
    /// A structured host/port form for simple standalone/replica-set setups.
    Manual { host: String, port: u16, srv: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthMechanism {
    ScramSha1,
    ScramSha256,
    MongodbX509,
    MongodbAws,
    Gssapi,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsOptions {
    pub enabled: bool,
    pub ca_file: Option<String>,
    pub cert_key_file: Option<String>,
    /// Whether a passphrase for `cert_key_file` is held in the secret store.
    pub cert_key_has_passphrase: bool,
    pub allow_invalid_certificates: bool,
    pub allow_invalid_hostnames: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuthMethod {
    Password,
    PrivateKey,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTunnelOptions {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: SshAuthMethod,
    pub private_key_path: Option<String>,
    /// Whether a passphrase for `private_key_path` is held in the secret store.
    pub private_key_has_passphrase: bool,
}

/// How binary subtype 3 values (legacy UUIDs) read. Subtype 3 never fixed a
/// byte order, so each old driver wrote the 16 bytes its own way; `Raw`
/// shows them as plain binary. Display only: stored bytes never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LegacyUuidEncoding {
    #[default]
    Raw,
    Python,
    Java,
    Csharp,
}

impl LegacyUuidEncoding {
    /// The encoding a `uuidRepresentation` URI option names, if any.
    fn from_uri(uri: &str) -> Option<Self> {
        let query = uri.split_once('?')?.1;
        let query = query.split('#').next().unwrap_or(query);
        let value = query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            key.eq_ignore_ascii_case("uuidRepresentation")
                .then_some(value)
        })?;
        Some(match value.to_ascii_lowercase().as_str() {
            "javalegacy" => Self::Java,
            "csharplegacy" => Self::Csharp,
            "pythonlegacy" => Self::Python,
            _ => Self::Raw,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionAdvancedOptions {
    pub app_name: Option<String>,
    pub connect_timeout_ms: Option<u64>,
    pub server_selection_timeout_ms: Option<u64>,
    pub max_pool_size: Option<u32>,
    pub min_pool_size: Option<u32>,
    pub replica_set: Option<String>,
    pub read_preference: Option<String>,
    pub retry_writes: Option<bool>,
    pub direct_connection: Option<bool>,
    pub auth_mechanism: Option<AuthMechanism>,
    pub auth_source: Option<String>,
    /// How legacy UUIDs show; `None` follows the connection string's
    /// `uuidRepresentation`, else raw.
    #[serde(default)]
    pub legacy_uuid: Option<LegacyUuidEncoding>,
}

/// Persisted, non-secret connection metadata. Lives in `connections.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    pub id: ConnectionId,
    pub name: String,
    /// Identity color as `#RRGGBB`; `None` lets the UI pick one from the id.
    /// Defaulted so files saved before it existed still load.
    #[serde(default)]
    pub color: Option<String>,
    pub source: ConnectionSource,
    pub database: Option<String>,
    pub username: Option<String>,
    /// Whether a password for `username` is held in the secret store.
    pub has_password: bool,
    pub tls: TlsOptions,
    pub ssh_tunnel: Option<SshTunnelOptions>,
    pub advanced: ConnectionAdvancedOptions,
    pub created_at: String,
    pub updated_at: String,
}

/// What the frontend submits when creating/editing a profile. Secret fields
/// here are written to the secret store and never appear in `ConnectionProfile`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfileInput {
    pub id: Option<ConnectionId>,
    pub name: String,
    /// `#RRGGBB` or `None` for automatic; checked by `validate_color`.
    #[serde(default)]
    pub color: Option<String>,
    pub source: ConnectionSource,
    pub database: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub tls: TlsOptions,
    pub tls_cert_key_passphrase: Option<String>,
    pub ssh_tunnel: Option<SshTunnelOptions>,
    pub ssh_password: Option<String>,
    pub ssh_key_passphrase: Option<String>,
    pub advanced: ConnectionAdvancedOptions,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfileMeta {
    pub id: ConnectionId,
    pub name: String,
    pub color: Option<String>,
    pub summary: String,
    pub database: Option<String>,
    /// Whether a password is saved for it, for the export picker.
    pub has_password: bool,
    /// How its legacy UUIDs show, the connection string's choice applied.
    pub legacy_uuid: LegacyUuidEncoding,
    /// Whether that was picked for it rather than left to the connection string.
    pub legacy_uuid_chosen: bool,
}

impl From<&ConnectionProfile> for ConnectionProfileMeta {
    fn from(profile: &ConnectionProfile) -> Self {
        let summary = match &profile.source {
            ConnectionSource::Uri { uri } => redact_uri_summary(uri),
            ConnectionSource::Manual { host, port, srv } => {
                if *srv {
                    format!("mongodb+srv://{host}")
                } else {
                    format!("mongodb://{host}:{port}")
                }
            }
        };
        Self {
            id: profile.id.clone(),
            name: profile.name.clone(),
            color: profile.color.clone(),
            summary,
            database: profile.database.clone(),
            has_password: profile.has_password,
            legacy_uuid: profile.legacy_uuid(),
            legacy_uuid_chosen: profile.advanced.legacy_uuid.is_some(),
        }
    }
}

impl ConnectionProfile {
    /// The legacy UUID encoding in effect: the one picked, else the
    /// connection string's `uuidRepresentation`, else raw.
    pub fn legacy_uuid(&self) -> LegacyUuidEncoding {
        let from_uri = match &self.source {
            ConnectionSource::Uri { uri } => LegacyUuidEncoding::from_uri(uri),
            ConnectionSource::Manual { .. } => None,
        };
        self.advanced.legacy_uuid.or(from_uri).unwrap_or_default()
    }
}

/// A connection color is either absent or a `#RRGGBB` hex string.
pub(crate) fn validate_color(color: Option<&str>) -> AppResult<()> {
    match color {
        None => Ok(()),
        Some(c)
            if c.len() == 7
                && c.starts_with('#')
                && c[1..].chars().all(|ch| ch.is_ascii_hexdigit()) =>
        {
            Ok(())
        }
        Some(c) => Err(AppError::InvalidInput(format!(
            "connection color must be a #RRGGBB hex value, got \"{c}\""
        ))),
    }
}

pub(crate) fn redact_uri_summary(uri: &str) -> String {
    match uri.find('@') {
        Some(at_idx) => match uri.find("://") {
            Some(scheme_idx) => format!("{}://***@{}", &uri[..scheme_idx], &uri[at_idx + 1..]),
            None => uri.to_string(),
        },
        None => uri.to_string(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTestResult {
    pub success: bool,
    pub message: String,
    pub server_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionHandle {
    pub session_id: SessionId,
    pub server_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretBackendKind {
    Keyring,
    EncryptedFile,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretBackendInfo {
    pub backend: SecretBackendKind,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub name: String,
    pub size_on_disk: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionInfo {
    pub name: String,
    pub collection_type: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindQueryInput {
    pub filter: serde_json::Value,
    pub sort: Option<serde_json::Value>,
    pub projection: Option<serde_json::Value>,
    pub limit: Option<i64>,
    pub skip: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResultPage {
    pub documents: Vec<serde_json::Value>,
    pub returned: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub key: serde_json::Value,
    pub unique: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionStats {
    pub document_count: u64,
    pub indexes: Vec<IndexInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptResult {
    pub value: serde_json::Value,
    pub logs: Vec<String>,
    /// Where the script ended up: its database, or the one a `use` switched to.
    pub database: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportQueryInput {
    pub filter: serde_json::Value,
    pub sort: Option<serde_json::Value>,
    pub projection: Option<serde_json::Value>,
    /// When set, exports the result of this aggregation pipeline instead of
    /// a plain find. `filter`/`sort`/`projection`/`limit` are ignored in that case.
    pub pipeline: Option<serde_json::Value>,
    /// Caps the number of exported documents, mirroring the grid's current
    /// Limit field. Only applies to the plain find path; omitted by default
    /// so export covers the whole filtered/sorted result set.
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionsExportSummary {
    pub exported: usize,
}

/// One connection found in an import file, shown for picking before
/// anything is saved.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionImportPreview {
    /// Position in the file; what `import_connections` takes to select it.
    pub index: usize,
    pub name: String,
    /// Where it connects, credentials masked.
    pub address: String,
    /// Imports, but needs attention afterwards.
    pub warning: Option<String>,
    /// Can't be imported; the reason why.
    pub error: Option<String>,
    /// A saved connection already has this name.
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionsImportSummary {
    pub imported: usize,
    /// Human-readable reasons individual connections in the file were
    /// skipped (e.g. passphrase-encrypted, or missing a connection
    /// string) - the rest of the file still imports.
    pub errors: Vec<String>,
    /// Notes about connections that *were* imported but need attention,
    /// e.g. a NoSQLBooster-encrypted password that couldn't be carried
    /// over and must be re-entered by hand.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportNestedMode {
    /// Nested objects/arrays become dot- and index-notation columns, e.g.
    /// `address.city`, `items.0.sku`.
    Flatten,
    /// Only top-level keys become columns; nested values are JSON-stringified
    /// into a single cell.
    Stringify,
}

/// The file an export writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// One row per document, one column per field (see ExportNestedMode).
    #[default]
    Csv,
    /// A JSON array of the documents in relaxed Extended JSON, as stored.
    Json,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    #[serde(default)]
    pub format: ExportFormat,
    /// CSV only: how nested fields become columns.
    pub nested_mode: ExportNestedMode,
    /// How many leading documents to sample for column inference. Columns
    /// for fields that only appear later in a very heterogeneous collection
    /// won't be included - a documented tradeoff for not buffering the
    /// entire result set in memory.
    pub sample_size: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    pub rows_written: u64,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainQueryInput {
    pub filter: serde_json::Value,
    pub sort: Option<serde_json::Value>,
    pub projection: Option<serde_json::Value>,
    pub limit: Option<i64>,
    pub skip: Option<u64>,
    /// When set, explains this aggregation pipeline instead of a find.
    pub pipeline: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplainVerbosity {
    QueryPlanner,
    ExecutionStats,
    AllPlansExecution,
}

#[cfg(test)]
mod tests {
    use super::LegacyUuidEncoding;

    #[test]
    fn legacy_uuid_follows_the_uri_option() {
        let read = LegacyUuidEncoding::from_uri;
        assert_eq!(
            read("mongodb://h/?uuidRepresentation=javaLegacy"),
            Some(LegacyUuidEncoding::Java)
        );
        assert_eq!(
            read("mongodb://h/db?w=1&UUIDREPRESENTATION=csharplegacy#x"),
            Some(LegacyUuidEncoding::Csharp)
        );
        assert_eq!(
            read("mongodb://h/?uuidRepresentation=pythonLegacy"),
            Some(LegacyUuidEncoding::Python)
        );
        assert_eq!(
            read("mongodb://h/?uuidRepresentation=standard"),
            Some(LegacyUuidEncoding::Raw)
        );
        assert_eq!(read("mongodb://h/?w=1"), None);
        assert_eq!(read("mongodb://h"), None);
    }
}

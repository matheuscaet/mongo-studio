//! Import/export of connection profiles.
//!
//! Export always writes the "Compass Connections" JSON schema MongoDB
//! Compass itself reads and writes
//! (`{"type": "Compass Connections", "version": 1, "connections": [...]}`),
//! so files round-trip with real Compass too.
//!
//! Import auto-detects and accepts that same Compass format, plus
//! NoSQLBooster's export format (`{"product": "NoSQLBooster", "connections":
//! [...]}`). Two things aren't carried over: Compass's passphrase-encrypted
//! `connectionSecrets` (re-export from Compass without a passphrase instead),
//! and NoSQLBooster's stored passwords (encrypted with an app-internal
//! scheme we can't reverse - those connections import without a password
//! and come back with a warning).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::driver::build_uri;
use crate::error::{AppError, AppResult};
use crate::models::{ConnectionProfile, SshAuthMethod, SshTunnelOptions};
use crate::secrets::{SecretKind, SecretStore};

const FILE_TYPE: &str = "Compass Connections";
const FILE_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
struct ExportFile {
    #[serde(rename = "type")]
    file_type: String,
    version: u32,
    connections: Vec<ExportedConnection>,
}

#[derive(Debug, Serialize)]
struct ExportedConnection {
    id: String,
    #[serde(rename = "connectionOptions")]
    connection_options: ExportedConnectionOptions,
    favorite: ExportedFavorite,
}

#[derive(Debug, Serialize)]
struct ExportedConnectionOptions {
    #[serde(rename = "connectionString")]
    connection_string: String,
    #[serde(rename = "sshTunnel", skip_serializing_if = "Option::is_none")]
    ssh_tunnel: Option<ExportedSshTunnel>,
}

#[derive(Debug, Serialize)]
struct ExportedSshTunnel {
    host: String,
    port: u16,
    username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    #[serde(rename = "identityKeyFile", skip_serializing_if = "Option::is_none")]
    identity_key_file: Option<String>,
    #[serde(
        rename = "identityKeyPassphrase",
        skip_serializing_if = "Option::is_none"
    )]
    identity_key_passphrase: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExportedFavorite {
    name: String,
}

/// Writes `profiles` to `dest_path` as a Compass-compatible connections
/// file. When `include_secrets` is true, passwords/passphrases are read
/// from the secret store and embedded in plaintext - this never crosses
/// the IPC boundary to the frontend, only the resulting file does, so the
/// caller (and the UI) must make that trade-off explicit to the user.
pub fn export_connections(
    profiles: &[ConnectionProfile],
    include_secrets: bool,
    secrets: &dyn SecretStore,
    dest_path: &Path,
) -> AppResult<usize> {
    let mut connections = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let password = if include_secrets && profile.has_password {
            secrets
                .get(&profile.id, SecretKind::Password)
                .map_err(AppError::Secret)?
        } else {
            None
        };
        let connection_string =
            build_uri(profile, profile.username.as_deref(), password.as_deref());

        let ssh_tunnel = profile.ssh_tunnel.as_ref().map(|ssh| {
            let password = if include_secrets && ssh.auth_method == SshAuthMethod::Password {
                secrets
                    .get(&profile.id, SecretKind::SshPassword)
                    .unwrap_or(None)
            } else {
                None
            };
            let identity_key_passphrase = if include_secrets && ssh.private_key_has_passphrase {
                secrets
                    .get(&profile.id, SecretKind::SshKeyPassphrase)
                    .unwrap_or(None)
            } else {
                None
            };
            ExportedSshTunnel {
                host: ssh.host.clone(),
                port: ssh.port,
                username: ssh.username.clone(),
                password,
                identity_key_file: ssh.private_key_path.clone(),
                identity_key_passphrase,
            }
        });

        connections.push(ExportedConnection {
            id: profile.id.clone(),
            connection_options: ExportedConnectionOptions {
                connection_string,
                ssh_tunnel,
            },
            favorite: ExportedFavorite {
                name: profile.name.clone(),
            },
        });
    }

    let file = ExportFile {
        file_type: FILE_TYPE.to_string(),
        version: FILE_VERSION,
        connections,
    };
    let raw =
        serde_json::to_string_pretty(&file).map_err(|e| AppError::InvalidInput(e.to_string()))?;
    std::fs::write(dest_path, raw)?;
    Ok(profiles.len())
}

#[derive(Debug, Deserialize)]
struct ImportFile {
    #[serde(rename = "type")]
    file_type: Option<String>,
    connections: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ImportedConnectionOptions {
    #[serde(rename = "connectionString")]
    connection_string: Option<String>,
    #[serde(rename = "sshTunnel")]
    ssh_tunnel: Option<ImportedSshTunnel>,
}

#[derive(Debug, Deserialize)]
struct ImportedSshTunnel {
    host: Option<String>,
    port: Option<u16>,
    username: Option<String>,
    password: Option<String>,
    #[serde(rename = "identityKeyFile")]
    identity_key_file: Option<String>,
    #[serde(rename = "identityKeyPassphrase")]
    identity_key_passphrase: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImportedFavorite {
    name: Option<String>,
}

/// One connection successfully parsed out of an import file, ready to be
/// handed to `commands::materialize_profile` (which extracts any
/// credentials embedded in `uri` into the secret store, same as a
/// hand-entered connection).
#[derive(Debug)]
pub struct ParsedImportEntry {
    pub name: String,
    pub uri: String,
    /// Set instead of embedding `user:pass@` in `uri` when the source
    /// format keeps username/password as separate fields (NoSQLBooster).
    pub username: Option<String>,
    pub ssh_tunnel: Option<SshTunnelOptions>,
    pub ssh_password: Option<String>,
    pub ssh_key_passphrase: Option<String>,
    /// Set when the entry imported successfully but needs attention, e.g.
    /// a password that couldn't be carried over and must be re-entered.
    pub warning: Option<String>,
}

/// Detects the source format (Compass vs. NoSQLBooster) and parses
/// accordingly. Returns one `Ok` entry per importable connection; a
/// connection that can't be imported at all (missing connection info, or
/// protected by encryption we don't support reversing) becomes an `Err`
/// describing why, so the caller can still import the rest of the file.
pub fn parse_connections_file(raw: &str) -> AppResult<Vec<Result<ParsedImportEntry, String>>> {
    let sniff: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| AppError::InvalidInput(format!("not a recognized connections file: {e}")))?;

    let is_nosqlbooster = sniff
        .get("product")
        .and_then(|v| v.as_str())
        .map(|p| p.eq_ignore_ascii_case("NoSQLBooster"))
        .unwrap_or(false);

    if is_nosqlbooster {
        parse_nosqlbooster_connections(sniff)
    } else {
        parse_compass_connections(sniff)
    }
}

/// Parses a Compass-format connections file
/// (`{"type": "Compass Connections", "version": 1, "connections": [...]}`).
fn parse_compass_connections(
    parsed: serde_json::Value,
) -> AppResult<Vec<Result<ParsedImportEntry, String>>> {
    let parsed: ImportFile = serde_json::from_value(parsed)
        .map_err(|e| AppError::InvalidInput(format!("not a recognized connections file: {e}")))?;
    if parsed.file_type.as_deref() != Some(FILE_TYPE) {
        return Err(AppError::InvalidInput(
            "Unrecognized file format (expected a Compass or NoSQLBooster connections export)"
                .to_string(),
        ));
    }

    let results = parsed
        .connections
        .into_iter()
        .enumerate()
        .map(|(i, raw_entry)| parse_compass_one(i, raw_entry))
        .collect();
    Ok(results)
}

fn parse_compass_one(
    index: usize,
    raw_entry: serde_json::Value,
) -> Result<ParsedImportEntry, String> {
    let has_secrets = raw_entry
        .get("connectionSecrets")
        .map(|v| !v.is_null())
        .unwrap_or(false);

    let name = raw_entry
        .get("favorite")
        .and_then(|v| serde_json::from_value::<ImportedFavorite>(v.clone()).ok())
        .and_then(|f| f.name)
        .unwrap_or_else(|| format!("Imported connection {}", index + 1));

    if has_secrets {
        return Err(format!(
            "\"{name}\" is encrypted with a passphrase, which isn't supported yet - re-export from Compass without a passphrase and try again"
        ));
    }

    let options: Option<ImportedConnectionOptions> = raw_entry
        .get("connectionOptions")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let uri = options
        .as_ref()
        .and_then(|o| o.connection_string.clone())
        .ok_or_else(|| format!("\"{name}\" has no connection string"))?;

    let ssh = options.and_then(|o| o.ssh_tunnel);
    let (ssh_tunnel, ssh_password, ssh_key_passphrase) = match ssh {
        Some(s) => {
            let auth_method = if s.identity_key_file.is_some() {
                SshAuthMethod::PrivateKey
            } else {
                SshAuthMethod::Password
            };
            (
                Some(SshTunnelOptions {
                    enabled: true,
                    host: s.host.unwrap_or_default(),
                    port: s.port.unwrap_or(22),
                    username: s.username.unwrap_or_default(),
                    auth_method,
                    private_key_path: s.identity_key_file,
                    private_key_has_passphrase: s.identity_key_passphrase.is_some(),
                }),
                s.password,
                s.identity_key_passphrase,
            )
        }
        None => (None, None, None),
    };

    Ok(ParsedImportEntry {
        name,
        uri,
        username: None,
        ssh_tunnel,
        ssh_password,
        ssh_key_passphrase,
        warning: None,
    })
}

#[derive(Debug, Deserialize)]
struct NosqlBoosterFile {
    connections: Vec<NosqlBoosterConnection>,
}

#[derive(Debug, Deserialize)]
struct NosqlBoosterConnection {
    name: Option<String>,
    uri: Option<NosqlBoosterUri>,
}

#[derive(Debug, Deserialize)]
struct NosqlBoosterUri {
    hosts: Option<Vec<NosqlBoosterHost>>,
    options: Option<serde_json::Map<String, serde_json::Value>>,
    username: Option<String>,
    password: Option<String>,
    database: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NosqlBoosterHost {
    host: String,
    port: Option<u16>,
}

/// Parses a NoSQLBooster connections export
/// (`{"product": "NoSQLBooster", "connections": [{"uri": {"hosts": [...], ...}}]}`).
/// NoSQLBooster stores passwords encrypted with an app-internal scheme we
/// can't reverse, so a connection with a stored password imports without
/// one and comes back with a warning asking the user to re-enter it.
fn parse_nosqlbooster_connections(
    parsed: serde_json::Value,
) -> AppResult<Vec<Result<ParsedImportEntry, String>>> {
    let parsed: NosqlBoosterFile = serde_json::from_value(parsed)
        .map_err(|e| AppError::InvalidInput(format!("not a recognized connections file: {e}")))?;

    let results = parsed
        .connections
        .into_iter()
        .enumerate()
        .map(|(i, raw_entry)| parse_nosqlbooster_one(i, raw_entry))
        .collect();
    Ok(results)
}

fn parse_nosqlbooster_one(
    index: usize,
    entry: NosqlBoosterConnection,
) -> Result<ParsedImportEntry, String> {
    let name = entry
        .name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Imported connection {}", index + 1));

    let uri = entry
        .uri
        .ok_or_else(|| format!("\"{name}\" has no connection details"))?;

    let hosts = uri
        .hosts
        .filter(|h| !h.is_empty())
        .ok_or_else(|| format!("\"{name}\" has no hosts"))?;
    let host_list = hosts
        .iter()
        .map(|h| format!("{}:{}", h.host, h.port.unwrap_or(27017)))
        .collect::<Vec<_>>()
        .join(",");

    let path = uri
        .database
        .as_deref()
        .map(|db| format!("/{db}"))
        .unwrap_or_default();

    let query = uri
        .options
        .unwrap_or_default()
        .into_iter()
        .filter(|(key, _)| !key.is_empty())
        .filter_map(|(key, value)| {
            let value = value
                .as_str()
                .map(str::to_string)
                .unwrap_or(value.to_string());
            (!value.is_empty()).then(|| {
                format!(
                    "{}={}",
                    urlencoding::encode(&key),
                    urlencoding::encode(&value)
                )
            })
        })
        .collect::<Vec<_>>()
        .join("&");
    let query_suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    };

    let connection_uri = format!("mongodb://{host_list}{path}{query_suffix}");

    let warning = uri.password.is_some().then(|| {
        format!(
            "\"{name}\" imported without its password - NoSQLBooster stores passwords encrypted, which can't be carried over. Edit the connection to add it."
        )
    });

    Ok(ParsedImportEntry {
        name,
        uri: connection_uri,
        username: uri.username,
        ssh_tunnel: None,
        ssh_password: None,
        ssh_key_passphrase: None,
        warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConnectionAdvancedOptions, ConnectionSource, TlsOptions};
    use crate::secrets::{InMemoryStore, SecretStore};

    fn test_profile(id: &str, has_password: bool) -> ConnectionProfile {
        ConnectionProfile {
            id: id.to_string(),
            name: "My cluster".to_string(),
            color: None,
            environment: None,
            source: ConnectionSource::Uri {
                uri: "mongodb://cluster0.example.net/mydb".to_string(),
            },
            database: None,
            username: has_password.then(|| "alice".to_string()),
            has_password,
            tls: TlsOptions::default(),
            ssh_tunnel: None,
            advanced: ConnectionAdvancedOptions::default(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn export_omits_password_by_default() {
        let secrets = InMemoryStore::new();
        secrets.set("c1", SecretKind::Password, "s3cret").unwrap();
        let dir = std::env::temp_dir();
        let dest = dir.join("mongo-studio-conn-export-test-plain.json");

        export_connections(&[test_profile("c1", true)], false, &secrets, &dest).unwrap();
        let raw = std::fs::read_to_string(&dest).unwrap();
        std::fs::remove_file(&dest).ok();

        assert!(!raw.contains("s3cret"));
        assert!(raw.contains("\"type\": \"Compass Connections\""));
    }

    #[test]
    fn export_embeds_password_when_requested() {
        let secrets = InMemoryStore::new();
        secrets.set("c1", SecretKind::Password, "s3cret").unwrap();
        let dir = std::env::temp_dir();
        let dest = dir.join("mongo-studio-conn-export-test-secrets.json");

        export_connections(&[test_profile("c1", true)], true, &secrets, &dest).unwrap();
        let raw = std::fs::read_to_string(&dest).unwrap();
        std::fs::remove_file(&dest).ok();

        assert!(raw.contains("alice:s3cret@cluster0.example.net"));
    }

    #[test]
    fn parse_extracts_uri_and_ssh_tunnel() {
        let raw = r#"{
            "type": "Compass Connections",
            "version": 1,
            "connections": [{
                "id": "abc",
                "connectionOptions": {
                    "connectionString": "mongodb://cluster0.example.net/mydb",
                    "sshTunnel": {
                        "host": "bastion.example.net",
                        "port": 22,
                        "username": "deploy",
                        "password": "tunnelpass"
                    }
                },
                "favorite": { "name": "Prod" }
            }]
        }"#;

        let results = parse_connections_file(raw).unwrap();
        assert_eq!(results.len(), 1);
        let entry = results.into_iter().next().unwrap().unwrap();
        assert_eq!(entry.name, "Prod");
        assert_eq!(entry.uri, "mongodb://cluster0.example.net/mydb");
        assert_eq!(entry.ssh_password.as_deref(), Some("tunnelpass"));
        let ssh = entry.ssh_tunnel.unwrap();
        assert_eq!(ssh.host, "bastion.example.net");
        assert_eq!(ssh.username, "deploy");
    }

    #[test]
    fn parse_reports_error_for_encrypted_entry_but_keeps_the_rest() {
        let raw = r#"{
            "type": "Compass Connections",
            "version": 1,
            "connections": [
                {
                    "id": "locked",
                    "connectionOptions": { "connectionString": "mongodb://x/db" },
                    "favorite": { "name": "Locked" },
                    "connectionSecrets": "opaque-ciphertext"
                },
                {
                    "id": "open",
                    "connectionOptions": { "connectionString": "mongodb://y/db" },
                    "favorite": { "name": "Open" }
                }
            ]
        }"#;

        let results = parse_connections_file(raw).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].as_ref().unwrap_err().contains("passphrase"));
        assert_eq!(results[1].as_ref().unwrap().name, "Open");
    }

    #[test]
    fn parse_rejects_unrecognized_file_type() {
        let raw = r#"{"connections": []}"#;
        assert!(parse_connections_file(raw).is_err());
    }

    #[test]
    fn parse_nosqlbooster_builds_uri_from_hosts_and_options() {
        // Shape matches a real NoSQLBooster export: hosts/options/username/
        // password/database live under `uri`, and the stored password is an
        // opaque encrypted blob we can't use.
        let raw = r#"{
            "product": "NoSQLBooster",
            "connections": [{
                "id": "672bc5d02743aa736eed2070",
                "name": "TICKET (C002)",
                "connectionType": "replica",
                "uri": {
                    "scheme": "mongodb",
                    "hosts": [
                        { "host": "mongo-ticket-1.internal.svc", "port": 27017 },
                        { "host": "mongo-ticket-2.internal.svc", "port": 27017 }
                    ],
                    "options": { "replicaSet": "rs-ticket", "readPreference": "secondary" },
                    "username": "app",
                    "password": "c454d54d3fc87660c019f9bf7a4392c8",
                    "database": "ticket"
                },
                "authMode": 1,
                "certRelated": {},
                "editable": true
            }]
        }"#;

        let results = parse_connections_file(raw).unwrap();
        assert_eq!(results.len(), 1);
        let entry = results.into_iter().next().unwrap().unwrap();

        assert_eq!(entry.name, "TICKET (C002)");
        assert_eq!(entry.username.as_deref(), Some("app"));
        assert!(entry.uri.starts_with(
            "mongodb://mongo-ticket-1.internal.svc:27017,mongo-ticket-2.internal.svc:27017/ticket?"
        ));
        assert!(entry.uri.contains("replicaSet=rs-ticket"));
        assert!(entry.uri.contains("readPreference=secondary"));
        assert!(!entry.uri.contains("c454d54d3fc87660c019f9bf7a4392c8"));
        assert!(entry.warning.unwrap().contains("without its password"));
    }

    #[test]
    fn parse_nosqlbooster_no_auth_connection_has_no_warning() {
        let raw = r#"{
            "product": "NoSQLBooster",
            "connections": [{
                "id": "69668c483cbbf412b0da8fbe",
                "name": "localhost",
                "connectionType": "direct",
                "uri": {
                    "scheme": "mongodb",
                    "hosts": [{ "host": "localhost", "port": 27017 }],
                    "options": {}
                },
                "authMode": 0,
                "certRelated": {},
                "editable": true
            }]
        }"#;

        let results = parse_connections_file(raw).unwrap();
        let entry = results.into_iter().next().unwrap().unwrap();
        assert_eq!(entry.uri, "mongodb://localhost:27017");
        assert_eq!(entry.username, None);
        assert!(entry.warning.is_none());
    }
}

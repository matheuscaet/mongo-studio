use std::sync::Arc;

use futures_util::TryStreamExt;
use mongodb::bson::{doc, Document};
use mongodb::options::{
    ClientOptions, Credential, ReturnDocument, ServerAddress, Tls, TlsOptions as DriverTlsOptions,
};
use mongodb::Client;

use crate::connection::reinsert_uri_credentials;
use crate::ejson::{bson_to_json, document_to_json, json_to_document, json_to_pipeline};
use crate::error::{AppError, AppResult};
use crate::models::{
    self, CollectionInfo, CollectionStats, ConnectionAdvancedOptions, ConnectionProfile,
    ConnectionSource, ConnectionTestResult, DatabaseInfo, ExplainQueryInput, ExplainVerbosity,
    FindQueryInput, IndexInfo, QueryResultPage,
};
use crate::secrets::{SecretKind, SecretStore};
use crate::ssh_tunnel::{self, KnownHosts, SshTunnel, SshTunnelAuth, SshTunnelConfig};

pub struct ActiveConnection {
    pub client: Client,
    pub tunnel: Option<SshTunnel>,
    /// The connection's name, for log lines about this session.
    pub name: String,
    /// The saved profile the session was opened from.
    pub profile_id: String,
}

/// Builds a plain connection URI (no advanced overrides applied yet) from a
/// profile, reinserting credentials pulled from the secret store.
pub(crate) fn build_uri(
    profile: &ConnectionProfile,
    username: Option<&str>,
    password: Option<&str>,
) -> String {
    let base = match &profile.source {
        ConnectionSource::Uri { uri } => uri.clone(),
        ConnectionSource::Manual { host, port, srv } => {
            if *srv {
                format!("mongodb+srv://{host}/")
            } else {
                format!("mongodb://{host}:{port}/")
            }
        }
    };
    match (username, password) {
        (Some(user), Some(pass)) => reinsert_uri_credentials(&base, user, pass),
        _ => base,
    }
}

fn apply_tls_overrides(
    options: &mut ClientOptions,
    profile: &ConnectionProfile,
    secrets: &dyn SecretStore,
) -> AppResult<()> {
    if !profile.tls.enabled {
        return Ok(());
    }
    let passphrase = if profile.tls.cert_key_has_passphrase {
        secrets
            .get(&profile.id, SecretKind::TlsCertKeyPassphrase)
            .map_err(AppError::Secret)?
    } else {
        None
    };
    options.tls = Some(Tls::Enabled(
        DriverTlsOptions::builder()
            .allow_invalid_certificates(Some(profile.tls.allow_invalid_certificates))
            .ca_file_path(profile.tls.ca_file.clone().map(std::path::PathBuf::from))
            .cert_key_file_path(
                profile
                    .tls
                    .cert_key_file
                    .clone()
                    .map(std::path::PathBuf::from),
            )
            .tls_certificate_key_file_password(passphrase.map(|p| p.into_bytes()))
            .build(),
    ));
    Ok(())
}

fn apply_advanced_overrides(
    options: &mut ClientOptions,
    advanced: &ConnectionAdvancedOptions,
) -> AppResult<()> {
    if let Some(app_name) = &advanced.app_name {
        options.app_name = Some(app_name.clone());
    } else {
        options.app_name = Some("mongo-studio".to_string());
    }
    if let Some(ms) = advanced.connect_timeout_ms {
        options.connect_timeout = Some(std::time::Duration::from_millis(ms));
    }
    if let Some(ms) = advanced.server_selection_timeout_ms {
        options.server_selection_timeout = Some(std::time::Duration::from_millis(ms));
    }
    // A small pool per session rather than the driver's own defaults (min 0
    // / max 10). One connection isn't enough: the session serves the tree,
    // every tab and console on it, and a single slow query would hold up
    // all of them.
    options.max_pool_size = Some(advanced.max_pool_size.unwrap_or(4));
    options.min_pool_size = Some(advanced.min_pool_size.unwrap_or(1));
    if let Some(rs) = &advanced.replica_set {
        options.repl_set_name = Some(rs.clone());
    }
    if let Some(retry) = advanced.retry_writes {
        options.retry_writes = Some(retry);
    }
    if let Some(direct) = advanced.direct_connection {
        options.direct_connection = Some(direct);
    }
    if let Some(mechanism) = advanced.auth_mechanism {
        let driver_mechanism = map_auth_mechanism(mechanism)?;
        let credential = options.credential.get_or_insert_with(Credential::default);
        credential.mechanism = Some(driver_mechanism);
        if let Some(source) = &advanced.auth_source {
            credential.source = Some(source.clone());
        }
    } else if let Some(source) = &advanced.auth_source {
        let credential = options.credential.get_or_insert_with(Credential::default);
        credential.source = Some(source.clone());
    }
    Ok(())
}

/// Whether the connection's auth mechanism takes a password: SCRAM (the
/// default when none is set) and LDAP's PLAIN do; X.509, AWS and Kerberos
/// don't.
fn logs_in_with_password(options: &ClientOptions) -> bool {
    use mongodb::options::AuthMechanism as Driver;
    matches!(
        options
            .credential
            .as_ref()
            .and_then(|c| c.mechanism.as_ref()),
        None | Some(Driver::ScramSha1 | Driver::ScramSha256 | Driver::Plain)
    )
}

fn map_auth_mechanism(
    mechanism: models::AuthMechanism,
) -> AppResult<mongodb::options::AuthMechanism> {
    use models::AuthMechanism as Ours;
    use mongodb::options::AuthMechanism as Driver;
    match mechanism {
        Ours::ScramSha1 => Ok(Driver::ScramSha1),
        Ours::ScramSha256 => Ok(Driver::ScramSha256),
        Ours::MongodbX509 => Ok(Driver::MongoDbX509),
        Ours::MongodbAws => Ok(Driver::MongoDbAws),
        Ours::Gssapi => Err(AppError::InvalidInput(
            "Kerberos (GSSAPI) authentication requires a build with the gssapi-auth feature, \
             which mongo-studio does not enable by default because it pulls in a system \
             Kerberos/SASL dependency. See README for details."
                .to_string(),
        )),
    }
}

/// The profile's password from the secret store, when it has one.
fn saved_password(
    profile: &ConnectionProfile,
    secrets: &dyn SecretStore,
) -> AppResult<Option<String>> {
    if !profile.has_password {
        return Ok(None);
    }
    let saved = secrets
        .get(&profile.id, SecretKind::Password)
        .map_err(AppError::Secret)?;
    saved.map(Some).ok_or_else(|| {
        AppError::PasswordMissing(
            "The saved password for this connection is missing from the system keychain. \
             Edit the connection and enter it again."
                .to_string(),
        )
    })
}

/// A connection string for another MongoDB client (mongosh) that connects
/// the way `build_client_options` does: the saved credentials, the TLS
/// files and the advanced options as URI options, and through the session's
/// SSH tunnel when `tunnel_port` is set (straight to that local port, like
/// the driver).
pub(crate) fn shell_uri(
    profile: &ConnectionProfile,
    secrets: &dyn SecretStore,
    tunnel_port: Option<u16>,
) -> AppResult<String> {
    let password = saved_password(profile, secrets)?;
    let uri = build_uri(profile, profile.username.as_deref(), password.as_deref());
    let (scheme, rest) = uri
        .split_once("://")
        .ok_or_else(|| AppError::InvalidInput("invalid connection string".to_string()))?;
    let srv = scheme == "mongodb+srv";
    // the userinfo is percent-encoded, so the authority ends at the first `/` or `?`
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let (userinfo, mut hosts) = match authority.rsplit_once('@') {
        Some((user, hosts)) => (Some(user), hosts.to_string()),
        None => (None, authority.to_string()),
    };
    let tail = tail.strip_prefix('/').unwrap_or(tail);
    let (path, query) = tail.split_once('?').unwrap_or((tail, ""));

    let mut params: Vec<(String, String)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.to_string(), value.to_string())
        })
        .collect();
    let has = |params: &[(String, String)], key: &str| {
        params.iter().any(|(k, _)| k.eq_ignore_ascii_case(key))
    };
    let set = |params: &mut Vec<(String, String)>, key: &str, value: &str| {
        params.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
        params.push((key.to_string(), urlencoding::encode(value).into_owned()));
    };

    let advanced = &profile.advanced;
    set(
        &mut params,
        "appName",
        advanced.app_name.as_deref().unwrap_or("mongo-studio"),
    );
    if let Some(ms) = advanced.connect_timeout_ms {
        set(&mut params, "connectTimeoutMS", &ms.to_string());
    }
    if let Some(ms) = advanced.server_selection_timeout_ms {
        set(&mut params, "serverSelectionTimeoutMS", &ms.to_string());
    }
    if let Some(rs) = &advanced.replica_set {
        set(&mut params, "replicaSet", rs);
    }
    if let Some(retry) = advanced.retry_writes {
        set(&mut params, "retryWrites", &retry.to_string());
    }
    if let Some(direct) = advanced.direct_connection {
        set(&mut params, "directConnection", &direct.to_string());
    }
    if let Some(mechanism) = advanced.auth_mechanism {
        let name = match mechanism {
            models::AuthMechanism::ScramSha1 => "SCRAM-SHA-1",
            models::AuthMechanism::ScramSha256 => "SCRAM-SHA-256",
            models::AuthMechanism::MongodbX509 => "MONGODB-X509",
            models::AuthMechanism::MongodbAws => "MONGODB-AWS",
            models::AuthMechanism::Gssapi => "GSSAPI",
        };
        set(&mut params, "authMechanism", name);
    }
    if let Some(source) = &advanced.auth_source {
        set(&mut params, "authSource", source);
    }

    let tls = &profile.tls;
    if tls.enabled {
        set(&mut params, "tls", "true");
        if let Some(ca) = &tls.ca_file {
            set(&mut params, "tlsCAFile", ca);
        }
        if let Some(cert) = &tls.cert_key_file {
            set(&mut params, "tlsCertificateKeyFile", cert);
        }
        if tls.cert_key_has_passphrase {
            if let Some(passphrase) = secrets
                .get(&profile.id, SecretKind::TlsCertKeyPassphrase)
                .map_err(AppError::Secret)?
            {
                set(&mut params, "tlsCertificateKeyFilePassword", &passphrase);
            }
        }
        if tls.allow_invalid_certificates {
            set(&mut params, "tlsAllowInvalidCertificates", "true");
        }
        if tls.allow_invalid_hostnames {
            set(&mut params, "tlsAllowInvalidHostnames", "true");
        }
    }

    let mut scheme = scheme;
    if let Some(port) = tunnel_port {
        hosts = format!("127.0.0.1:{port}");
        if srv {
            // what the SRV lookup would have implied for the remote host
            scheme = "mongodb";
            if !has(&params, "tls") && !has(&params, "ssl") {
                set(&mut params, "tls", "true");
            }
            if !has(&params, "authSource") {
                set(&mut params, "authSource", "admin");
            }
        }
        params.retain(|(k, _)| !k.eq_ignore_ascii_case("replicaSet"));
        set(&mut params, "directConnection", "true");
    }

    let userinfo = userinfo.map(|u| format!("{u}@")).unwrap_or_default();
    let query = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    Ok(format!("{scheme}://{userinfo}{hosts}/{path}?{query}"))
}

/// Resolves credentials/secrets for a profile and builds a ready-to-use
/// `ClientOptions`, starting an SSH tunnel first if one is configured.
pub async fn build_client_options(
    profile: &ConnectionProfile,
    secrets: &dyn SecretStore,
    known_hosts: &Arc<KnownHosts>,
) -> AppResult<(ClientOptions, Option<SshTunnel>)> {
    let password = saved_password(profile, secrets)?;
    let uri = build_uri(profile, profile.username.as_deref(), password.as_deref());

    let mut options = ClientOptions::parse(&uri)
        .await
        .map_err(|e| AppError::InvalidInput(format!("invalid connection string: {e}")))?;

    apply_tls_overrides(&mut options, profile, secrets)?;
    apply_advanced_overrides(&mut options, &profile.advanced)?;

    // Without a password the URI goes out with no user at all, and the
    // server takes the connection as anonymous: the ping passes and the
    // first real command fails as unauthorized. Say what's missing instead.
    if let Some(user) = profile.username.as_deref().filter(|u| !u.is_empty()) {
        if password.is_none() && logs_in_with_password(&options) {
            return Err(AppError::PasswordMissing(format!(
                "No password is saved for user \"{user}\". Edit the connection and enter it."
            )));
        }
    }

    let tunnel = if let Some(ssh) = &profile.ssh_tunnel {
        if !ssh.enabled {
            None
        } else {
            let ServerAddress::Tcp { host, port } =
                options.hosts.first().cloned().ok_or_else(|| {
                    AppError::InvalidInput("connection has no hosts to tunnel to".to_string())
                })?
            else {
                return Err(AppError::InvalidInput(
                    "SSH tunneling to a Unix socket target is not supported".to_string(),
                ));
            };
            let remote_port = port.unwrap_or(27017);

            let auth = match ssh.auth_method {
                models::SshAuthMethod::Password => {
                    let pw = secrets
                        .get(&profile.id, SecretKind::SshPassword)
                        .map_err(AppError::Secret)?
                        .ok_or_else(|| {
                            AppError::InvalidInput("SSH password not set".to_string())
                        })?;
                    SshTunnelAuth::Password(pw)
                }
                models::SshAuthMethod::PrivateKey => {
                    let path = ssh.private_key_path.clone().ok_or_else(|| {
                        AppError::InvalidInput("SSH private key path not set".to_string())
                    })?;
                    let passphrase = if ssh.private_key_has_passphrase {
                        secrets
                            .get(&profile.id, SecretKind::SshKeyPassphrase)
                            .map_err(AppError::Secret)?
                    } else {
                        None
                    };
                    SshTunnelAuth::PrivateKey { path, passphrase }
                }
                models::SshAuthMethod::Agent => SshTunnelAuth::Agent,
            };

            let tunnel = ssh_tunnel::start_tunnel(
                SshTunnelConfig {
                    ssh_host: ssh.host.clone(),
                    ssh_port: ssh.port,
                    ssh_username: ssh.username.clone(),
                    auth,
                    remote_host: host,
                    remote_port,
                },
                known_hosts.clone(),
            )
            .await?;

            options.hosts = vec![ServerAddress::Tcp {
                host: "127.0.0.1".to_string(),
                port: Some(tunnel.local_addr.port()),
            }];
            options.direct_connection = Some(true);
            Some(tunnel)
        }
    } else {
        None
    };

    Ok((options, tunnel))
}

pub async fn connect(
    profile: &ConnectionProfile,
    secrets: &dyn SecretStore,
    known_hosts: &Arc<KnownHosts>,
) -> AppResult<ActiveConnection> {
    let (options, tunnel) = build_client_options(profile, secrets, known_hosts).await?;
    let client = Client::with_options(options)?;
    // `with_options` doesn't itself perform I/O; force a round trip now so
    // connect failures surface immediately instead of on the first query.
    client
        .database("admin")
        .run_command(doc! { "ping": 1 })
        .await?;
    Ok(ActiveConnection {
        client,
        tunnel,
        name: profile.name.clone(),
        profile_id: profile.id.clone(),
    })
}

pub async fn test_connection(
    profile: &ConnectionProfile,
    secrets: &dyn SecretStore,
    known_hosts: &Arc<KnownHosts>,
) -> ConnectionTestResult {
    let (options, tunnel) = match build_client_options(profile, secrets, known_hosts).await {
        Ok(v) => v,
        Err(e) => {
            return ConnectionTestResult {
                success: false,
                message: e.to_string(),
                server_version: None,
            }
        }
    };
    let result = async {
        let client = Client::with_options(options)?;
        let build_info = client
            .database("admin")
            .run_command(doc! { "buildInfo": 1 })
            .await?;
        let version = build_info.get_str("version").ok().map(str::to_string);
        Ok::<_, mongodb::error::Error>(version)
    }
    .await;

    if let Some(tunnel) = tunnel {
        tunnel.shutdown().await;
    }

    match result {
        Ok(version) => ConnectionTestResult {
            success: true,
            message: "Connected successfully".to_string(),
            server_version: version,
        },
        Err(e) => ConnectionTestResult {
            success: false,
            message: e.to_string(),
            server_version: None,
        },
    }
}

pub async fn list_databases(client: &Client) -> AppResult<Vec<DatabaseInfo>> {
    let dbs = client.list_databases().await?;
    Ok(dbs
        .into_iter()
        .map(|d| DatabaseInfo {
            name: d.name,
            size_on_disk: d.size_on_disk as i64,
        })
        .collect())
}

pub async fn list_collections(client: &Client, db: &str) -> AppResult<Vec<CollectionInfo>> {
    let collections: Vec<_> = client
        .database(db)
        .list_collections()
        .await?
        .try_collect()
        .await?;
    let mut collections: Vec<CollectionInfo> = collections
        .into_iter()
        .map(|c| CollectionInfo {
            name: c.name,
            collection_type: match c.collection_type {
                mongodb::results::CollectionType::View => "view",
                mongodb::results::CollectionType::Collection => "collection",
                mongodb::results::CollectionType::Timeseries => "timeseries",
                _ => "collection",
            }
            .to_string(),
        })
        .collect();
    sort_by_name(&mut collections);
    Ok(collections)
}

/// Orders collections by name the way people read a list: case doesn't
/// split `Orders` from `orders_archive`. The server returns them in no
/// particular order, which gets hard to scan in a database with many.
fn sort_by_name(collections: &mut [CollectionInfo]) {
    collections.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
}

pub async fn run_find(
    client: &Client,
    db: &str,
    collection: &str,
    input: &FindQueryInput,
) -> AppResult<QueryResultPage> {
    let filter = json_to_document(input.filter.clone())?;
    let coll = client.database(db).collection::<Document>(collection);
    let mut find = coll.find(filter);
    if let Some(sort) = &input.sort {
        find = find.sort(json_to_document(sort.clone())?);
    }
    if let Some(projection) = &input.projection {
        find = find.projection(json_to_document(projection.clone())?);
    }
    if let Some(limit) = input.limit {
        find = find.limit(limit);
    }
    if let Some(skip) = input.skip {
        find = find.skip(skip);
    }
    let docs: Vec<Document> = find.await?.try_collect().await?;
    let documents: Vec<_> = docs.into_iter().map(document_to_json).collect();
    let returned = documents.len();
    Ok(QueryResultPage {
        documents,
        returned,
    })
}

/// Joins a document field path for `$set`. Rejects `_id`, which MongoDB
/// won't change, and segments `$set` would read as something other than one
/// plain field name: empty, dotted, or starting with `$`.
pub fn field_path(segments: &[String]) -> AppResult<String> {
    let Some(first) = segments.first() else {
        return Err(AppError::InvalidInput("empty field path".to_string()));
    };
    if first == "_id" {
        return Err(AppError::InvalidInput("_id can't be changed".to_string()));
    }
    if let Some(bad) = segments
        .iter()
        .find(|s| s.is_empty() || s.contains('.') || s.starts_with('$'))
    {
        return Err(AppError::InvalidInput(format!(
            "field {bad:?} can't be edited in place"
        )));
    }
    Ok(segments.join("."))
}

/// Sets one field of the document with this `_id` and returns the document
/// as stored afterwards, so the caller shows exactly what was written. `id`
/// and `value` are Extended JSON, so BSON types survive the round trip.
pub async fn update_field(
    client: &Client,
    db: &str,
    collection: &str,
    id: serde_json::Value,
    path: &[String],
    value: serde_json::Value,
) -> AppResult<serde_json::Value> {
    let path = field_path(path)?;
    let filter = json_to_document(serde_json::json!({ "_id": id }))?;
    let value = json_to_document(serde_json::json!({ "value": value }))?
        .remove("value")
        .ok_or_else(|| AppError::InvalidInput("missing value".to_string()))?;
    let mut set = Document::new();
    set.insert(path, value);

    let updated = client
        .database(db)
        .collection::<Document>(collection)
        .find_one_and_update(filter, doc! { "$set": set })
        .return_document(ReturnDocument::After)
        .await?
        .ok_or_else(|| {
            AppError::NotFound("the document no longer exists in the collection".to_string())
        })?;
    Ok(document_to_json(updated))
}

pub async fn run_aggregate(
    client: &Client,
    db: &str,
    collection: &str,
    pipeline: serde_json::Value,
) -> AppResult<QueryResultPage> {
    let stages = json_to_pipeline(pipeline)?;
    let docs: Vec<Document> = client
        .database(db)
        .collection::<Document>(collection)
        .aggregate(stages)
        .await?
        .try_collect()
        .await?;
    let documents: Vec<_> = docs.into_iter().map(document_to_json).collect();
    let returned = documents.len();
    Ok(QueryResultPage {
        documents,
        returned,
    })
}

pub async fn count_documents(
    client: &Client,
    db: &str,
    collection: &str,
    filter: serde_json::Value,
) -> AppResult<u64> {
    let filter = json_to_document(filter)?;
    Ok(client
        .database(db)
        .collection::<Document>(collection)
        .count_documents(filter)
        .await?)
}

pub async fn insert_one(
    client: &Client,
    db: &str,
    collection: &str,
    document: serde_json::Value,
) -> AppResult<serde_json::Value> {
    let document = json_to_document(document)?;
    let result = client
        .database(db)
        .collection::<Document>(collection)
        .insert_one(document)
        .await?;
    Ok(bson_to_json(result.inserted_id))
}

pub async fn update_one(
    client: &Client,
    db: &str,
    collection: &str,
    filter: serde_json::Value,
    update: serde_json::Value,
) -> AppResult<serde_json::Value> {
    let filter = json_to_document(filter)?;
    let update = json_to_document(update)?;
    let result = client
        .database(db)
        .collection::<Document>(collection)
        .update_one(filter, update)
        .await?;
    Ok(serde_json::json!({
        "matchedCount": result.matched_count,
        "modifiedCount": result.modified_count,
        "upsertedId": result.upserted_id.map(bson_to_json),
    }))
}

pub async fn delete_one(
    client: &Client,
    db: &str,
    collection: &str,
    filter: serde_json::Value,
) -> AppResult<serde_json::Value> {
    let filter = json_to_document(filter)?;
    let result = client
        .database(db)
        .collection::<Document>(collection)
        .delete_one(filter)
        .await?;
    Ok(serde_json::json!({ "deletedCount": result.deleted_count }))
}

/// Deletes the documents with these `_id`s (Extended JSON); returns how many
/// went. Fewer than asked means some were already gone.
pub async fn delete_documents(
    client: &Client,
    db: &str,
    collection: &str,
    ids: Vec<serde_json::Value>,
) -> AppResult<u64> {
    if ids.is_empty() {
        return Err(AppError::InvalidInput("no documents to delete".to_string()));
    }
    let filter = json_to_document(serde_json::json!({ "_id": { "$in": ids } }))?;
    let result = client
        .database(db)
        .collection::<Document>(collection)
        .delete_many(filter)
        .await?;
    Ok(result.deleted_count)
}

/// Inserts every document of a JSON array; `insertedIds` keeps their order.
pub async fn insert_many(
    client: &Client,
    db: &str,
    collection: &str,
    documents: serde_json::Value,
) -> AppResult<serde_json::Value> {
    let serde_json::Value::Array(items) = documents else {
        return Err(AppError::InvalidInput(
            "insertMany expects an array of documents".to_string(),
        ));
    };
    if items.is_empty() {
        return Err(AppError::InvalidInput(
            "insertMany needs at least one document".to_string(),
        ));
    }
    let documents = items
        .into_iter()
        .map(json_to_document)
        .collect::<AppResult<Vec<_>>>()?;
    let result = client
        .database(db)
        .collection::<Document>(collection)
        .insert_many(documents)
        .await?;
    let mut ids: Vec<_> = result.inserted_ids.into_iter().collect();
    ids.sort_by_key(|(index, _)| *index);
    let inserted_ids: Vec<_> = ids.into_iter().map(|(_, id)| bson_to_json(id)).collect();
    Ok(serde_json::json!({
        "insertedCount": inserted_ids.len(),
        "insertedIds": inserted_ids,
    }))
}

pub async fn update_many(
    client: &Client,
    db: &str,
    collection: &str,
    filter: serde_json::Value,
    update: serde_json::Value,
) -> AppResult<serde_json::Value> {
    let filter = json_to_document(filter)?;
    let update = json_to_document(update)?;
    let result = client
        .database(db)
        .collection::<Document>(collection)
        .update_many(filter, update)
        .await?;
    Ok(serde_json::json!({
        "matchedCount": result.matched_count,
        "modifiedCount": result.modified_count,
        "upsertedId": result.upserted_id.map(bson_to_json),
    }))
}

pub async fn delete_many(
    client: &Client,
    db: &str,
    collection: &str,
    filter: serde_json::Value,
) -> AppResult<serde_json::Value> {
    let filter = json_to_document(filter)?;
    let result = client
        .database(db)
        .collection::<Document>(collection)
        .delete_many(filter)
        .await?;
    Ok(serde_json::json!({ "deletedCount": result.deleted_count }))
}

pub async fn get_collection_stats(
    client: &Client,
    db: &str,
    collection: &str,
) -> AppResult<CollectionStats> {
    let coll = client.database(db).collection::<Document>(collection);
    // From the collection's metadata, so it answers at once. An exact
    // count_documents scans the whole collection, which takes minutes on a
    // large one and holds the session's connection all that time.
    let document_count = coll.estimated_document_count().await?;
    let indexes = list_indexes(client, db, collection).await?;
    Ok(CollectionStats {
        document_count,
        indexes,
    })
}

pub async fn list_indexes(
    client: &Client,
    db: &str,
    collection: &str,
) -> AppResult<Vec<IndexInfo>> {
    let models: Vec<_> = client
        .database(db)
        .collection::<Document>(collection)
        .list_indexes()
        .await?
        .try_collect()
        .await?;
    Ok(models
        .into_iter()
        .map(|m| {
            let name = m
                .options
                .as_ref()
                .and_then(|o| o.name.clone())
                .unwrap_or_default();
            let unique = m.options.as_ref().and_then(|o| o.unique).unwrap_or(false);
            IndexInfo {
                name,
                key: document_to_json(m.keys),
                unique,
            }
        })
        .collect())
}

/// Per-index usage counters via the `$indexStats` aggregation stage: how
/// many read/write ops have used each index since the server started (or
/// since the index was created), which is what actually answers "is this
/// index doing anything?" rather than just listing index shapes.
pub async fn list_index_stats(
    client: &Client,
    db: &str,
    collection: &str,
) -> AppResult<Vec<serde_json::Value>> {
    let docs: Vec<Document> = client
        .database(db)
        .collection::<Document>(collection)
        .aggregate(vec![doc! { "$indexStats": {} }])
        .await?
        .try_collect()
        .await?;
    Ok(docs.into_iter().map(document_to_json).collect())
}

/// Runs the MongoDB `explain` command over a find or (when `query.pipeline`
/// is set) an aggregate, at the requested verbosity. Built by hand via
/// `run_command` rather than a driver-native helper, since the Rust driver
/// doesn't expose one - this is also more portable, since `explain` wraps
/// arbitrary commands the same way for any query shape.
pub async fn explain_query(
    client: &Client,
    db: &str,
    collection: &str,
    query: &ExplainQueryInput,
    verbosity: ExplainVerbosity,
) -> AppResult<serde_json::Value> {
    let inner_command = if let Some(pipeline) = &query.pipeline {
        let stages = json_to_pipeline(pipeline.clone())?;
        doc! {
            "aggregate": collection,
            "pipeline": stages,
            "cursor": {},
        }
    } else {
        let filter = json_to_document(query.filter.clone())?;
        let mut cmd = doc! {
            "find": collection,
            "filter": filter,
        };
        if let Some(sort) = &query.sort {
            cmd.insert("sort", json_to_document(sort.clone())?);
        }
        if let Some(projection) = &query.projection {
            cmd.insert("projection", json_to_document(projection.clone())?);
        }
        if let Some(limit) = query.limit {
            cmd.insert("limit", limit);
        }
        if let Some(skip) = query.skip {
            cmd.insert("skip", skip as i64);
        }
        cmd
    };

    let verbosity_str = match verbosity {
        ExplainVerbosity::QueryPlanner => "queryPlanner",
        ExplainVerbosity::ExecutionStats => "executionStats",
        ExplainVerbosity::AllPlansExecution => "allPlansExecution",
    };
    let explain_command = doc! {
        "explain": inner_command,
        "verbosity": verbosity_str,
    };

    let result = client.database(db).run_command(explain_command).await?;
    Ok(document_to_json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AuthMechanism, ConnectionAdvancedOptions, TlsOptions};
    use crate::secrets::InMemoryStore;

    fn segments(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    fn profile(username: Option<&str>, has_password: bool) -> ConnectionProfile {
        ConnectionProfile {
            id: "conn".to_string(),
            name: "test".to_string(),
            color: None,
            source: ConnectionSource::Uri {
                uri: "mongodb://localhost:27017/".to_string(),
            },
            database: None,
            username: username.map(str::to_string),
            has_password,
            tls: TlsOptions::default(),
            ssh_tunnel: None,
            advanced: ConnectionAdvancedOptions::default(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    async fn options_for(
        profile: &ConnectionProfile,
        secrets: &InMemoryStore,
    ) -> AppResult<ClientOptions> {
        let known_hosts = Arc::new(
            KnownHosts::load(&std::env::temp_dir().join("mongo-studio-driver-test")).unwrap(),
        );
        build_client_options(profile, secrets, &known_hosts)
            .await
            .map(|(options, _)| options)
    }

    #[tokio::test]
    async fn a_password_missing_from_the_keychain_fails_instead_of_going_anonymous() {
        let err = options_for(&profile(Some("alice"), true), &InMemoryStore::new())
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::PasswordMissing(_)), "{err}");
        assert!(err.to_string().contains("keychain"));
    }

    #[tokio::test]
    async fn a_user_without_a_password_fails_unless_the_mechanism_needs_none() {
        let secrets = InMemoryStore::new();
        let err = options_for(&profile(Some("alice"), false), &secrets)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::PasswordMissing(_)), "{err}");
        assert!(err.to_string().contains("\"alice\""));

        let mut x509 = profile(Some("CN=alice"), false);
        x509.advanced.auth_mechanism = Some(AuthMechanism::MongodbX509);
        assert!(options_for(&x509, &secrets).await.is_ok());

        assert!(options_for(&profile(None, false), &secrets).await.is_ok());
    }

    #[tokio::test]
    async fn a_saved_password_goes_into_the_credential() {
        let secrets = InMemoryStore::new();
        secrets.set("conn", SecretKind::Password, "p@ss").unwrap();
        let options = options_for(&profile(Some("alice"), true), &secrets)
            .await
            .unwrap();
        let credential = options.credential.unwrap();
        assert_eq!(credential.username.as_deref(), Some("alice"));
        assert_eq!(credential.password.as_deref(), Some("p@ss"));
    }

    #[test]
    fn collections_sort_by_name_ignoring_case() {
        let mut collections: Vec<CollectionInfo> = ["users", "Orders", "audit", "orders", "Zones"]
            .iter()
            .map(|name| CollectionInfo {
                name: name.to_string(),
                collection_type: "collection".to_string(),
            })
            .collect();

        sort_by_name(&mut collections);

        let names: Vec<&str> = collections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["audit", "Orders", "orders", "users", "Zones"]);
    }

    #[test]
    fn field_path_joins_nested_and_array_segments() {
        assert_eq!(
            field_path(&segments(&["props", "path"])).unwrap(),
            "props.path"
        );
        assert_eq!(
            field_path(&segments(&["items", "0", "qty"])).unwrap(),
            "items.0.qty"
        );
    }

    #[test]
    fn field_path_rejects_what_set_would_misread() {
        assert!(field_path(&[]).is_err());
        assert!(field_path(&segments(&["_id"])).is_err());
        assert!(field_path(&segments(&["a", ""])).is_err());
        assert!(field_path(&segments(&["a.b"])).is_err());
        assert!(field_path(&segments(&["$where"])).is_err());
        // a nested field named _id is an ordinary field
        assert!(field_path(&segments(&["ref", "_id"])).is_ok());
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::connection::extract_uri_credentials;
    use crate::models::{ConnectionAdvancedOptions, TlsOptions};
    use crate::secrets::{InMemoryStore, SecretKind, SecretStore};

    async fn live_connect() -> ActiveConnection {
        let uri = std::env::var("MONGO_STUDIO_TEST_URI")
            .expect("set MONGO_STUDIO_TEST_URI to run this test");
        let (stripped_uri, username, password) = extract_uri_credentials(&uri);

        let secrets = InMemoryStore::new();
        let profile_id = "live-test".to_string();
        if let Some(password) = &password {
            secrets
                .set(&profile_id, SecretKind::Password, password)
                .unwrap();
        }

        let profile = ConnectionProfile {
            id: profile_id,
            name: "live test".to_string(),
            color: None,
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
            KnownHosts::load(&std::env::temp_dir().join("mongo-studio-live-test")).unwrap(),
        );
        connect(&profile, &secrets, &known_hosts)
            .await
            .expect("connect should succeed")
    }

    /// Exercises the real Phase 2 connect path against a live MongoDB
    /// instance. Skipped by default; run explicitly with:
    ///   set -a; source ../.env.local; set +a
    ///   cargo test --lib -- --ignored live_connect_lists_databases
    #[tokio::test]
    #[ignore]
    async fn live_connect_lists_databases() {
        let active = live_connect().await;
        let dbs = list_databases(&active.client)
            .await
            .expect("list_databases should succeed");
        assert!(
            !dbs.is_empty(),
            "expected at least one database visible to this user"
        );
        println!(
            "databases: {:?}",
            dbs.iter().map(|d| &d.name).collect::<Vec<_>>()
        );
    }

    /// Exercises the Phase 3 query path (list_collections, find, aggregate,
    /// count, stats/indexes) against whatever the first database/collection
    /// visible to the test user happens to contain. Skipped by default; run
    /// explicitly with:
    ///   set -a; source ../.env.local; set +a
    ///   cargo test --lib -- --ignored live_query_flow
    #[tokio::test]
    #[ignore]
    async fn live_query_flow() {
        let active = live_connect().await;
        let dbs = list_databases(&active.client).await.unwrap();
        let db = dbs
            .iter()
            .find(|d| d.name != "admin" && d.name != "local")
            .expect("expected at least one non-system database");

        let collections = list_collections(&active.client, &db.name).await.unwrap();
        let Some(collection) = collections.first() else {
            println!("database {} has no collections, skipping", db.name);
            return;
        };

        let stats = get_collection_stats(&active.client, &db.name, &collection.name)
            .await
            .unwrap();
        println!(
            "{}.{}: {} documents, {} indexes",
            db.name,
            collection.name,
            stats.document_count,
            stats.indexes.len()
        );

        let page = run_find(
            &active.client,
            &db.name,
            &collection.name,
            &FindQueryInput {
                filter: serde_json::json!({}),
                sort: None,
                projection: None,
                limit: Some(2),
                skip: None,
            },
        )
        .await
        .unwrap();
        println!("find returned {} document(s)", page.returned);

        let count = count_documents(
            &active.client,
            &db.name,
            &collection.name,
            serde_json::json!({}),
        )
        .await
        .unwrap();
        assert_eq!(count, stats.document_count);

        let agg = run_aggregate(
            &active.client,
            &db.name,
            &collection.name,
            serde_json::json!([{ "$limit": 1 }]),
        )
        .await
        .unwrap();
        assert!(agg.returned <= 1);
    }

    /// Exercises explain (both find- and aggregate-shaped) and $indexStats
    /// against whatever the first database/collection contains. Skipped by
    /// default; run explicitly with:
    ///   set -a; source ../.env.local; set +a
    ///   cargo test --lib -- --ignored live_update_field
    #[tokio::test]
    #[ignore]
    async fn live_update_field() {
        let active = live_connect().await;
        let coll_name = format!("update_field_{}", uuid::Uuid::new_v4().simple());
        let coll = active
            .client
            .database("mongo_studio_test")
            .collection::<Document>(&coll_name);
        let oid = mongodb::bson::oid::ObjectId::new();
        coll.insert_one(doc! {
            "_id": oid,
            "props": { "path": "/products", "durationMs": 10 },
            "items": [ { "qty": 1 } ],
        })
        .await
        .unwrap();

        let id = serde_json::json!({ "$oid": oid.to_hex() });
        let updated = update_field(
            &active.client,
            "mongo_studio_test",
            &coll_name,
            id.clone(),
            &["props".to_string(), "path".to_string()],
            serde_json::json!("/cart"),
        )
        .await
        .unwrap();
        assert_eq!(updated["props"]["path"], "/cart");
        assert_eq!(updated["props"]["durationMs"], 10, "siblings untouched");

        let updated = update_field(
            &active.client,
            "mongo_studio_test",
            &coll_name,
            id,
            &["items".to_string(), "0".to_string(), "qty".to_string()],
            serde_json::json!(5),
        )
        .await
        .unwrap();
        assert_eq!(updated["items"][0]["qty"], 5);

        let stored = coll.find_one(doc! { "_id": oid }).await.unwrap().unwrap();
        assert_eq!(
            stored
                .get_document("props")
                .unwrap()
                .get_str("path")
                .unwrap(),
            "/cart"
        );
        coll.drop().await.unwrap();
    }

    ///   cargo test --lib -- --ignored live_explain_and_index_stats
    #[tokio::test]
    #[ignore]
    async fn live_explain_and_index_stats() {
        let active = live_connect().await;
        let dbs = list_databases(&active.client).await.unwrap();
        let db = dbs
            .iter()
            .find(|d| d.name != "admin" && d.name != "local")
            .expect("expected at least one non-system database");
        let collections = list_collections(&active.client, &db.name).await.unwrap();
        let Some(collection) = collections.first() else {
            println!("database {} has no collections, skipping", db.name);
            return;
        };

        // Some Atlas tiers/roles don't grant the $indexStats privilege; that's
        // a real, expected permissions boundary (not a bug here), so this
        // just documents the behavior instead of asserting success.
        match list_index_stats(&active.client, &db.name, &collection.name).await {
            Ok(stats) => {
                println!("index stats: {} entries", stats.len());
                assert!(
                    !stats.is_empty(),
                    "every collection has at least the _id index"
                );
            }
            Err(e) => println!(
                "$indexStats not permitted for this user (expected on some Atlas tiers): {e}"
            ),
        }

        let find_explain = explain_query(
            &active.client,
            &db.name,
            &collection.name,
            &ExplainQueryInput {
                filter: serde_json::json!({}),
                sort: None,
                projection: None,
                limit: Some(1),
                skip: None,
                pipeline: None,
            },
            ExplainVerbosity::ExecutionStats,
        )
        .await
        .unwrap();
        assert!(
            find_explain.get("queryPlanner").is_some(),
            "find explain should include queryPlanner: {find_explain}"
        );
        assert!(
            find_explain.get("executionStats").is_some(),
            "executionStats verbosity should include executionStats: {find_explain}"
        );

        let agg_explain = explain_query(
            &active.client,
            &db.name,
            &collection.name,
            &ExplainQueryInput {
                filter: serde_json::json!({}),
                sort: None,
                projection: None,
                limit: None,
                skip: None,
                pipeline: Some(serde_json::json!([{ "$limit": 1 }])),
            },
            ExplainVerbosity::QueryPlanner,
        )
        .await
        .unwrap();
        println!("aggregate explain: {agg_explain}");
        assert!(agg_explain.is_object());
    }
}

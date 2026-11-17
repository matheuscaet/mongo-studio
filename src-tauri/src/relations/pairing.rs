//! Which connection the other side of a relation lives on.
//!
//! Two saved connections are on the same cluster when their connection
//! strings name the same hosts. A profile's databases can't be known
//! offline, so among the connections to the cluster the one whose default
//! database is the other side's wins, then the one named after it, then any.

use crate::connection::{extract_uri_credentials, ConnectionStore};
use crate::models::{ConnectionProfile, ConnectionSource};

use super::model::{Dir, Relation};

const DEFAULT_PORT: &str = "27017";

/// The hosts part of a connection string (`cluster003.mongodb.net`,
/// `a:27017,b:27017`), credentials and options left out.
///
/// Lowercased and sorted, so `a,b` and `B,a` are the same cluster; a
/// `mongodb://` host without a port gets `:27017`, so `localhost` and
/// `localhost:27017` are too. Empty for anything that isn't a MongoDB URI.
pub fn host_of(uri: &str) -> String {
    let uri = uri.trim();
    let (rest, srv) = if let Some(rest) = uri.strip_prefix("mongodb+srv://") {
        (rest, true)
    } else if let Some(rest) = uri.strip_prefix("mongodb://") {
        (rest, false)
    } else {
        return String::new();
    };
    let (stripped, _, _) = extract_uri_credentials(&format!("mongodb://{rest}"));
    let rest = &stripped["mongodb://".len()..];
    let authority = &rest[..rest.find(['/', '?']).unwrap_or(rest.len())];
    let mut hosts: Vec<String> = authority
        .split(',')
        .map(|h| h.trim().to_lowercase())
        .filter(|h| !h.is_empty())
        .map(|h| {
            // `[::1]:27017` has colons of its own: only a colon after the
            // closing bracket (or in a plain name) is a port.
            let has_port = match h.rfind(']') {
                Some(end) => h[end..].contains(':'),
                None => h.contains(':'),
            };
            if srv || has_port {
                h
            } else {
                format!("{h}:{DEFAULT_PORT}")
            }
        })
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts.join(",")
}

/// The hosts of a saved connection, as `host_of` gives them.
fn hosts_of_profile(profile: &ConnectionProfile) -> String {
    match &profile.source {
        ConnectionSource::Uri { uri } => host_of(uri),
        ConnectionSource::Manual {
            host, srv: true, ..
        } => host_of(&format!("mongodb+srv://{host}")),
        ConnectionSource::Manual { host, port, .. } => host_of(&format!("mongodb://{host}:{port}")),
    }
}

/// The database a saved connection opens: its own setting, else the path of
/// its connection string.
fn default_db(profile: &ConnectionProfile) -> Option<String> {
    if let Some(db) = profile.database.as_deref().filter(|d| !d.is_empty()) {
        return Some(db.to_string());
    }
    let ConnectionSource::Uri { uri } = &profile.source else {
        return None;
    };
    let rest = &uri[uri.find("://")? + 3..];
    let path = &rest[rest.find('/')? + 1..];
    let db = &path[..path.find('?').unwrap_or(path.len())];
    (!db.is_empty()).then(|| {
        urlencoding::decode(db)
            .map(|d| d.into_owned())
            .unwrap_or(db.into())
    })
}

/// The connection to the same cluster as `source` for database `db`:
/// another one whose default database or name says it holds `db`, else
/// `source` itself when its default database is `db` or it's the only
/// connection to that cluster, else any other one to the cluster.
fn cluster_mate(connections: &ConnectionStore, source: &str, db: &str) -> Option<String> {
    let profiles = connections.list();
    let me = profiles.iter().find(|p| p.id == source)?;
    let hosts = hosts_of_profile(me);
    if hosts.is_empty() {
        return Some(source.to_string());
    }
    let mates: Vec<&ConnectionProfile> = profiles
        .iter()
        .filter(|p| p.id != source && hosts_of_profile(p) == hosts)
        .collect();
    let db_lower = db.to_lowercase();
    mates
        .iter()
        .find(|p| default_db(p).as_deref() == Some(db))
        .or_else(|| {
            mates
                .iter()
                .find(|p| !db.is_empty() && p.name.to_lowercase().contains(&db_lower))
        })
        .map(|p| p.id.clone())
        .or_else(|| {
            let mine = default_db(me).as_deref() == Some(db);
            if mine || mates.is_empty() {
                Some(source.to_string())
            } else {
                mates.first().map(|p| p.id.clone())
            }
        })
}

/// The connection the other side lives on, for documents of `source_conn`
/// on side `dir`: the relation's pair for it, else the connection itself
/// ("same"), a fixed one, or the connection to the same cluster (same hosts)
/// holding the other side's database. None when a pair skips it or nothing fits.
pub fn target_connection(
    connections: &ConnectionStore,
    relation: &Relation,
    dir: Dir,
    source_conn: &str,
) -> Option<String> {
    match dir {
        Dir::From => {
            if let Some(pair) = relation.pairs.get(source_conn) {
                return pair.clone();
            }
            match relation.to.conn.as_str() {
                "same" => Some(source_conn.to_string()),
                "cluster" | "any" | "" => cluster_mate(connections, source_conn, &relation.to.db),
                id => Some(id.to_string()),
            }
        }
        Dir::To => {
            let mut back: Vec<&String> = relation
                .pairs
                .iter()
                .filter(|(_, to)| to.as_deref() == Some(source_conn))
                .map(|(from, _)| from)
                .collect();
            // HashMap order isn't stable: the smallest id, so the answer is.
            back.sort();
            if let Some(from) = back.first() {
                return Some((*from).clone());
            }
            match relation.from.conn.as_str() {
                "any" | "" => match relation.to.conn.as_str() {
                    "same" => Some(source_conn.to_string()),
                    _ => cluster_mate(connections, source_conn, &relation.from.db),
                },
                id => Some(id.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConnectionAdvancedOptions, TlsOptions};
    use crate::relations::store::tests::relation;

    fn profile(
        id: &str,
        name: &str,
        source: ConnectionSource,
        db: Option<&str>,
    ) -> ConnectionProfile {
        ConnectionProfile {
            id: id.to_string(),
            name: name.to_string(),
            color: None,
            environment: None,
            source,
            database: db.map(str::to_string),
            username: None,
            has_password: false,
            tls: TlsOptions::default(),
            ssh_tunnel: None,
            advanced: ConnectionAdvancedOptions::default(),
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        }
    }

    fn uri(u: &str) -> ConnectionSource {
        ConnectionSource::Uri { uri: u.to_string() }
    }

    fn store(profiles: Vec<ConnectionProfile>) -> (ConnectionStore, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("mongo-studio-pairing-{}", uuid::Uuid::new_v4()));
        let store = ConnectionStore::load(&dir).unwrap();
        for p in profiles {
            store.upsert(p).unwrap();
        }
        (store, dir)
    }

    #[test]
    fn hosts_of_srv_multi_host_and_manual_connections() {
        assert_eq!(
            host_of("mongodb+srv://ana:s3cret@Cluster003.abcde.mongodb.net/shop?retryWrites=true"),
            "cluster003.abcde.mongodb.net"
        );
        assert_eq!(
            host_of("mongodb://u:p@b.example:27017,A.example:27018/db?replicaSet=rs0"),
            "a.example:27018,b.example:27017"
        );
        assert_eq!(
            host_of("mongodb://a.example:27018,b.example"),
            host_of("mongodb://b.example:27017,a.example:27018/other")
        );
        assert_eq!(host_of("mongodb://localhost"), "localhost:27017");
        assert_eq!(host_of("mongodb://[::1]:27018"), "[::1]:27018");
        assert_eq!(host_of("mongodb://[::1]"), "[::1]:27017");
        assert_eq!(host_of("http://localhost"), "");

        let manual = profile(
            "m",
            "Manual",
            ConnectionSource::Manual {
                host: "LocalHost".into(),
                port: 27017,
                srv: false,
            },
            None,
        );
        assert_eq!(hosts_of_profile(&manual), host_of("mongodb://localhost/"));
        let manual_srv = profile(
            "s",
            "Srv",
            ConnectionSource::Manual {
                host: "cluster0.example.net".into(),
                port: 27017,
                srv: true,
            },
            None,
        );
        assert_eq!(
            hosts_of_profile(&manual_srv),
            host_of("mongodb+srv://cluster0.example.net")
        );
    }

    #[test]
    fn default_database_from_the_profile_or_the_uri_path() {
        let p = profile("a", "A", uri("mongodb://h/logistics?x=1"), None);
        assert_eq!(default_db(&p).as_deref(), Some("logistics"));
        let p = profile("a", "A", uri("mongodb://h/shop"), Some("other"));
        assert_eq!(default_db(&p).as_deref(), Some("other"));
        let p = profile("a", "A", uri("mongodb://h/?x=1"), None);
        assert_eq!(default_db(&p), None);
    }

    #[test]
    fn target_connection_same_fixed_cluster_pairs_and_inverse() {
        let (connections, dir) = store(vec![
            profile(
                "shop3",
                "003 shop",
                uri("mongodb+srv://c3.example.net"),
                Some("shop"),
            ),
            profile(
                "misc3",
                "003 misc",
                uri("mongodb+srv://c3.example.net"),
                None,
            ),
            profile(
                "named3",
                "003 Logistics",
                uri("mongodb+srv://C3.example.net/"),
                None,
            ),
            profile(
                "logistics3",
                "003 billing",
                uri("mongodb+srv://c3.example.net/logistics"),
                None,
            ),
            profile(
                "shop4",
                "004 shop",
                uri("mongodb+srv://c4.example.net"),
                Some("shop"),
            ),
            profile(
                "other4",
                "004 something",
                uri("mongodb+srv://c4.example.net"),
                None,
            ),
            profile("alone", "alone", uri("mongodb://alone:27017"), None),
        ]);
        let mut r = relation("Shipments");
        // to.db is "logistics": the one whose default database is it wins...
        assert_eq!(
            target_connection(&connections, &r, Dir::From, "shop3").as_deref(),
            Some("logistics3")
        );
        // ...then any other connection to the cluster.
        assert_eq!(
            target_connection(&connections, &r, Dir::From, "shop4").as_deref(),
            Some("other4")
        );
        // No other connection to that cluster: the cluster is its own.
        assert_eq!(
            target_connection(&connections, &r, Dir::From, "alone").as_deref(),
            Some("alone")
        );

        // The name, when no default database matches.
        connections.delete("logistics3").unwrap();
        assert_eq!(
            target_connection(&connections, &r, Dir::From, "shop3").as_deref(),
            Some("named3")
        );

        r.to.conn = "same".into();
        assert_eq!(
            target_connection(&connections, &r, Dir::From, "shop3").as_deref(),
            Some("shop3")
        );
        r.to.conn = "misc3".into();
        assert_eq!(
            target_connection(&connections, &r, Dir::From, "shop4").as_deref(),
            Some("misc3")
        );

        // A pair overrides; an explicit null skips the connection.
        r.to.conn = "cluster".into();
        r.pairs.insert("shop3".into(), Some("misc3".into()));
        r.pairs.insert("shop4".into(), None);
        assert_eq!(
            target_connection(&connections, &r, Dir::From, "shop3").as_deref(),
            Some("misc3")
        );
        assert_eq!(
            target_connection(&connections, &r, Dir::From, "shop4"),
            None
        );

        // Backwards: the pair read the other way...
        assert_eq!(
            target_connection(&connections, &r, Dir::To, "misc3").as_deref(),
            Some("shop3")
        );
        // ...else the cluster toward from.db ("shop").
        assert_eq!(
            target_connection(&connections, &r, Dir::To, "other4").as_deref(),
            Some("shop4")
        );
        // A fixed From connection is the answer.
        r.from.conn = "shop4".into();
        assert_eq!(
            target_connection(&connections, &r, Dir::To, "named3").as_deref(),
            Some("shop4")
        );
        // "same" goes back to the connection itself.
        r.from.conn = "any".into();
        r.to.conn = "same".into();
        r.pairs.clear();
        assert_eq!(
            target_connection(&connections, &r, Dir::To, "shop3").as_deref(),
            Some("shop3")
        );
        std::fs::remove_dir_all(dir).ok();
    }
}

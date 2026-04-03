use std::cell::RefCell;
use std::collections::HashMap;

/// Metadata about a database connection, populated from PDO DSN parsing.
///
/// Stored per PHP object handle so that subsequent query calls on the same
/// PDO instance can look up connection attributes (db.system, server.address,
/// server.port, db.name) for span decoration.
#[derive(Debug, Clone)]
pub struct ConnectionMeta {
    /// Database system identifier: "mysql", "postgresql", "sqlite", "oracle", "mssql", "unknown"
    pub db_system: &'static str,
    /// Server hostname (empty for unix socket or sqlite)
    pub host: String,
    /// Server port (0 when not applicable)
    pub port: u16,
    /// Database name (or file path for sqlite)
    pub database: String,
}

thread_local! {
    static CONN_META: RefCell<HashMap<u32, ConnectionMeta>> = RefCell::new(HashMap::new());
}

/// Store connection metadata keyed by the PHP object handle.
pub fn store(object_handle: u32, meta: ConnectionMeta) {
    CONN_META.with(|m| {
        m.borrow_mut().insert(object_handle, meta);
    });
}

/// Retrieve a clone of the connection metadata for a given object handle.
pub fn get(object_handle: u32) -> Option<ConnectionMeta> {
    CONN_META.with(|m| m.borrow().get(&object_handle).cloned())
}

/// Clear all stored connection metadata. Call at request end.
pub fn clear() {
    CONN_META.with(|m| m.borrow_mut().clear());
}

/// Parse a PDO DSN string into connection metadata.
///
/// Supported drivers: mysql, pgsql, sqlite, sqlite2, oci, sqlsrv, dblib.
/// Unknown drivers produce `db_system = "unknown"`.
pub fn parse_pdo_dsn(dsn: &str) -> ConnectionMeta {
    let Some((driver, rest)) = dsn.split_once(':') else {
        return ConnectionMeta {
            db_system: "unknown",
            host: String::new(),
            port: 0,
            database: String::new(),
        };
    };

    let db_system = match driver {
        "mysql" => "mysql",
        "pgsql" => "postgresql",
        "sqlite" | "sqlite2" => "sqlite",
        "oci" => "oracle",
        "sqlsrv" | "dblib" => "mssql",
        _ => "unknown",
    };

    // sqlite: the rest is just the file path
    if db_system == "sqlite" {
        return ConnectionMeta {
            db_system,
            host: String::new(),
            port: 0,
            database: rest.to_string(),
        };
    }

    let default_port = match db_system {
        "mysql" => 3306,
        "postgresql" => 5432,
        "mssql" => 1433,
        "oracle" => 1521,
        _ => 0,
    };

    // Parse key=value pairs separated by semicolons
    let mut host = String::new();
    let mut port: Option<u16> = None;
    let mut database = String::new();

    for part in rest.split(';') {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=') {
            let key = key.trim().to_lowercase();
            let value = value.trim();
            match key.as_str() {
                "host" | "server" => host = value.to_string(),
                "port" => {
                    port = value.parse().ok();
                }
                "dbname" | "database" => database = value.to_string(),
                _ => {}
            }
        }
    }

    ConnectionMeta {
        db_system,
        host,
        port: port.unwrap_or(default_port),
        database,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mysql_dsn() {
        let meta = parse_pdo_dsn("mysql:host=localhost;port=3306;dbname=shop");
        assert_eq!(meta.db_system, "mysql");
        assert_eq!(meta.host, "localhost");
        assert_eq!(meta.port, 3306);
        assert_eq!(meta.database, "shop");
    }

    #[test]
    fn test_parse_mysql_default_port() {
        let meta = parse_pdo_dsn("mysql:host=db.example.com;dbname=app");
        assert_eq!(meta.db_system, "mysql");
        assert_eq!(meta.host, "db.example.com");
        assert_eq!(meta.port, 3306);
        assert_eq!(meta.database, "app");
    }

    #[test]
    fn test_parse_pgsql_dsn() {
        let meta = parse_pdo_dsn("pgsql:host=pg.local;port=5433;dbname=analytics");
        assert_eq!(meta.db_system, "postgresql");
        assert_eq!(meta.host, "pg.local");
        assert_eq!(meta.port, 5433);
        assert_eq!(meta.database, "analytics");
    }

    #[test]
    fn test_parse_sqlite_dsn() {
        let meta = parse_pdo_dsn("sqlite:/path/to/db.sqlite");
        assert_eq!(meta.db_system, "sqlite");
        assert_eq!(meta.host, "");
        assert_eq!(meta.port, 0);
        assert_eq!(meta.database, "/path/to/db.sqlite");
    }

    #[test]
    fn test_parse_empty_dsn() {
        let meta = parse_pdo_dsn("unknown");
        assert_eq!(meta.db_system, "unknown");
        assert_eq!(meta.host, "");
        assert_eq!(meta.port, 0);
        assert_eq!(meta.database, "");
    }

    #[test]
    fn test_store_and_get() {
        // Clear any previous state
        clear();

        let meta = ConnectionMeta {
            db_system: "mysql",
            host: "localhost".into(),
            port: 3306,
            database: "test".into(),
        };
        store(42, meta);

        let retrieved = get(42);
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.db_system, "mysql");
        assert_eq!(retrieved.host, "localhost");
        assert_eq!(retrieved.port, 3306);
        assert_eq!(retrieved.database, "test");

        // Nonexistent handle returns None
        assert!(get(999).is_none());
    }

    #[test]
    fn test_clear() {
        // Clear any previous state
        clear();

        let meta = ConnectionMeta {
            db_system: "postgresql",
            host: "pg.local".into(),
            port: 5432,
            database: "mydb".into(),
        };
        store(100, meta);
        assert!(get(100).is_some());

        clear();
        assert!(get(100).is_none());
    }

    #[test]
    fn test_parse_mysql_unix_socket() {
        let meta = parse_pdo_dsn("mysql:unix_socket=/tmp/mysql.sock;dbname=shop");
        assert_eq!(meta.db_system, "mysql");
        assert_eq!(meta.host, "");
        assert_eq!(meta.port, 3306);
        assert_eq!(meta.database, "shop");
    }

    #[test]
    fn test_parse_sqlsrv_dsn() {
        let meta = parse_pdo_dsn("sqlsrv:host=mssql.local;port=1433;database=master");
        assert_eq!(meta.db_system, "mssql");
        assert_eq!(meta.host, "mssql.local");
        assert_eq!(meta.port, 1433);
        assert_eq!(meta.database, "master");
    }
}

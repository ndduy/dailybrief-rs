//! SQLite access (ADR 0002): one `Db` handle per process owns the connection, the pragmas and the
//! blocking boundary. Every SQL statement lives in `repo`; callers compose repo functions inside
//! one `Db::call` closure.

mod migrate;
pub mod migrations;
pub mod repo;

pub use migrate::{applied_migrations, migrate};

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

pub use rusqlite::Connection;

/// Every way database access can fail.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("cannot create {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("migration {id} failed: {source}")]
    Migration {
        id: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error("database task panicked")]
    Panicked,
    #[error("corrupt row: {0}")]
    Corrupt(String),
}

/// Handle to the single connection. Cheap to clone; all clones share the connection.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Db")
    }
}

impl Db {
    /// Opens (creating if needed) the file, sets the pragmas, and applies pending migrations.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        Self::open_reporting(path).map(|(db, _)| db)
    }

    /// Like `open`, and also returns the migration ids this call applied (empty when current).
    pub fn open_reporting(path: &Path) -> Result<(Self, Vec<&'static str>), DbError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| DbError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut conn = Connection::open(path)?;
        let applied = Self::prepare(&mut conn)?;
        Ok((Self::from_connection(conn), applied))
    }

    /// An in-memory database with the same pragmas and schema; one per test.
    pub fn open_in_memory() -> Result<Self, DbError> {
        let mut conn = Connection::open_in_memory()?;
        Self::prepare(&mut conn)?;
        Ok(Self::from_connection(conn))
    }

    fn from_connection(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Pragmas on every open (`SPEC.md` §7, ADR 0002), then the ledger.
    fn prepare(conn: &mut Connection) -> Result<Vec<&'static str>, DbError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(conn, chrono::Utc::now())
    }

    /// Runs `f` on the connection inside `spawn_blocking`. A panic inside `f` becomes
    /// `DbError::Panicked`; the lock is recovered so later calls still work.
    pub async fn call<T, F>(&self, f: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut guard)
        })
        .await
        .map_err(|_| DbError::Panicked)?
    }

    /// Synchronous access for code that is already on a blocking thread (commands, tests).
    pub fn with<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, DbError>,
    ) -> Result<T, DbError> {
        let mut guard = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pragma<T: rusqlite::types::FromSql>(conn: &Connection, name: &str) -> T {
        conn.pragma_query_value(None, name, |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn pragmas_are_set() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("nested/brief.db")).unwrap();
        db.with(|conn| {
            assert_eq!(pragma::<String>(conn, "journal_mode"), "wal");
            assert_eq!(pragma::<i64>(conn, "synchronous"), 1);
            assert_eq!(pragma::<i64>(conn, "busy_timeout"), 5000);
            assert_eq!(pragma::<i64>(conn, "foreign_keys"), 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn foreign_key_violation_is_rejected() {
        let db = Db::open_in_memory().unwrap();
        let err = db
            .with(|conn| {
                conn.execute(
                    "INSERT INTO items (id, source_id, url, canonical_url, title, fetched_at, text, word_count, content_hash, title_hash)
                     VALUES ('i1', 'missing', 'u', 'u', 't', '2026-09-17T00:00:00.000Z', '', 0, 'c', 't')",
                    [],
                )?;
                Ok(())
            })
            .unwrap_err();
        assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
    }

    #[tokio::test]
    async fn call_runs_on_blocking_thread_and_returns_value() {
        let db = Db::open_in_memory().unwrap();
        let n: i64 = db
            .call(|conn| Ok(conn.query_row("SELECT count(*) FROM migrations", [], |r| r.get(0))?))
            .await
            .unwrap();
        assert_eq!(n, 2, "0001_init and 0002_feedback");
    }

    #[tokio::test]
    async fn call_maps_poisoned_lock_to_error() {
        let db = Db::open_in_memory().unwrap();
        let err = db
            .call(|_conn| -> Result<(), DbError> { panic!("boom") })
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::Panicked), "{err}");
        // The lock is recovered: the handle keeps working after the panic.
        let n: i64 = db
            .call(|conn| Ok(conn.query_row("SELECT 1", [], |r| r.get(0))?))
            .await
            .unwrap();
        assert_eq!(n, 1);
    }
}

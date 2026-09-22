//! The migration ledger (`SPEC.md` §5): `migrations(id TEXT PRIMARY KEY, applied_at TEXT NOT NULL)`,
//! created if missing, applied in id order, one transaction per migration, never edited.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use super::DbError;
use super::migrations::MIGRATIONS;

const LEDGER_DDL: &str =
    "CREATE TABLE IF NOT EXISTS migrations (id TEXT PRIMARY KEY, applied_at TEXT NOT NULL)";

/// Ids already in the ledger, in id order. Creates the ledger table if it does not exist.
pub fn applied_migrations(conn: &Connection) -> Result<Vec<String>, DbError> {
    conn.execute_batch(LEDGER_DDL)?;
    let mut stmt = conn.prepare("SELECT id FROM migrations ORDER BY id")?;
    let ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Applies every pending migration in order; returns the ids applied by this call.
pub fn migrate(conn: &mut Connection, now: DateTime<Utc>) -> Result<Vec<&'static str>, DbError> {
    let done = applied_migrations(conn)?;
    let applied_at = now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let mut applied = Vec::new();
    for m in MIGRATIONS {
        if done.iter().any(|d| d == m.id) {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)
            .and_then(|()| {
                tx.execute(
                    "INSERT INTO migrations (id, applied_at) VALUES (?1, ?2)",
                    params![m.id, applied_at],
                )
            })
            .map_err(|source| DbError::Migration { id: m.id, source })?;
        tx.commit()?;
        applied.push(m.id);
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fresh() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    fn names(conn: &Connection, kind: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = ?1 ORDER BY name")
            .unwrap();
        stmt.query_map([kind], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<String>, _>>()
            .unwrap()
    }

    #[test]
    fn applies_0001_on_empty_db() {
        let mut conn = fresh();
        let applied = migrate(&mut conn, Utc::now()).unwrap();
        assert_eq!(applied, vec!["0001_init", "0002_feedback"]);
        let tables = names(&conn, "table");
        for t in [
            "sources",
            "items",
            "topics",
            "runs",
            "run_events",
            "run_reads",
            "selections",
            "digests",
            "digest_items",
            "reads",
            "editor_notes",
            "run_lock",
            "feed_issues",
            "migrations",
        ] {
            assert!(
                tables.iter().any(|n| n == t),
                "missing table {t}: {tables:?}"
            );
        }
        let indexes = names(&conn, "index");
        for i in ["items_fetched_at", "items_title_hash", "digests_date"] {
            assert!(
                indexes.iter().any(|n| n == i),
                "missing index {i}: {indexes:?}"
            );
        }
    }

    #[test]
    fn applying_twice_is_a_noop() {
        let mut conn = fresh();
        migrate(&mut conn, Utc::now()).unwrap();
        let second = migrate(&mut conn, Utc::now()).unwrap();
        assert!(second.is_empty());
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2);
        assert_eq!(
            applied_migrations(&conn).unwrap(),
            vec!["0001_init", "0002_feedback"]
        );
    }

    #[test]
    fn ledger_row_has_rfc3339_millis_z() {
        let mut conn = fresh();
        let at = Utc.with_ymd_and_hms(2026, 9, 17, 6, 30, 0).unwrap();
        migrate(&mut conn, at).unwrap();
        let stored: String = conn
            .query_row(
                "SELECT applied_at FROM migrations WHERE id = '0001_init'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, "2026-09-17T06:30:00.000Z");
    }

    #[test]
    fn a_failing_migration_leaves_no_ledger_row() {
        // A ledger that claims a bogus id is never touched; a bogus statement rolls back.
        let mut conn = fresh();
        conn.execute_batch(
            "CREATE TABLE migrations (id TEXT PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        // Pre-create `sources` so 0001_init's first statement fails.
        conn.execute_batch("CREATE TABLE sources (id TEXT PRIMARY KEY)")
            .unwrap();
        let err = migrate(&mut conn, Utc::now()).unwrap_err();
        assert!(
            matches!(
                err,
                DbError::Migration {
                    id: "0001_init",
                    ..
                }
            ),
            "{err}"
        );
        assert!(applied_migrations(&conn).unwrap().is_empty());
        assert!(!names(&conn, "table").iter().any(|n| n == "items"));
    }

    fn schema(conn: &Connection) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare(
                "SELECT name, sql FROM sqlite_master WHERE type IN ('table','index') AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn migration_0002_applies_on_the_r0_schema_and_twice_is_a_noop() {
        let mut conn = fresh();
        // The R0 database: 0001 only.
        let tx = conn.transaction().unwrap();
        tx.execute_batch(LEDGER_DDL).unwrap();
        tx.execute_batch(MIGRATIONS[0].sql).unwrap();
        tx.execute(
            "INSERT INTO migrations (id, applied_at) VALUES ('0001_init', '2026-09-17T00:00:00.000Z')",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
        let applied = migrate(&mut conn, Utc::now()).unwrap();
        assert_eq!(applied, vec!["0002_feedback"]);
        let tables = names(&conn, "table");
        assert!(tables.contains(&"ratings".to_string()));
        assert!(tables.contains(&"proposals".to_string()));
        assert!(columns(&conn, "runs").contains(&"role".to_string()));
        let role: String = conn
            .query_row(
                "INSERT INTO runs (id, kind, harness, status, started_at) VALUES ('r', 'manual', 'h', 'running', 'now') RETURNING role",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(role, "editor", "existing and new runs default to editor");
        assert!(migrate(&mut conn, Utc::now()).unwrap().is_empty());
    }

    #[test]
    fn migration_0002_rollback_restores_the_0001_schema() {
        let mut conn = fresh();
        let tx = conn.transaction().unwrap();
        tx.execute_batch(LEDGER_DDL).unwrap();
        tx.execute_batch(MIGRATIONS[0].sql).unwrap();
        tx.execute(
            "INSERT INTO migrations (id, applied_at) VALUES ('0001_init', '2026-09-17T00:00:00.000Z')",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
        let before = schema(&conn);
        migrate(&mut conn, Utc::now()).unwrap();
        assert_ne!(schema(&conn), before);
        conn.execute_batch(super::super::migrations::ROLLBACK_0002)
            .unwrap();
        assert_eq!(
            schema(&conn),
            before,
            "the 0001 schema is back, text for text"
        );
        assert_eq!(applied_migrations(&conn).unwrap(), vec!["0001_init"]);
    }

    #[test]
    fn unknown_applied_migration_ids_are_ignored() {
        let mut conn = fresh();
        migrate(&mut conn, Utc::now()).unwrap();
        conn.execute(
            "INSERT INTO migrations (id, applied_at) VALUES ('0003_future', '2027-01-01T00:00:00.000Z')",
            [],
        )
        .unwrap();
        assert!(migrate(&mut conn, Utc::now()).unwrap().is_empty());
        assert_eq!(
            applied_migrations(&conn).unwrap(),
            vec!["0001_init", "0002_feedback", "0003_future"]
        );
    }
}

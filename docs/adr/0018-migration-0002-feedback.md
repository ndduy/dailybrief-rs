# ADR 0018: Migration `0002_feedback` — ratings, proposals, `runs.role`, and the way back

Date: 2026-09-22 · Status: accepted (Task 1; the live apply is recorded at the M3 ship) · Milestone: M3

## Context
M3 needs the reader's ratings, the Curator's proposals, and a way to tell a Curator run from
an Editor run. The `runs.kind` column carries a CHECK (`scheduled`, `manual`) that SQLite
cannot alter in place, and the live database is the R0 file adopted on 2026-09-18, so the
change must be one appended migration with a stated rollback (`SPEC.md` §5 ledger rules).

## Decision
`0002_feedback`, in one transaction:

```sql
CREATE TABLE ratings (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id TEXT NOT NULL REFERENCES items(id),
  digest_id TEXT REFERENCES digests(id),
  sign TEXT NOT NULL CHECK (sign IN ('up', 'down')),
  reason TEXT NOT NULL,
  at TEXT NOT NULL,
  UNIQUE (item_id)
);
CREATE TABLE proposals (
  id TEXT PRIMARY KEY,
  run_id TEXT REFERENCES runs(id),
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  evidence_json TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'rejected')),
  created_at TEXT NOT NULL,
  decided_at TEXT,
  applied_json TEXT
);
CREATE INDEX proposals_status_created ON proposals (status, created_at);
ALTER TABLE runs ADD COLUMN role TEXT NOT NULL DEFAULT 'editor';
```

- One rating per item (`UNIQUE (item_id)`): a second rating replaces the first; un-rating deletes.
- `runs.role` defaults to `editor`, so every existing row and every Editor run reads the same;
  Curator runs write `curator`. The `kind` CHECK is untouched.
- Rollback (`migrations::ROLLBACK_0002`, never run by the service): drop the index and the two
  tables, `ALTER TABLE runs DROP COLUMN role`, delete the ledger row. Proven text-for-text
  against `sqlite_master` on a copy (`migration_0002_rollback_restores_the_0001_schema`).
- Unknown applied ids are ignored by `migrate`, so the `m2` image serves against a database
  that carries `0002` (`unknown_applied_migration_ids_are_ignored`); the M3 deploy still
  rehearses that on a copy before the live apply.

## Live apply (Checkpoint E)
`cp /data/brief.db /data/brief.db.pre-0002` inside the volume, `dailybrief migrate` from the
new image, then `up -d`. Rolling back the image alone keeps the tables (harmless); rolling
back the schema means `down`, run `ROLLBACK_0002` on the file (or restore the backup), `up -d`
with the old image.

## Consequences
- Ratings and proposals are lost on a schema rollback; nothing else is.
- The next migration (M5 or later) follows the same shape: one id, one transaction, its own
  documented rollback next to it.

## Rehearsal

2026-09-22 (Checkpoint A): a copy of the live `brief.db` (21 runs) taken in the test container, `dailybrief migrate` applied `0002_feedback`, the rollback SQL above restored the 0001 schema (ledger, objects, `runs.role` all checked, `integrity_check` ok), and a second `migrate` re-applied it cleanly. Scripts: `data/checkpoint-a/{sq.sh,inspect.sql,rollback.sql}` (gitignored; the same SQL is `ROLLBACK_0002`).

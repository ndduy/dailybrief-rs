//! Migration ledger contents. `SPEC.md` §5 is the source of truth for every statement here;
//! an applied migration is never edited — append a new id instead.

/// One migration: an id (applied in lexical order) and the SQL it runs inside one transaction.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub id: &'static str,
    pub sql: &'static str,
}

/// Every migration, in the order they apply. The live database already holds `0001_init`.
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "0001_init",
    sql: INIT_0001,
}];

/// `SPEC.md` §5, verbatim.
const INIT_0001: &str = r#"
CREATE TABLE sources (
  id TEXT PRIMARY KEY,
  url TEXT NOT NULL,
  title TEXT NOT NULL,
  weight REAL NOT NULL DEFAULT 1,
  enabled INTEGER NOT NULL DEFAULT 1,
  etag TEXT,
  last_modified TEXT,
  failures INTEGER NOT NULL DEFAULT 0,
  last_ok_at TEXT,
  last_error TEXT
);

CREATE TABLE items (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  url TEXT NOT NULL,
  canonical_url TEXT NOT NULL UNIQUE,
  title TEXT NOT NULL,
  author TEXT,
  published_at TEXT,
  fetched_at TEXT NOT NULL,
  text TEXT NOT NULL,
  word_count INTEGER NOT NULL,
  content_hash TEXT NOT NULL,
  title_hash TEXT NOT NULL,
  vector BLOB,
  social_score REAL
);
CREATE INDEX items_fetched_at ON items(fetched_at);
CREATE INDEX items_title_hash ON items(title_hash);

CREATE TABLE topics (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  weight REAL NOT NULL DEFAULT 1,
  vector BLOB,
  origin TEXT NOT NULL CHECK (origin IN ('seed', 'curator', 'explore-promoted')),
  last_positive_at TEXT,
  saturation INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE runs (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('scheduled', 'manual')),
  harness TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('running', 'success', 'failed', 'killed')),
  attempt INTEGER NOT NULL DEFAULT 1,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  turns INTEGER,
  usage_json TEXT,
  cost_usd REAL,
  session_id TEXT,
  error TEXT,
  transcript_path TEXT
);

CREATE TABLE run_events (
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  PRIMARY KEY (run_id, seq)
);

CREATE TABLE run_reads (
  run_id TEXT NOT NULL,
  item_id TEXT NOT NULL REFERENCES items(id),
  PRIMARY KEY (run_id, item_id)
);

CREATE TABLE selections (
  run_id TEXT NOT NULL,
  item_id TEXT NOT NULL REFERENCES items(id),
  section TEXT NOT NULL CHECK (section IN ('for_you', 'beyond_radar')),
  position INTEGER NOT NULL,
  summary TEXT NOT NULL,
  why_it_matters TEXT NOT NULL,
  reason TEXT,
  topic TEXT NOT NULL,
  PRIMARY KEY (run_id, item_id)
);

CREATE TABLE digests (
  id TEXT PRIMARY KEY,
  date TEXT NOT NULL,
  run_id TEXT NOT NULL REFERENCES runs(id),
  published_at TEXT NOT NULL,
  for_you_count INTEGER NOT NULL,
  beyond_radar_count INTEGER NOT NULL
);
CREATE INDEX digests_date ON digests(date);

CREATE TABLE digest_items (
  digest_id TEXT NOT NULL REFERENCES digests(id) ON DELETE CASCADE,
  item_id TEXT NOT NULL REFERENCES items(id),
  section TEXT NOT NULL CHECK (section IN ('for_you', 'beyond_radar')),
  position INTEGER NOT NULL,
  summary TEXT NOT NULL,
  why_it_matters TEXT NOT NULL,
  reason TEXT,
  topic TEXT NOT NULL,
  PRIMARY KEY (digest_id, item_id)
);

CREATE TABLE reads (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id TEXT NOT NULL REFERENCES items(id),
  digest_id TEXT REFERENCES digests(id),
  at TEXT NOT NULL
);

CREATE TABLE editor_notes (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  text TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL
);

CREATE TABLE run_lock (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  run_id TEXT NOT NULL,
  acquired_at TEXT NOT NULL
);

CREATE TABLE feed_issues (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  run_id TEXT,
  kind TEXT NOT NULL,
  note TEXT,
  at TEXT NOT NULL
);
"#;

//! Verifies that a v1 database (schema_version = 1, status CHECK without
//! 'partial') is migrated in place when the CLI opens it.

use assert_cmd::Command;
use rusqlite::params;
use tempfile::TempDir;

const V1_SCHEMA: &str = r#"
CREATE TABLE tasks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    title        TEXT NOT NULL,
    details      TEXT,
    status       TEXT NOT NULL CHECK(status IN ('pending','in-progress','done')),
    priority     INTEGER NOT NULL DEFAULT 3 CHECK(priority BETWEEN 1 AND 5),
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    completed_at TEXT
);
CREATE TABLE tags (
    task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    tag     TEXT NOT NULL,
    PRIMARY KEY (task_id, tag)
);
CREATE TABLE deps (
    task_id       INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, depends_on_id),
    CHECK (task_id <> depends_on_id)
);
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE INDEX idx_tasks_status_priority ON tasks(status, priority, created_at);
CREATE INDEX idx_tags_tag ON tags(tag);
CREATE INDEX idx_deps_depends_on ON deps(depends_on_id);
"#;

const V2_SCHEMA: &str = r#"
CREATE TABLE tasks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    title        TEXT NOT NULL,
    details      TEXT,
    status       TEXT NOT NULL CHECK(status IN ('pending','partial','in-progress','done')),
    priority     INTEGER NOT NULL DEFAULT 3 CHECK(priority BETWEEN 1 AND 5),
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    completed_at TEXT
);
CREATE TABLE tags (
    task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    tag     TEXT NOT NULL,
    PRIMARY KEY (task_id, tag)
);
CREATE TABLE deps (
    task_id       INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, depends_on_id),
    CHECK (task_id <> depends_on_id)
);
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE INDEX idx_tasks_status_priority ON tasks(status, priority, created_at);
CREATE INDEX idx_tags_tag ON tags(tag);
CREATE INDEX idx_deps_depends_on ON deps(depends_on_id);
"#;

#[test]
fn v1_database_migrates_to_v2_on_open() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("v1.db");

    // Build a v1 DB with two tasks at the v1 schema version.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(V1_SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO meta(key, value) VALUES('schema_version', '1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks(title, status, priority, created_at, started_at) \
             VALUES('inflight', 'in-progress', 2, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks(title, status, priority, created_at) \
             VALUES('queued', 'pending', 3, '2026-01-02T00:00:00Z')",
            [],
        )
        .unwrap();
    }

    // Run any CLI command — opening the DB should migrate it.
    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.arg("--db").arg(&db).args(["list", "--json"]);
    cmd.env_remove("TODO_SQLITE_CLI_DB");
    cmd.assert().success();

    // Verify schema_version bumped to the latest and 'partial'/'rejected'
    // are now accepted statuses (v1 chains through v2, v3, to v4).
    let conn = rusqlite::Connection::open(&db).unwrap();
    let v: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v, "6");

    // Existing data preserved.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);

    // 'partial' must be insertable now.
    conn.execute(
        "UPDATE tasks SET status = 'partial' WHERE title = 'inflight'",
        params![],
    )
    .expect("partial must be allowed after migration");

    // 'rejected' must be insertable now.
    conn.execute(
        "UPDATE tasks SET status = 'rejected' WHERE title = 'queued'",
        params![],
    )
    .expect("rejected must be allowed after migration");
}

#[test]
fn v2_database_migrates_to_v3_on_open() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("v2.db");

    // Build a v2 DB: 'partial' allowed, 'rejected' not yet.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(V2_SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO meta(key, value) VALUES('schema_version', '2')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks(title, status, priority, created_at) \
             VALUES('queued', 'pending', 3, '2026-01-02T00:00:00Z')",
            [],
        )
        .unwrap();
        // 'rejected' must be rejected by the v2 CHECK constraint.
        conn.execute(
            "UPDATE tasks SET status = 'rejected' WHERE title = 'queued'",
            params![],
        )
        .expect_err("rejected must NOT be allowed before migration");
    }

    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.arg("--db").arg(&db).args(["list", "--json"]);
    cmd.env_remove("TODO_SQLITE_CLI_DB");
    cmd.assert().success();

    let conn = rusqlite::Connection::open(&db).unwrap();
    let v: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v, "6");

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);

    conn.execute(
        "UPDATE tasks SET status = 'rejected' WHERE title = 'queued'",
        params![],
    )
    .expect("rejected must be allowed after migration");
}

const V3_SCHEMA: &str = r#"
CREATE TABLE tasks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    title        TEXT NOT NULL,
    details      TEXT,
    status       TEXT NOT NULL CHECK(status IN ('pending','partial','in-progress','done','rejected')),
    priority     INTEGER NOT NULL DEFAULT 3 CHECK(priority BETWEEN 1 AND 5),
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    completed_at TEXT
);
CREATE TABLE tags (
    task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    tag     TEXT NOT NULL,
    PRIMARY KEY (task_id, tag)
);
CREATE TABLE deps (
    task_id       INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, depends_on_id),
    CHECK (task_id <> depends_on_id)
);
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE INDEX idx_tasks_status_priority ON tasks(status, priority, created_at);
CREATE INDEX idx_tags_tag ON tags(tag);
CREATE INDEX idx_deps_depends_on ON deps(depends_on_id);
"#;

#[test]
fn v3_database_migrates_to_v4_on_open() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("v3.db");

    // Build a v3 DB: no is_gate column yet.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(V3_SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO meta(key, value) VALUES('schema_version', '3')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks(title, status, priority, created_at) \
             VALUES('preexisting', 'pending', 3, '2026-01-02T00:00:00Z')",
            [],
        )
        .unwrap();
        // is_gate must not exist yet on a v3 DB.
        conn.query_row("SELECT is_gate FROM tasks", [], |r| r.get::<_, i64>(0))
            .expect_err("is_gate must NOT exist before migration");
    }

    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.arg("--db").arg(&db).args(["list", "--json"]);
    cmd.env_remove("TODO_SQLITE_CLI_DB");
    cmd.assert().success();

    let conn = rusqlite::Connection::open(&db).unwrap();
    let v: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v, "6");

    // Pre-existing rows default is_gate to 0.
    let is_gate: i64 = conn
        .query_row(
            "SELECT is_gate FROM tasks WHERE title = 'preexisting'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(is_gate, 0);

    // Column accepts 0/1 only.
    conn.execute(
        "UPDATE tasks SET is_gate = 1 WHERE title = 'preexisting'",
        [],
    )
    .expect("is_gate must accept 1");
    conn.execute(
        "UPDATE tasks SET is_gate = 2 WHERE title = 'preexisting'",
        [],
    )
    .expect_err("is_gate must reject values outside 0/1");
}

#[test]
fn add_after_migration_continues_from_max_existing_id() {
    // v5 drops AUTOINCREMENT/sqlite_sequence for `id` entirely — it's a
    // plain, non-unique display id now, allocated as MAX(id)+1 at insert
    // time. This test replaces the old
    // `migrated_db_preserves_autoincrement_counter`, which relied on
    // sqlite_sequence surviving a delete-everything-then-add sequence; that
    // guarantee no longer exists; what does still hold is that `add`
    // continues from the highest id actually present after migration.
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("v1.db");

    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(V1_SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO meta(key, value) VALUES('schema_version', '1')",
            [],
        )
        .unwrap();
        for i in 1..=3 {
            conn.execute(
                "INSERT INTO tasks(title, status, priority, created_at) \
                 VALUES(?1, 'pending', 3, '2026-01-01T00:00:00Z')",
                params![format!("t{i}")],
            )
            .unwrap();
        }
        // Delete the highest-id row; a stale AUTOINCREMENT counter would
        // still hand out id 4 next — MAX(id)+1 must hand out 3 instead.
        conn.execute("DELETE FROM tasks WHERE title = 't3'", [])
            .unwrap();
    }

    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.arg("--db").arg(&db).args(["add", "after-migration"]);
    cmd.env_remove("TODO_SQLITE_CLI_DB");
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let id: i64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    assert_eq!(id, 3, "id allocation must be MAX(existing id) + 1");
}

// Frozen copy of the v4 schema (the shape right before uuid became the
// primary key) — intentionally not derived from `db.rs`'s current schema,
// so this fixture stays historically accurate as the schema keeps evolving.
const V4_SCHEMA: &str = r#"
CREATE TABLE tasks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    title        TEXT NOT NULL,
    details      TEXT,
    status       TEXT NOT NULL CHECK(status IN ('pending','partial','in-progress','done','rejected')),
    priority     INTEGER NOT NULL DEFAULT 3 CHECK(priority BETWEEN 1 AND 5),
    is_gate      INTEGER NOT NULL DEFAULT 0 CHECK(is_gate IN (0,1)),
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    completed_at TEXT
);
CREATE TABLE tags (
    task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    tag     TEXT NOT NULL,
    PRIMARY KEY (task_id, tag)
);
CREATE TABLE deps (
    task_id       INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, depends_on_id),
    CHECK (task_id <> depends_on_id)
);
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE INDEX idx_tasks_status_priority ON tasks(status, priority, created_at);
CREATE INDEX idx_tags_tag ON tags(tag);
CREATE INDEX idx_deps_depends_on ON deps(depends_on_id);
"#;

#[test]
fn v4_database_migrates_to_v5_on_open() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("v4.db");

    // Build a v4 DB: two tasks, a tag, and a dep edge, so the migration's
    // id -> uuid translation of tags/deps can be checked, not just tasks.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(V4_SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO meta(key, value) VALUES('schema_version', '4')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks(title, status, priority, is_gate, created_at) \
             VALUES('blocker', 'pending', 3, 0, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks(title, status, priority, is_gate, created_at) \
             VALUES('blocked', 'pending', 3, 0, '2026-01-02T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO tags(task_id, tag) VALUES(1, 'infra')", [])
            .unwrap();
        conn.execute("INSERT INTO deps(task_id, depends_on_id) VALUES(2, 1)", [])
            .unwrap();
        // uuid must not exist yet on a v4 DB.
        conn.query_row("SELECT uuid FROM tasks", [], |r| r.get::<_, String>(0))
            .expect_err("uuid must NOT exist before migration");
    }

    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.arg("--db").arg(&db).args(["list", "--json"]);
    cmd.env_remove("TODO_SQLITE_CLI_DB");
    cmd.assert().success();

    let conn = rusqlite::Connection::open(&db).unwrap();
    let v: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v, "6");

    // Row counts preserved, ids preserved, every task has a distinct
    // well-formed uuid.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);

    let mut stmt = conn
        .prepare("SELECT id, uuid FROM tasks ORDER BY id")
        .unwrap();
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        rows.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        rows[0].1.len(),
        36,
        "uuid should be a canonical 36-char string"
    );
    assert_ne!(rows[0].1, rows[1].1, "each task must get a distinct uuid");

    // The tag survived, keyed by the new uuid.
    let tag_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tags WHERE task_uuid = ?1 AND tag = 'infra'",
            params![rows[0].1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tag_count, 1, "tag must be repointed at the blocker's uuid");

    // The dep edge survived, still pointing task 2 -> task 1 by uuid.
    let dep_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM deps WHERE task_uuid = ?1 AND depends_on_uuid = ?2",
            params![rows[1].1, rows[0].1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        dep_count, 1,
        "dep edge must be repointed at the same logical tasks by uuid"
    );
}

// Frozen copy of the v5 schema (uuid as primary key, plain non-unique `id`,
// no `location` column, no `related` table) — intentionally not derived
// from `db.rs`'s current schema, so this fixture stays historically
// accurate as the schema keeps evolving.
const V5_SCHEMA: &str = r#"
CREATE TABLE tasks (
    uuid         TEXT PRIMARY KEY,
    id           INTEGER NOT NULL,
    title        TEXT NOT NULL,
    details      TEXT,
    status       TEXT NOT NULL CHECK(status IN ('pending','partial','in-progress','done','rejected')),
    priority     INTEGER NOT NULL DEFAULT 3 CHECK(priority BETWEEN 1 AND 5),
    is_gate      INTEGER NOT NULL DEFAULT 0 CHECK(is_gate IN (0,1)),
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    completed_at TEXT
);
CREATE TABLE tags (
    task_uuid TEXT NOT NULL REFERENCES tasks(uuid) ON DELETE CASCADE,
    tag       TEXT NOT NULL,
    PRIMARY KEY (task_uuid, tag)
);
CREATE TABLE deps (
    task_uuid       TEXT NOT NULL REFERENCES tasks(uuid) ON DELETE CASCADE,
    depends_on_uuid TEXT NOT NULL REFERENCES tasks(uuid) ON DELETE CASCADE,
    PRIMARY KEY (task_uuid, depends_on_uuid),
    CHECK (task_uuid <> depends_on_uuid)
);
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE INDEX idx_tasks_status_priority ON tasks(status, priority, created_at);
CREATE INDEX idx_tasks_id ON tasks(id);
CREATE INDEX idx_tags_tag ON tags(tag);
CREATE INDEX idx_deps_depends_on ON deps(depends_on_uuid);
"#;

#[test]
fn v5_database_migrates_to_v6_on_open() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("v5.db");

    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(V5_SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO meta(key, value) VALUES('schema_version', '5')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks(uuid, id, title, status, priority, created_at) \
             VALUES('11111111-1111-1111-1111-111111111111', 1, 'a', 'pending', 3, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks(uuid, id, title, status, priority, created_at) \
             VALUES('22222222-2222-2222-2222-222222222222', 2, 'b', 'pending', 3, '2026-01-02T00:00:00Z')",
            [],
        )
        .unwrap();
        // Neither `location` nor `related` must exist yet on a v5 DB.
        conn.query_row("SELECT location FROM tasks", [], |r| {
            r.get::<_, Option<String>>(0)
        })
        .expect_err("location column must NOT exist before migration");
        conn.query_row("SELECT 1 FROM related", [], |r| r.get::<_, i64>(0))
            .expect_err("related table must NOT exist before migration");
    }

    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.arg("--db").arg(&db).args(["list", "--json"]);
    cmd.env_remove("TODO_SQLITE_CLI_DB");
    cmd.assert().success();

    let conn = rusqlite::Connection::open(&db).unwrap();
    let v: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v, "6");

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);

    // Pre-existing rows default location to NULL.
    let location: Option<String> = conn
        .query_row("SELECT location FROM tasks WHERE title = 'a'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(location, None);

    // `related` table exists and is queryable (empty).
    let related_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM related", [], |r| r.get(0))
        .unwrap();
    assert_eq!(related_count, 0);
}

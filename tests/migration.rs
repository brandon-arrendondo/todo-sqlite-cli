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

    // Verify schema_version bumped and 'partial' is now an accepted status.
    let conn = rusqlite::Connection::open(&db).unwrap();
    let v: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v, "2");

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
}

#[test]
fn migrated_db_preserves_autoincrement_counter() {
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
        // Insert and delete to bump sqlite_sequence past 1.
        for i in 1..=3 {
            conn.execute(
                "INSERT INTO tasks(title, status, priority, created_at) \
                 VALUES(?1, 'pending', 3, '2026-01-01T00:00:00Z')",
                params![format!("t{i}")],
            )
            .unwrap();
        }
        conn.execute("DELETE FROM tasks", []).unwrap();
    }

    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.arg("--db").arg(&db).args(["add", "after-migration"]);
    cmd.env_remove("TODO_SQLITE_CLI_DB");
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let id: i64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    assert_eq!(id, 4, "AUTOINCREMENT counter must survive migration");
}

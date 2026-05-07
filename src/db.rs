use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::{system, user, CliResult};

pub const SCHEMA_VERSION: i64 = 2;

const SCHEMA_SQL: &str = r#"
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Pending,
    Partial,
    InProgress,
    Done,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Partial => "partial",
            Status::InProgress => "in-progress",
            Status::Done => "done",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub details: Option<String>,
    pub status: String,
    pub priority: i64,
    pub tags: Vec<String>,
    pub depends_on: Vec<i64>,
    pub blocked: bool,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn parse_date_bound(s: &str) -> CliResult<String> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt
            .with_timezone(&Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
        return Ok(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    Err(user(format!(
        "invalid date '{s}' (expected YYYY-MM-DD or RFC3339)"
    )))
}

pub fn open(path: &Path) -> CliResult<Connection> {
    let conn = Connection::open(path)
        .map_err(|e| system(format!("cannot open database {}: {e}", path.display())))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| system(format!("pragma journal_mode failed: {e}")))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| system(format!("pragma foreign_keys failed: {e}")))?;
    if is_initialized(&conn) {
        migrate(&conn)?;
    }
    Ok(conn)
}

fn read_schema_version(conn: &Connection) -> CliResult<i64> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| system(format!("meta read failed: {e}")))?;
    match v {
        Some(s) => s
            .parse::<i64>()
            .map_err(|e| system(format!("schema_version parse failed: {e}"))),
        None => Ok(1),
    }
}

fn migrate(conn: &Connection) -> CliResult<()> {
    let current = read_schema_version(conn)?;
    if current == SCHEMA_VERSION {
        return Ok(());
    }
    if current > SCHEMA_VERSION {
        return Err(system(format!(
            "database schema version {current} is newer than this binary supports ({SCHEMA_VERSION}); upgrade todo-sqlite-cli"
        )));
    }
    if current < 1 {
        return Err(system(format!("invalid schema_version {current}")));
    }
    if current == 1 {
        migrate_v1_to_v2(conn)?;
    }
    Ok(())
}

fn migrate_v1_to_v2(conn: &Connection) -> CliResult<()> {
    // Recreate tasks with the expanded status CHECK to allow 'partial'.
    // SQLite cannot alter CHECK constraints in place, so copy via a new table.
    // The AUTOINCREMENT counter must survive — read it before, set it after.
    let old_seq: i64 = conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'tasks'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| system(format!("read sqlite_sequence failed: {e}")))?
        .unwrap_or(0);

    conn.pragma_update(None, "foreign_keys", "OFF")
        .map_err(|e| system(format!("pragma foreign_keys=OFF failed: {e}")))?;
    conn.execute_batch(
        r#"
        BEGIN;
        CREATE TABLE tasks_new (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            title        TEXT NOT NULL,
            details      TEXT,
            status       TEXT NOT NULL CHECK(status IN ('pending','partial','in-progress','done')),
            priority     INTEGER NOT NULL DEFAULT 3 CHECK(priority BETWEEN 1 AND 5),
            created_at   TEXT NOT NULL,
            started_at   TEXT,
            completed_at TEXT
        );
        INSERT INTO tasks_new(id, title, details, status, priority, created_at, started_at, completed_at)
            SELECT id, title, details, status, priority, created_at, started_at, completed_at FROM tasks;
        DROP TABLE tasks;
        ALTER TABLE tasks_new RENAME TO tasks;
        DROP INDEX IF EXISTS idx_tasks_status_priority;
        CREATE INDEX idx_tasks_status_priority ON tasks(status, priority, created_at);
        UPDATE meta SET value = '2' WHERE key = 'schema_version';
        COMMIT;
        "#,
    )
    .map_err(|e| system(format!("v1->v2 migration failed: {e}")))?;

    // Restore the AUTOINCREMENT counter. sqlite_sequence has no UNIQUE on
    // `name`, so we must clear any rows the table-swap dance left behind
    // before writing the saved value.
    conn.execute("DELETE FROM sqlite_sequence WHERE name = 'tasks'", [])
        .map_err(|e| system(format!("clear sqlite_sequence failed: {e}")))?;
    if old_seq > 0 {
        conn.execute(
            "INSERT INTO sqlite_sequence(name, seq) VALUES('tasks', ?1)",
            params![old_seq],
        )
        .map_err(|e| system(format!("restore sqlite_sequence failed: {e}")))?;
    }

    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| system(format!("pragma foreign_keys=ON failed: {e}")))?;
    Ok(())
}

pub fn create_schema(conn: &Connection) -> CliResult<()> {
    conn.execute_batch(SCHEMA_SQL)
        .map_err(|e| system(format!("schema create failed: {e}")))?;
    conn.execute(
        "INSERT INTO meta(key, value) VALUES('schema_version', ?1)",
        params![SCHEMA_VERSION.to_string()],
    )
    .map_err(|e| system(format!("meta insert failed: {e}")))?;
    Ok(())
}

pub fn is_initialized(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='tasks'",
        [],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

pub fn load_task(conn: &Connection, id: i64) -> CliResult<Task> {
    let task = conn
        .query_row(
            "SELECT id, title, details, status, priority, created_at, started_at, completed_at
             FROM tasks WHERE id = ?1",
            params![id],
            row_to_task_base,
        )
        .optional()
        .map_err(|e| system(format!("query failed: {e}")))?
        .ok_or_else(|| user(format!("task {id} not found")))?;
    hydrate(conn, task)
}

fn row_to_task_base(row: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        details: row.get(2)?,
        status: row.get(3)?,
        priority: row.get(4)?,
        tags: Vec::new(),
        depends_on: Vec::new(),
        blocked: false,
        created_at: row.get(5)?,
        started_at: row.get(6)?,
        completed_at: row.get(7)?,
    })
}

fn hydrate(conn: &Connection, mut t: Task) -> CliResult<Task> {
    t.tags = load_tags(conn, t.id)?;
    t.depends_on = load_deps(conn, t.id)?;
    t.blocked = is_blocked(conn, t.id)?;
    Ok(t)
}

pub fn load_tags(conn: &Connection, task_id: i64) -> CliResult<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT tag FROM tags WHERE task_id = ?1 ORDER BY tag")
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map(params![task_id], |r| r.get::<_, String>(0))
        .map_err(|e| system(format!("query failed: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
    }
    Ok(out)
}

pub fn load_deps(conn: &Connection, task_id: i64) -> CliResult<Vec<i64>> {
    let mut stmt = conn
        .prepare("SELECT depends_on_id FROM deps WHERE task_id = ?1 ORDER BY depends_on_id")
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map(params![task_id], |r| r.get::<_, i64>(0))
        .map_err(|e| system(format!("query failed: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
    }
    Ok(out)
}

pub fn is_blocked(conn: &Connection, task_id: i64) -> CliResult<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM deps d
             JOIN tasks t ON t.id = d.depends_on_id
             WHERE d.task_id = ?1 AND t.status <> 'done'",
            params![task_id],
            |r| r.get(0),
        )
        .map_err(|e| system(format!("query failed: {e}")))?;
    Ok(count > 0)
}

pub fn require_task_exists(conn: &Connection, id: i64) -> CliResult<()> {
    let exists: Option<i64> = conn
        .query_row("SELECT id FROM tasks WHERE id = ?1", params![id], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| system(format!("query failed: {e}")))?;
    if exists.is_none() {
        return Err(user(format!("task {id} not found")));
    }
    Ok(())
}

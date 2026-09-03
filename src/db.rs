use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::{system, user, CliResult};

pub const SCHEMA_VERSION: i64 = 6;

const SCHEMA_SQL: &str = r#"
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
    completed_at TEXT,
    location     TEXT
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

-- Non-blocking "see also" links between tasks, stored mirrored: linking A
-- and B writes both (A,B) and (B,A) so either task's row lookup finds the
-- other with a plain WHERE task_uuid = ? (no OR-join needed).
CREATE TABLE related (
    task_uuid    TEXT NOT NULL REFERENCES tasks(uuid) ON DELETE CASCADE,
    related_uuid TEXT NOT NULL REFERENCES tasks(uuid) ON DELETE CASCADE,
    PRIMARY KEY (task_uuid, related_uuid),
    CHECK (task_uuid <> related_uuid)
);

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE INDEX idx_tasks_status_priority ON tasks(status, priority, created_at);
CREATE INDEX idx_tasks_id ON tasks(id);
CREATE INDEX idx_tags_tag ON tags(tag);
CREATE INDEX idx_deps_depends_on ON deps(depends_on_uuid);
CREATE INDEX idx_related_related ON related(related_uuid);
"#;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Pending,
    Partial,
    InProgress,
    Done,
    Rejected,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Partial => "partial",
            Status::InProgress => "in-progress",
            Status::Done => "done",
            Status::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: i64,
    pub uuid: String,
    pub title: String,
    pub details: Option<String>,
    pub status: String,
    pub priority: i64,
    pub is_gate: bool,
    pub tags: Vec<String>,
    pub depends_on: Vec<i64>,
    pub blocked: bool,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub location: Option<String>,
    /// Display ids of mutually-linked "see also" tasks. Only populated by
    /// `hydrate` (i.e. single-task commands like `show`/`add`/`edit`) —
    /// list-style commands leave this empty on purpose, and it's hidden
    /// from JSON in that case rather than printed as `[]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<i64>,
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
    if current <= 1 {
        migrate_v1_to_v2(conn)?;
    }
    if current <= 2 {
        migrate_v2_to_v3(conn)?;
    }
    if current <= 3 {
        migrate_v3_to_v4(conn)?;
    }
    if current <= 4 {
        migrate_v4_to_v5(conn)?;
    }
    if current <= 5 {
        migrate_v5_to_v6(conn)?;
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

fn migrate_v2_to_v3(conn: &Connection) -> CliResult<()> {
    // Recreate tasks with the expanded status CHECK to allow 'rejected'.
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
            status       TEXT NOT NULL CHECK(status IN ('pending','partial','in-progress','done','rejected')),
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
        UPDATE meta SET value = '3' WHERE key = 'schema_version';
        COMMIT;
        "#,
    )
    .map_err(|e| system(format!("v2->v3 migration failed: {e}")))?;

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

fn migrate_v3_to_v4(conn: &Connection) -> CliResult<()> {
    // Add the is_gate column (default 0 for all pre-existing rows).
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
            status       TEXT NOT NULL CHECK(status IN ('pending','partial','in-progress','done','rejected')),
            priority     INTEGER NOT NULL DEFAULT 3 CHECK(priority BETWEEN 1 AND 5),
            is_gate      INTEGER NOT NULL DEFAULT 0 CHECK(is_gate IN (0,1)),
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
        UPDATE meta SET value = '4' WHERE key = 'schema_version';
        COMMIT;
        "#,
    )
    .map_err(|e| system(format!("v3->v4 migration failed: {e}")))?;

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

/// v4's `id` was the AUTOINCREMENT primary key; v5 promotes a generated UUID
/// to the real primary key and relaxes `id` to a plain, non-unique "display
/// id". A cross-node merge can then legitimately produce two tasks sharing a
/// display id — that no longer corrupts anything, since identity is now the
/// uuid; `resolve_one` surfaces the ambiguity to the operator instead.
/// SQLite has no builtin UUID generator, so — unlike v1-v4, which are pure
/// SQL — this migration generates uuids in Rust and can't be one
/// `execute_batch` script; it's wrapped in a single manual transaction
/// instead.
fn migrate_v4_to_v5(conn: &Connection) -> CliResult<()> {
    struct OldTask {
        id: i64,
        title: String,
        details: Option<String>,
        status: String,
        priority: i64,
        is_gate: bool,
        created_at: String,
        started_at: Option<String>,
        completed_at: Option<String>,
    }

    conn.pragma_update(None, "foreign_keys", "OFF")
        .map_err(|e| system(format!("pragma foreign_keys=OFF failed: {e}")))?;
    conn.execute("BEGIN", [])
        .map_err(|e| system(format!("begin failed: {e}")))?;

    conn.execute_batch(
        r#"
        CREATE TABLE tasks_new (
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
        CREATE TABLE tags_new (
            task_uuid TEXT NOT NULL REFERENCES tasks_new(uuid) ON DELETE CASCADE,
            tag       TEXT NOT NULL,
            PRIMARY KEY (task_uuid, tag)
        );
        CREATE TABLE deps_new (
            task_uuid       TEXT NOT NULL REFERENCES tasks_new(uuid) ON DELETE CASCADE,
            depends_on_uuid TEXT NOT NULL REFERENCES tasks_new(uuid) ON DELETE CASCADE,
            PRIMARY KEY (task_uuid, depends_on_uuid),
            CHECK (task_uuid <> depends_on_uuid)
        );
        "#,
    )
    .map_err(|e| system(format!("v4->v5 migration (create) failed: {e}")))?;

    let old_tasks: Vec<OldTask> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, details, status, priority, is_gate, created_at, started_at, completed_at
                 FROM tasks",
            )
            .map_err(|e| system(format!("prepare failed: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(OldTask {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    details: r.get(2)?,
                    status: r.get(3)?,
                    priority: r.get(4)?,
                    is_gate: r.get(5)?,
                    created_at: r.get(6)?,
                    started_at: r.get(7)?,
                    completed_at: r.get(8)?,
                })
            })
            .map_err(|e| system(format!("query failed: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
        }
        out
    };

    let mut id_to_uuid: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    for t in &old_tasks {
        let new_uuid = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO tasks_new(uuid, id, title, details, status, priority, is_gate, created_at, started_at, completed_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                new_uuid,
                t.id,
                t.title,
                t.details,
                t.status,
                t.priority,
                t.is_gate,
                t.created_at,
                t.started_at,
                t.completed_at,
            ],
        )
        .map_err(|e| system(format!("tasks_new insert failed: {e}")))?;
        id_to_uuid.insert(t.id, new_uuid);
    }

    let old_tags: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare("SELECT task_id, tag FROM tags")
            .map_err(|e| system(format!("prepare failed: {e}")))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| system(format!("query failed: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
        }
        out
    };
    for (task_id, tag) in old_tags {
        let task_uuid = id_to_uuid
            .get(&task_id)
            .ok_or_else(|| system(format!("tag references unknown task id {task_id}")))?;
        conn.execute(
            "INSERT OR IGNORE INTO tags_new(task_uuid, tag) VALUES(?1, ?2)",
            params![task_uuid, tag],
        )
        .map_err(|e| system(format!("tags_new insert failed: {e}")))?;
    }

    let old_deps: Vec<(i64, i64)> = {
        let mut stmt = conn
            .prepare("SELECT task_id, depends_on_id FROM deps")
            .map_err(|e| system(format!("prepare failed: {e}")))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| system(format!("query failed: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
        }
        out
    };
    for (task_id, depends_on_id) in old_deps {
        let task_uuid = id_to_uuid
            .get(&task_id)
            .ok_or_else(|| system(format!("dep references unknown task id {task_id}")))?;
        let depends_on_uuid = id_to_uuid
            .get(&depends_on_id)
            .ok_or_else(|| system(format!("dep references unknown task id {depends_on_id}")))?;
        conn.execute(
            "INSERT OR IGNORE INTO deps_new(task_uuid, depends_on_uuid) VALUES(?1, ?2)",
            params![task_uuid, depends_on_uuid],
        )
        .map_err(|e| system(format!("deps_new insert failed: {e}")))?;
    }

    conn.execute_batch(
        r#"
        DROP TABLE deps;
        DROP TABLE tags;
        DROP TABLE tasks;
        ALTER TABLE tasks_new RENAME TO tasks;
        ALTER TABLE tags_new RENAME TO tags;
        ALTER TABLE deps_new RENAME TO deps;
        CREATE INDEX idx_tasks_status_priority ON tasks(status, priority, created_at);
        CREATE INDEX idx_tasks_id ON tasks(id);
        CREATE INDEX idx_tags_tag ON tags(tag);
        CREATE INDEX idx_deps_depends_on ON deps(depends_on_uuid);
        DELETE FROM sqlite_sequence WHERE name = 'tasks';
        UPDATE meta SET value = '5' WHERE key = 'schema_version';
        "#,
    )
    .map_err(|e| system(format!("v4->v5 migration (finish) failed: {e}")))?;

    conn.execute("COMMIT", [])
        .map_err(|e| system(format!("commit failed: {e}")))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| system(format!("pragma foreign_keys=ON failed: {e}")))?;
    Ok(())
}

/// Adds `tasks.location` and the mirrored `related` table. Unlike v4->v5,
/// nothing here needs a Rust-generated value, so a single SQL script
/// suffices.
fn migrate_v5_to_v6(conn: &Connection) -> CliResult<()> {
    conn.execute_batch(
        r#"
        ALTER TABLE tasks ADD COLUMN location TEXT;
        CREATE TABLE related (
            task_uuid    TEXT NOT NULL REFERENCES tasks(uuid) ON DELETE CASCADE,
            related_uuid TEXT NOT NULL REFERENCES tasks(uuid) ON DELETE CASCADE,
            PRIMARY KEY (task_uuid, related_uuid),
            CHECK (task_uuid <> related_uuid)
        );
        CREATE INDEX idx_related_related ON related(related_uuid);
        UPDATE meta SET value = '6' WHERE key = 'schema_version';
        "#,
    )
    .map_err(|e| system(format!("v5->v6 migration failed: {e}")))?;
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

/// Read a database's on-disk schema_version *without* triggering
/// `migrate()`. Used by the git merge driver, which must decide whether
/// it's safe to open (and thus silently auto-migrate) a file before doing
/// so — see `commands::git_merge_driver`. Returns `None` for an
/// uninitialized/empty file (no `tasks` table yet).
pub fn peek_schema_version(path: &Path) -> CliResult<Option<i64>> {
    let conn = Connection::open(path)
        .map_err(|e| system(format!("cannot open database {}: {e}", path.display())))?;
    if !is_initialized(&conn) {
        return Ok(None);
    }
    read_schema_version(&conn).map(Some)
}

/// Check that every present database's *pre-migration* schema version
/// (from `peek_schema_version`) agrees, erroring loudly if not. Any caller
/// that's about to merge multiple database files must call this before
/// `open`-ing any of them for real: `open` auto-migrates unconditionally,
/// and migrating a behind side mid-merge mints fresh, uncorrelated uuids
/// for its pre-existing rows — a merge that then unions by uuid duplicates
/// every one of them instead of reconciling. See CORRUPTION_LOG.md for a
/// real incident this caused.
pub fn require_matching_schema_versions(labeled: &[(&str, Option<i64>)]) -> CliResult<()> {
    let versions: std::collections::HashSet<i64> = labeled.iter().filter_map(|(_, v)| *v).collect();
    if versions.len() > 1 {
        let detail = labeled
            .iter()
            .map(|(name, v)| format!("{name}={v:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(user(format!(
            "refusing to merge: schema versions differ ({detail}) — migrating mid-merge would mint fresh, \
             uncorrelated uuids for whichever side is behind and duplicate every pre-existing task (see \
             CORRUPTION_LOG.md). Run any todo-sqlite-cli command (e.g. `todo-sqlite-cli doctor`) against the \
             database that's behind to migrate it to schema v{SCHEMA_VERSION} first, then retry."
        )));
    }
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

/// Base task columns, in the fixed order every `SELECT ... FROM tasks` in
/// this codebase uses. Shared so a query built anywhere can hand its rows
/// straight to `row_to_task_base` instead of re-deriving the column list.
pub const TASK_COLUMNS: &str =
    "id, uuid, title, details, status, priority, is_gate, created_at, started_at, completed_at, location";

pub fn row_to_task_base(row: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        uuid: row.get(1)?,
        title: row.get(2)?,
        details: row.get(3)?,
        status: row.get(4)?,
        priority: row.get(5)?,
        is_gate: row.get(6)?,
        tags: Vec::new(),
        depends_on: Vec::new(),
        blocked: false,
        created_at: row.get(7)?,
        started_at: row.get(8)?,
        completed_at: row.get(9)?,
        location: row.get(10)?,
        related: Vec::new(),
    })
}

pub fn load_task_by_uuid(conn: &Connection, task_uuid: &str) -> CliResult<Task> {
    let task = conn
        .query_row(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE uuid = ?1"),
            params![task_uuid],
            row_to_task_base,
        )
        .optional()
        .map_err(|e| system(format!("query failed: {e}")))?
        .ok_or_else(|| system(format!("task {task_uuid} vanished mid-operation")))?;
    hydrate(conn, task)
}

/// Resolve a user-supplied `<ID>` CLI argument — either a display id or a
/// full UUID — to exactly one task. A display id is unique per-node under
/// normal use, but a cross-node merge can legitimately leave two tasks
/// sharing one; rather than silently picking one (today's bug, worse once
/// `id` is explicitly non-unique) this lists every match and asks the
/// caller to re-run with the full uuid. No prefix matching: a short numeric
/// prefix like "12" is genuinely ambiguous between "display id 12" and "a
/// uuid starting with 12...", so only a full parse of either form is
/// accepted.
pub fn resolve_one(conn: &Connection, raw: &str) -> CliResult<Task> {
    if let Ok(id) = raw.parse::<i64>() {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1 ORDER BY uuid"
            ))
            .map_err(|e| system(format!("prepare failed: {e}")))?;
        let rows = stmt
            .query_map(params![id], row_to_task_base)
            .map_err(|e| system(format!("query failed: {e}")))?;
        let mut matches = Vec::new();
        for r in rows {
            matches.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
        }
        drop(stmt);
        return match matches.len() {
            0 => Err(user(format!("task {raw} not found"))),
            1 => hydrate(conn, matches.into_iter().next().unwrap()),
            n => {
                let mut msg = format!(
                    "task id {raw} is ambiguous ({n} matches) — re-run with the full uuid shown below to pick one:\n"
                );
                for t in &matches {
                    msg.push_str(&format!(
                        "  id={} uuid={} status={} title={}\n",
                        t.id, t.uuid, t.status, t.title
                    ));
                }
                Err(user(msg))
            }
        };
    }
    if uuid::Uuid::parse_str(raw).is_ok() {
        let task = conn
            .query_row(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE uuid = ?1"),
                params![raw],
                row_to_task_base,
            )
            .optional()
            .map_err(|e| system(format!("query failed: {e}")))?;
        return match task {
            Some(t) => hydrate(conn, t),
            None => Err(user(format!("task {raw} not found"))),
        };
    }
    Err(user(format!("'{raw}' is not a task id or UUID")))
}

fn hydrate(conn: &Connection, mut t: Task) -> CliResult<Task> {
    t.tags = load_tags(conn, &t.uuid)?;
    t.depends_on = load_deps(conn, &t.uuid)?;
    t.blocked = is_blocked(conn, &t.uuid)?;
    t.related = load_related(conn, &t.uuid)?;
    Ok(t)
}

pub fn load_tags(conn: &Connection, task_uuid: &str) -> CliResult<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT tag FROM tags WHERE task_uuid = ?1 ORDER BY tag")
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map(params![task_uuid], |r| r.get::<_, String>(0))
        .map_err(|e| system(format!("query failed: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
    }
    Ok(out)
}

/// Display ids of this task's dependency targets, for `Task.depends_on`
/// (text/JSON output). Joins through `uuid` so the id shown always reflects
/// the current display id of the dependency's row.
pub fn load_deps(conn: &Connection, task_uuid: &str) -> CliResult<Vec<i64>> {
    let mut stmt = conn
        .prepare(
            "SELECT t2.id FROM deps d
             JOIN tasks t2 ON t2.uuid = d.depends_on_uuid
             WHERE d.task_uuid = ?1 ORDER BY t2.id",
        )
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map(params![task_uuid], |r| r.get::<_, i64>(0))
        .map_err(|e| system(format!("query failed: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
    }
    Ok(out)
}

/// Uuid-space dependency targets — for the merge engine, which must join on
/// true identity rather than the (now possibly duplicate) display id.
pub fn load_dep_uuids(conn: &Connection, task_uuid: &str) -> CliResult<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT depends_on_uuid FROM deps WHERE task_uuid = ?1 ORDER BY depends_on_uuid")
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map(params![task_uuid], |r| r.get::<_, String>(0))
        .map_err(|e| system(format!("query failed: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
    }
    Ok(out)
}

/// Display ids of this task's mutually-linked "see also" tasks, for
/// `Task.related` (text/JSON output). Joins through `uuid` so the id shown
/// always reflects the current display id of the linked row.
pub fn load_related(conn: &Connection, task_uuid: &str) -> CliResult<Vec<i64>> {
    let mut stmt = conn
        .prepare(
            "SELECT t2.id FROM related r
             JOIN tasks t2 ON t2.uuid = r.related_uuid
             WHERE r.task_uuid = ?1 ORDER BY t2.id",
        )
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map(params![task_uuid], |r| r.get::<_, i64>(0))
        .map_err(|e| system(format!("query failed: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
    }
    Ok(out)
}

/// Uuid-space related-link targets — for the merge engine, which must join
/// on true identity rather than the (now possibly duplicate) display id.
pub fn load_related_uuids(conn: &Connection, task_uuid: &str) -> CliResult<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT related_uuid FROM related WHERE task_uuid = ?1 ORDER BY related_uuid")
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map(params![task_uuid], |r| r.get::<_, String>(0))
        .map_err(|e| system(format!("query failed: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
    }
    Ok(out)
}

pub fn is_blocked(conn: &Connection, task_uuid: &str) -> CliResult<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM deps d
             JOIN tasks t ON t.uuid = d.depends_on_uuid
             WHERE d.task_uuid = ?1 AND t.status <> 'done'",
            params![task_uuid],
            |r| r.get(0),
        )
        .map_err(|e| system(format!("query failed: {e}")))?;
    Ok(count > 0)
}

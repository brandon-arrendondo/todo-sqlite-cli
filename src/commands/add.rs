use std::path::Path;

use rusqlite::params;
use serde_json::json;

use crate::db::{self, Status};
use crate::error::{system, user, CliResult};

#[allow(clippy::too_many_arguments)]
pub fn run(
    db_path: &Path,
    json: bool,
    title: &str,
    details: Option<&str>,
    tags: &[String],
    priority: i64,
    depends_on: &[String],
    start: bool,
    gate: bool,
) -> CliResult<()> {
    if title.trim().is_empty() {
        return Err(user("title must not be empty"));
    }
    let mut conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let mut dep_uuids = Vec::new();
    for dep_id in depends_on {
        let dep = db::resolve_one(&conn, dep_id)
            .map_err(|e| user(format!("dependency {dep_id}: {e}")))?;
        dep_uuids.push(dep.uuid);
    }

    let tx = conn
        .transaction()
        .map_err(|e| system(format!("begin tx failed: {e}")))?;

    let now = db::now_iso();
    let (status, started_at) = if start {
        // Auto-move any in-progress task to 'partial' (preserving started_at).
        tx.execute(
            "UPDATE tasks SET status = 'partial' WHERE status = 'in-progress'",
            [],
        )
        .map_err(|e| system(format!("auto-move failed: {e}")))?;
        (Status::InProgress, Some(now.clone()))
    } else {
        (Status::Pending, None)
    };

    let uuid = uuid::Uuid::new_v4().to_string();
    let id: i64 = tx
        .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM tasks", [], |r| {
            r.get(0)
        })
        .map_err(|e| system(format!("next id query failed: {e}")))?;

    tx.execute(
        "INSERT INTO tasks(uuid, id, title, details, status, priority, is_gate, created_at, started_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            uuid,
            id,
            title,
            details,
            status.as_str(),
            priority,
            gate,
            now,
            started_at,
        ],
    )
    .map_err(|e| system(format!("insert failed: {e}")))?;

    for tag in dedup(tags) {
        tx.execute(
            "INSERT OR IGNORE INTO tags(task_uuid, tag) VALUES(?1, ?2)",
            params![uuid, tag],
        )
        .map_err(|e| system(format!("tag insert failed: {e}")))?;
    }
    for dep in dedup(&dep_uuids) {
        tx.execute(
            "INSERT OR IGNORE INTO deps(task_uuid, depends_on_uuid) VALUES(?1, ?2)",
            params![uuid, dep],
        )
        .map_err(|e| system(format!("dep insert failed: {e}")))?;
    }

    tx.commit()
        .map_err(|e| system(format!("commit failed: {e}")))?;

    let task = db::load_task_by_uuid(&conn, &uuid)?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&json!({"id": id, "task": task})).unwrap()
        );
    } else {
        println!("{id}");
    }
    Ok(())
}

fn dedup(v: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in v {
        if seen.insert(s.clone()) {
            out.push(s.clone());
        }
    }
    out
}

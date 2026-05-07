use std::path::Path;

use rusqlite::params;
use serde_json::json;

use crate::db::{self, Status};
use crate::error::{system, user, CliResult};

pub fn run(
    db_path: &Path,
    json: bool,
    title: &str,
    details: Option<&str>,
    tags: &[String],
    priority: i64,
    depends_on: &[i64],
    start: bool,
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

    for dep_id in depends_on {
        db::require_task_exists(&conn, *dep_id)?;
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

    tx.execute(
        "INSERT INTO tasks(title, details, status, priority, created_at, started_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        params![title, details, status.as_str(), priority, now, started_at,],
    )
    .map_err(|e| system(format!("insert failed: {e}")))?;
    let id = tx.last_insert_rowid();

    for tag in dedup(tags) {
        tx.execute(
            "INSERT OR IGNORE INTO tags(task_id, tag) VALUES(?1, ?2)",
            params![id, tag],
        )
        .map_err(|e| system(format!("tag insert failed: {e}")))?;
    }
    for dep in dedup_i64(depends_on) {
        if dep == id {
            return Err(user("a task cannot depend on itself"));
        }
        tx.execute(
            "INSERT OR IGNORE INTO deps(task_id, depends_on_id) VALUES(?1, ?2)",
            params![id, dep],
        )
        .map_err(|e| system(format!("dep insert failed: {e}")))?;
    }

    tx.commit()
        .map_err(|e| system(format!("commit failed: {e}")))?;

    let task = db::load_task(&conn, id)?;
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

fn dedup_i64(v: &[i64]) -> Vec<i64> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in v {
        if seen.insert(*s) {
            out.push(*s);
        }
    }
    out
}

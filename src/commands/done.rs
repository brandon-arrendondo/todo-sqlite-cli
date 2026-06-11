use std::path::Path;

use rusqlite::params;

use crate::db;
use crate::error::{system, user, CliResult};
use crate::format;

pub fn run(db_path: &Path, json: bool, id: i64, rejected: bool) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let target = if rejected { "rejected" } else { "done" };

    let current: Option<String> = conn
        .query_row("SELECT status FROM tasks WHERE id = ?1", params![id], |r| {
            r.get(0)
        })
        .ok();
    let current = current.ok_or_else(|| user(format!("task {id} not found")))?;

    if current != target {
        conn.execute(
            "UPDATE tasks SET status = ?1, completed_at = ?2, \
                              started_at = COALESCE(started_at, ?2) \
             WHERE id = ?3",
            params![target, db::now_iso(), id],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }

    let t = db::load_task(&conn, id)?;
    if json {
        format::print_task_json(&t);
    } else {
        println!("{target} {id}");
    }
    Ok(())
}

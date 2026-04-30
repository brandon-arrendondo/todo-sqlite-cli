use std::path::Path;

use rusqlite::params;

use crate::db::{self, Status};
use crate::error::{system, user, CliResult};
use crate::format;

pub fn run(db_path: &Path, json: bool, id: i64) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let current: Option<String> = conn
        .query_row("SELECT status FROM tasks WHERE id = ?1", params![id], |r| {
            r.get(0)
        })
        .ok();
    let current = current.ok_or_else(|| user(format!("task {id} not found")))?;

    if current == Status::Done.as_str() {
        return Err(user(format!("task {id} is done; cannot revert")));
    }

    conn.execute(
        "UPDATE tasks SET status = 'pending', started_at = NULL WHERE id = ?1",
        params![id],
    )
    .map_err(|e| system(format!("update failed: {e}")))?;

    let t = db::load_task(&conn, id)?;
    if json {
        format::print_task_json(&t);
    } else {
        println!("reverted {id}");
    }
    Ok(())
}

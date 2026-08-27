use std::path::Path;

use rusqlite::params;

use crate::db::{self, Status};
use crate::error::{system, user, CliResult};
use crate::format;

pub fn run(db_path: &Path, json: bool, id: &str) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let target = db::resolve_one(&conn, id)?;
    let display_id = target.id;

    if target.status == Status::Done.as_str() {
        return Err(user(format!("task {display_id} is done; cannot stop")));
    }

    if target.status == Status::InProgress.as_str() {
        conn.execute(
            "UPDATE tasks SET status = 'partial' WHERE uuid = ?1",
            params![target.uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }

    let t = db::load_task_by_uuid(&conn, &target.uuid)?;
    if json {
        format::print_task_json(&t);
    } else {
        println!("stopped {display_id}");
    }
    Ok(())
}

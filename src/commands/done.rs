use std::path::Path;

use rusqlite::params;

use crate::db;
use crate::error::{system, user, CliResult};
use crate::format;

pub fn run(db_path: &Path, json: bool, id: &str, rejected: bool) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let target_status = if rejected { "rejected" } else { "done" };
    let target = db::resolve_one(&conn, id)?;
    let display_id = target.id;

    if target.status != target_status {
        conn.execute(
            "UPDATE tasks SET status = ?1, completed_at = ?2, \
                              started_at = COALESCE(started_at, ?2) \
             WHERE uuid = ?3",
            params![target_status, db::now_iso(), target.uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }

    let t = db::load_task_by_uuid(&conn, &target.uuid)?;
    if json {
        format::print_task_json(&t);
    } else {
        println!("{target_status} {display_id}");
    }
    Ok(())
}

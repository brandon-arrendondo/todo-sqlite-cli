use std::path::Path;

use rusqlite::params;
use serde_json::json;

use crate::db;
use crate::error::{system, user, CliResult};

pub fn run(db_path: &Path, json: bool, id: i64) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }
    db::require_task_exists(&conn, id)?;

    let n = conn
        .execute("DELETE FROM tasks WHERE id = ?1", params![id])
        .map_err(|e| system(format!("delete failed: {e}")))?;
    if n == 0 {
        return Err(user(format!("task {id} not found")));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string(&json!({"deleted": id})).unwrap()
        );
    } else {
        println!("removed {id}");
    }
    Ok(())
}

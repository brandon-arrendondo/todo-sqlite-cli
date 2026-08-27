use std::path::Path;

use rusqlite::params;
use serde_json::json;

use crate::db;
use crate::error::{system, user, CliResult};

pub fn run(db_path: &Path, json: bool, id: &str) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }
    let target = db::resolve_one(&conn, id)?;
    let display_id = target.id;

    let n = conn
        .execute("DELETE FROM tasks WHERE uuid = ?1", params![target.uuid])
        .map_err(|e| system(format!("delete failed: {e}")))?;
    if n == 0 {
        return Err(user(format!("task {display_id} not found")));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string(&json!({"deleted": display_id, "uuid": target.uuid})).unwrap()
        );
    } else {
        println!("removed {display_id}");
    }
    Ok(())
}

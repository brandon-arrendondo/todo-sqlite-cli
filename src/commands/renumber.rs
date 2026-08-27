use std::path::Path;

use rusqlite::{params, OptionalExtension};
use serde_json::json;

use crate::db;
use crate::error::{system, user, CliResult};

pub fn run(db_path: &Path, json: bool, id: &str, new_id: i64, force: bool) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }
    if new_id < 1 {
        return Err(user(format!(
            "new id must be a positive integer (got {new_id})"
        )));
    }

    let target = db::resolve_one(&conn, id)?;
    let old_id = target.id;

    if old_id == new_id {
        return Err(user(format!("task {old_id} already has id {new_id}")));
    }

    if !force {
        let conflict: Option<String> = conn
            .query_row(
                "SELECT uuid FROM tasks WHERE id = ?1 AND uuid <> ?2",
                params![new_id, target.uuid],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| system(format!("query failed: {e}")))?;
        if let Some(other_uuid) = conflict {
            return Err(user(format!(
                "id {new_id} is already in use by task {other_uuid} — pass --force to leave both sharing it"
            )));
        }
    }

    let n = conn
        .execute(
            "UPDATE tasks SET id = ?1 WHERE uuid = ?2",
            params![new_id, target.uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    if n == 0 {
        return Err(system(format!("task {} vanished mid-operation", target.uuid)));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "uuid": target.uuid,
                "old_id": old_id,
                "id": new_id,
            }))
            .unwrap()
        );
    } else {
        println!("renumbered {old_id} -> {new_id}");
    }
    Ok(())
}

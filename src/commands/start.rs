use std::path::Path;

use rusqlite::params;

use crate::db::{self, Status};
use crate::error::{system, user, CliResult};
use crate::format;

pub fn run(db_path: &Path, json: bool, id: i64, force: bool) -> CliResult<()> {
    let mut conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let tx = conn
        .transaction()
        .map_err(|e| system(format!("begin tx failed: {e}")))?;

    let row: Option<(String,)> = tx
        .query_row("SELECT status FROM tasks WHERE id = ?1", params![id], |r| {
            Ok((r.get::<_, String>(0)?,))
        })
        .ok();
    let (current,) = row.ok_or_else(|| user(format!("task {id} not found")))?;

    if current == Status::InProgress.as_str() {
        // already in-progress — no-op, still print
    } else if current == Status::Done.as_str() {
        return Err(user(format!("task {id} is already done")));
    } else {
        if !force {
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM tasks WHERE status = 'in-progress'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| system(format!("query failed: {e}")))?;
            if count > 0 {
                return Err(user(
                    "another task is in-progress; finish it or pass --force",
                ));
            }
            let blocked: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM deps d \
                     JOIN tasks t ON t.id = d.depends_on_id \
                     WHERE d.task_id = ?1 AND t.status <> 'done'",
                    params![id],
                    |r| r.get(0),
                )
                .map_err(|e| system(format!("query failed: {e}")))?;
            if blocked > 0 {
                return Err(user(format!(
                    "task {id} has unmet dependencies; pass --force to override"
                )));
            }
        }
        tx.execute(
            "UPDATE tasks SET status = 'in-progress', started_at = COALESCE(started_at, ?1) \
             WHERE id = ?2",
            params![db::now_iso(), id],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }

    tx.commit()
        .map_err(|e| system(format!("commit failed: {e}")))?;

    let t = db::load_task(&conn, id)?;
    if json {
        format::print_task_json(&t);
    } else {
        println!("started {id}");
    }
    Ok(())
}

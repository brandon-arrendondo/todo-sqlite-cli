use std::path::Path;

use rusqlite::params;

use crate::db::{self, Status};
use crate::error::{system, user, CliResult};
use crate::format;

pub fn run(db_path: &Path, json: bool, id: &str, force: bool) -> CliResult<()> {
    let mut conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }
    let target = db::resolve_one(&conn, id)?;
    let uuid = target.uuid.clone();
    let display_id = target.id;

    let tx = conn
        .transaction()
        .map_err(|e| system(format!("begin tx failed: {e}")))?;

    let current = target.status;

    if current == Status::InProgress.as_str() {
        // already in-progress — no-op, still print
    } else if current == Status::Done.as_str() {
        return Err(user(format!("task {display_id} is already done")));
    } else {
        if !force {
            let blocked: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM deps d \
                     JOIN tasks t ON t.uuid = d.depends_on_uuid \
                     WHERE d.task_uuid = ?1 AND t.status <> 'done'",
                    params![uuid],
                    |r| r.get(0),
                )
                .map_err(|e| system(format!("query failed: {e}")))?;
            if blocked > 0 {
                return Err(user(format!(
                    "task {display_id} has unmet dependencies; pass --force to override"
                )));
            }
        }
        // Auto-move any other in-progress task to 'partial' (preserving started_at)
        // unless --force was passed (which keeps multiple in-progress).
        if !force {
            tx.execute(
                "UPDATE tasks SET status = 'partial' \
                 WHERE status = 'in-progress' AND uuid <> ?1",
                params![uuid],
            )
            .map_err(|e| system(format!("auto-move failed: {e}")))?;
        }
        tx.execute(
            "UPDATE tasks SET status = 'in-progress', started_at = COALESCE(started_at, ?1) \
             WHERE uuid = ?2",
            params![db::now_iso(), uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }

    tx.commit()
        .map_err(|e| system(format!("commit failed: {e}")))?;

    let t = db::load_task_by_uuid(&conn, &uuid)?;
    if json {
        format::print_task_json(&t);
    } else {
        println!("started {display_id}");
    }
    Ok(())
}

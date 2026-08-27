use std::path::Path;

use rusqlite::Connection;

use crate::db::{self, Task};
use crate::error::{system, user, CliResult};
use crate::format;

pub fn run(db_path: &Path, json: bool) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    // A gate is never "the task to work on next" — there's no start/stop/
    // partial episode that makes sense for it; someone periodically
    // re-assesses its condition and calls `done` directly. Exclude gates
    // from every tier below rather than surfacing one.

    // 1. Oldest in-progress
    let winner: Option<String> = conn
        .query_row(
            "SELECT uuid FROM tasks WHERE status = 'in-progress' AND is_gate = 0 \
             ORDER BY started_at ASC, id ASC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();

    // 2. Highest-priority partial that is not blocked (resume paused work first)
    let winner = match winner {
        Some(v) => Some(v),
        None => conn
            .query_row(
                "SELECT uuid FROM tasks t \
                 WHERE status = 'partial' AND is_gate = 0 \
                   AND NOT EXISTS (\
                     SELECT 1 FROM deps d \
                     JOIN tasks td ON td.uuid = d.depends_on_uuid \
                     WHERE d.task_uuid = t.uuid AND td.status <> 'done'\
                   ) \
                 ORDER BY priority ASC, started_at ASC, id ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok(),
    };

    // 3. Highest-priority pending that is not blocked
    let winner = match winner {
        Some(v) => Some(v),
        None => conn
            .query_row(
                "SELECT uuid FROM tasks t \
                 WHERE status = 'pending' AND is_gate = 0 \
                   AND NOT EXISTS (\
                     SELECT 1 FROM deps d \
                     JOIN tasks td ON td.uuid = d.depends_on_uuid \
                     WHERE d.task_uuid = t.uuid AND td.status <> 'done'\
                   ) \
                 ORDER BY priority ASC, created_at ASC, id ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok(),
    };

    match winner {
        Some(u) => {
            let t: Task = db::load_task_by_uuid(&conn, &u)?;
            if json {
                format::print_task_json(&t);
            } else {
                format::print_task_text(&t, false);
            }
        }
        None => {
            if json {
                println!("null");
            } else if has_open_gate(&conn)? {
                eprintln!(
                    "no actionable task — only gate(s) remain open; see `todo-sqlite-cli list --kind gate`"
                );
            } else {
                // nothing to say — stay silent so scripts can branch on empty output
            }
        }
    }
    Ok(())
}

fn has_open_gate(conn: &Connection) -> CliResult<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks \
             WHERE is_gate = 1 AND status IN ('pending','partial','in-progress')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| system(format!("query failed: {e}")))?;
    Ok(count > 0)
}

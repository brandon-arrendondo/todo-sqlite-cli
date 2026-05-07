use std::path::Path;

use crate::db::{self, Task};
use crate::error::{user, CliResult};
use crate::format;

pub fn run(db_path: &Path, json: bool) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    // 1. Oldest in-progress
    let id: Option<i64> = conn
        .query_row(
            "SELECT id FROM tasks WHERE status = 'in-progress' \
             ORDER BY started_at ASC, id ASC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();

    // 2. Highest-priority partial that is not blocked (resume paused work first)
    let id = match id {
        Some(v) => Some(v),
        None => conn
            .query_row(
                "SELECT id FROM tasks t \
                 WHERE status = 'partial' \
                   AND NOT EXISTS (\
                     SELECT 1 FROM deps d \
                     JOIN tasks td ON td.id = d.depends_on_id \
                     WHERE d.task_id = t.id AND td.status <> 'done'\
                   ) \
                 ORDER BY priority ASC, started_at ASC, id ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok(),
    };

    // 3. Highest-priority pending that is not blocked
    let id = match id {
        Some(v) => Some(v),
        None => conn
            .query_row(
                "SELECT id FROM tasks t \
                 WHERE status = 'pending' \
                   AND NOT EXISTS (\
                     SELECT 1 FROM deps d \
                     JOIN tasks td ON td.id = d.depends_on_id \
                     WHERE d.task_id = t.id AND td.status <> 'done'\
                   ) \
                 ORDER BY priority ASC, created_at ASC, id ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok(),
    };

    match id {
        Some(i) => {
            let t: Task = db::load_task(&conn, i)?;
            if json {
                format::print_task_json(&t);
            } else {
                format::print_task_text(&t, false);
            }
        }
        None => {
            if json {
                println!("null");
            } else {
                // nothing to say — stay silent so scripts can branch on empty output
            }
        }
    }
    Ok(())
}

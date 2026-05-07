use std::path::Path;

use crate::db::{self, Task};
use crate::error::{system, user, CliResult};
use crate::format;

pub fn run(db_path: &Path, _json_flag: bool, fmt: &str, verbose: bool) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let mut stmt = conn
        .prepare(
            "SELECT id, title, details, status, priority, created_at, started_at, completed_at \
             FROM tasks WHERE status IN ('in-progress','partial','pending') \
             ORDER BY CASE status \
                       WHEN 'in-progress' THEN 0 \
                       WHEN 'partial' THEN 1 \
                       WHEN 'pending' THEN 2 END, \
                      priority ASC, created_at ASC, id ASC",
        )
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Task {
                id: row.get(0)?,
                title: row.get(1)?,
                details: row.get(2)?,
                status: row.get(3)?,
                priority: row.get(4)?,
                tags: Vec::new(),
                depends_on: Vec::new(),
                blocked: false,
                created_at: row.get(5)?,
                started_at: row.get(6)?,
                completed_at: row.get(7)?,
            })
        })
        .map_err(|e| system(format!("query failed: {e}")))?;

    let mut tasks: Vec<Task> = Vec::new();
    for r in rows {
        tasks.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
    }
    for t in tasks.iter_mut() {
        t.tags = db::load_tags(&conn, t.id)?;
        t.depends_on = db::load_deps(&conn, t.id)?;
        t.blocked = db::is_blocked(&conn, t.id)?;
    }

    match fmt {
        "json" => format::print_tasks_json(&tasks),
        "markdown" => {
            print!("{}", format::markdown_todo(&tasks, verbose));
        }
        other => {
            return Err(user(format!(
                "invalid --format '{other}' (expected json|markdown)"
            )))
        }
    }
    Ok(())
}

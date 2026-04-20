use std::path::Path;

use rusqlite::params_from_iter;
use rusqlite::types::Value;

use crate::db::{self, Task};
use crate::error::{system, user, CliResult};
use crate::format;

pub fn run(
    db_path: &Path,
    _json_flag: bool,
    since: Option<&str>,
    until: Option<&str>,
) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let mut sql = String::from(
        "SELECT id, title, details, status, priority, created_at, started_at, completed_at \
         FROM tasks WHERE status = 'done'",
    );
    let mut params: Vec<Value> = Vec::new();
    if let Some(s) = since {
        let norm = db::parse_date_bound(s)?;
        let idx = params.len() + 1;
        sql.push_str(&format!(" AND completed_at >= ?{idx}"));
        params.push(Value::Text(norm));
    }
    if let Some(u) = until {
        let norm = db::parse_date_bound(u)?;
        let idx = params.len() + 1;
        sql.push_str(&format!(" AND completed_at < ?{idx}"));
        params.push(Value::Text(norm));
    }
    sql.push_str(" ORDER BY completed_at DESC, id DESC");

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map(params_from_iter(params.iter()), |row| {
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
        let t = r.map_err(|e| system(format!("row read failed: {e}")))?;
        tasks.push(t);
    }
    for t in tasks.iter_mut() {
        t.tags = db::load_tags(&conn, t.id)?;
        t.depends_on = db::load_deps(&conn, t.id)?;
    }

    format::print_completed_json(&tasks);
    Ok(())
}

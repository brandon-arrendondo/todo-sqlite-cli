use std::path::Path;

use rusqlite::params_from_iter;
use rusqlite::types::Value;

use crate::db::{self, Task};
use crate::error::{system, user, CliResult};
use crate::format;

pub fn run(
    db_path: &Path,
    json: bool,
    status: &str,
    tags: &[String],
    limit: Option<i64>,
) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let status_clause = match status {
        "active" => "status IN ('in-progress','pending')",
        "all" => "1=1",
        "pending" | "in-progress" | "done" => "status = ?S",
        other => {
            return Err(user(format!(
                "invalid --status '{other}' (expected pending|in-progress|done|active|all)"
            )))
        }
    };

    let mut sql = String::from(
        "SELECT id, title, details, status, priority, created_at, started_at, completed_at \
         FROM tasks WHERE ",
    );
    sql.push_str(status_clause);

    let mut params: Vec<Value> = Vec::new();
    if status_clause.contains("?S") {
        sql = sql.replace("?S", &format!("?{}", params.len() + 1));
        params.push(Value::Text(status.to_string()));
    }

    if !tags.is_empty() {
        for tag in tags {
            let idx = params.len() + 1;
            sql.push_str(&format!(
                " AND id IN (SELECT task_id FROM tags WHERE tag = ?{idx})"
            ));
            params.push(Value::Text(tag.clone()));
        }
    }

    // Order: in-progress first, then pending, then done; within each, priority ASC, created_at ASC.
    sql.push_str(
        " ORDER BY CASE status \
           WHEN 'in-progress' THEN 0 \
           WHEN 'pending' THEN 1 \
           WHEN 'done' THEN 2 END, \
         priority ASC, created_at ASC, id ASC",
    );
    if let Some(n) = limit {
        sql.push_str(&format!(" LIMIT {n}"));
    }

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
        t.blocked = db::is_blocked(&conn, t.id)?;
    }

    if json {
        format::print_tasks_json(&tasks);
    } else {
        format::print_tasks_table(&tasks);
    }
    Ok(())
}

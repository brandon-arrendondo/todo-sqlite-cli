use std::path::Path;

use rusqlite::params_from_iter;
use rusqlite::types::Value;

use crate::db::{self, Task};
use crate::error::{system, user, CliResult};
use crate::format;

#[allow(clippy::too_many_arguments)]
pub fn run(
    db_path: &Path,
    json: bool,
    status: &str,
    tags: &[String],
    limit: Option<i64>,
    fmt: &str,
    since: Option<&str>,
    ids_only: bool,
    verbose: bool,
    kind: &str,
    unblocked: bool,
) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let (sql, params) = build_list_query(status, tags, since, limit, kind)?;

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map(params_from_iter(params.iter()), db::row_to_task_base)
        .map_err(|e| system(format!("query failed: {e}")))?;

    let mut tasks: Vec<Task> = Vec::new();
    for r in rows {
        let t = r.map_err(|e| system(format!("row read failed: {e}")))?;
        tasks.push(t);
    }
    for t in tasks.iter_mut() {
        t.tags = db::load_tags(&conn, &t.uuid)?;
        t.depends_on = db::load_deps(&conn, &t.uuid)?;
        t.blocked = db::is_blocked(&conn, &t.uuid)?;
    }

    if unblocked {
        tasks.retain(|t| !t.blocked);
    }

    if ids_only {
        format::print_ids(&tasks, json);
        return Ok(());
    }

    // `--format` is the source of truth; `--json` is shorthand for `json` when
    // `--format` is left at its default. An explicit `--format ndjson --json`
    // means the user wanted NDJSON.
    let effective_fmt = if fmt != "table" {
        fmt
    } else if json {
        "json"
    } else {
        "table"
    };

    print_tasks(&tasks, effective_fmt, verbose)
}

/// Build the `SELECT` and its positional params for the given filters.
fn build_list_query(
    status: &str,
    tags: &[String],
    since: Option<&str>,
    limit: Option<i64>,
    kind: &str,
) -> CliResult<(String, Vec<Value>)> {
    let status_clause = match status {
        "active" => "status IN ('in-progress','partial','pending')",
        "all" => "1=1",
        "pending" | "partial" | "in-progress" | "done" | "rejected" => "status = ?S",
        other => {
            return Err(user(format!(
                "invalid --status '{other}' (expected pending|partial|in-progress|done|rejected|active|all)"
            )))
        }
    };

    let mut sql = format!("SELECT {} FROM tasks WHERE ", db::TASK_COLUMNS);
    sql.push_str(status_clause);

    let mut params: Vec<Value> = Vec::new();
    if status_clause.contains("?S") {
        sql = sql.replace("?S", &format!("?{}", params.len() + 1));
        params.push(Value::Text(status.to_string()));
    }

    match kind {
        "all" => {}
        "gate" => sql.push_str(" AND is_gate = 1"),
        "task" => sql.push_str(" AND is_gate = 0"),
        other => {
            return Err(user(format!(
                "invalid --kind '{other}' (expected gate|task|all)"
            )))
        }
    }

    for tag in tags {
        let idx = params.len() + 1;
        sql.push_str(&format!(
            " AND uuid IN (SELECT task_uuid FROM tags WHERE tag = ?{idx})"
        ));
        params.push(Value::Text(tag.clone()));
    }

    if let Some(s) = since {
        let norm = db::parse_date_bound(s)?;
        let idx = params.len() + 1;
        sql.push_str(&format!(" AND created_at >= ?{idx}"));
        params.push(Value::Text(norm));
    }

    // Order: in-progress, partial, pending, done, rejected; within each, priority ASC, created_at ASC.
    sql.push_str(
        " ORDER BY CASE status \
           WHEN 'in-progress' THEN 0 \
           WHEN 'partial' THEN 1 \
           WHEN 'pending' THEN 2 \
           WHEN 'done' THEN 3 \
           WHEN 'rejected' THEN 4 END, \
         priority ASC, created_at ASC, id ASC",
    );
    if let Some(n) = limit {
        sql.push_str(&format!(" LIMIT {n}"));
    }

    Ok((sql, params))
}

/// Render the resolved task list in the requested output format.
fn print_tasks(tasks: &[Task], effective_fmt: &str, verbose: bool) -> CliResult<()> {
    match effective_fmt {
        "table" => format::print_tasks_table(tasks),
        "json" => format::print_tasks_json(tasks),
        "ndjson" => format::print_tasks_ndjson(tasks),
        "markdown" => print!("{}", format::markdown_todo(tasks, verbose)),
        other => {
            return Err(user(format!(
                "invalid --format '{other}' (expected table|json|ndjson|markdown)"
            )))
        }
    }
    Ok(())
}

use std::path::Path;

use chrono::Utc;
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use serde::Serialize;
use serde_json::json;

use crate::db::{self, Task};
use crate::error::{system, user, CliResult};

#[derive(Serialize)]
struct AgingRow {
    id: i64,
    title: String,
    status: String,
    priority: i64,
    tags: Vec<String>,
    created_at: String,
    age_days: i64,
    stale: bool,
}

pub fn run(db_path: &Path, json: bool, stale_days: Option<i64>, tags: &[String]) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let mut sql = String::from(
        "SELECT id, title, details, status, priority, created_at, started_at, completed_at \
         FROM tasks WHERE status IN ('pending','partial','in-progress')",
    );
    let mut params: Vec<Value> = Vec::new();
    for tag in tags {
        let idx = params.len() + 1;
        sql.push_str(&format!(
            " AND id IN (SELECT task_id FROM tags WHERE tag = ?{idx})"
        ));
        params.push(Value::Text(tag.clone()));
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
        tasks.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
    }
    for t in tasks.iter_mut() {
        t.tags = db::load_tags(&conn, t.id)?;
    }

    let now = Utc::now();
    let mut aging_rows: Vec<AgingRow> = tasks
        .iter()
        .map(|t| {
            let age_days = age_in_days(&t.created_at, now);
            let stale = stale_days.is_some_and(|n| age_days >= n);
            AgingRow {
                id: t.id,
                title: t.title.clone(),
                status: t.status.clone(),
                priority: t.priority,
                tags: t.tags.clone(),
                created_at: t.created_at.clone(),
                age_days,
                stale,
            }
        })
        .collect();

    // Oldest first; among ties, higher (numerically smaller) priority first.
    aging_rows.sort_by(|a, b| {
        b.age_days
            .cmp(&a.age_days)
            .then(a.priority.cmp(&b.priority))
    });

    if json {
        let v = json!({ "tasks": aging_rows });
        println!("{}", serde_json::to_string(&v).unwrap());
    } else {
        print_table(&aging_rows);
    }
    Ok(())
}

/// Whole-day age of `created_at` (RFC3339) as of `now`. Malformed timestamps
/// (shouldn't occur — the column is always written by this CLI) age as 0
/// rather than erroring out a report command.
fn age_in_days(created_at: &str, now: chrono::DateTime<Utc>) -> i64 {
    match chrono::DateTime::parse_from_rfc3339(created_at) {
        Ok(dt) => (now - dt.with_timezone(&Utc)).num_days(),
        Err(_) => 0,
    }
}

fn print_table(rows: &[AgingRow]) {
    for r in rows {
        let flag = if r.stale { " [STALE]" } else { "" };
        let tags = if r.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", r.tags.join(","))
        };
        println!(
            "{:>4}  {:<11}  P{}  age={:>4}d  {}{}{}",
            r.id, r.status, r.priority, r.age_days, r.title, tags, flag
        );
    }
}

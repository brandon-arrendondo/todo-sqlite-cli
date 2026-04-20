use std::path::Path;

use rusqlite::params;

use crate::db;
use crate::error::{system, user, CliResult};
use crate::format;

pub fn run(
    db_path: &Path,
    json: bool,
    id: i64,
    title: Option<&str>,
    details: Option<&str>,
    clear_details: bool,
    priority: Option<i64>,
    add_tag: &[String],
    rm_tag: &[String],
    add_dep: &[i64],
    rm_dep: &[i64],
) -> CliResult<()> {
    if details.is_some() && clear_details {
        return Err(user("--details and --clear-details are mutually exclusive"));
    }
    let mut conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }
    db::require_task_exists(&conn, id)?;

    let tx = conn
        .transaction()
        .map_err(|e| system(format!("begin tx failed: {e}")))?;

    if let Some(t) = title {
        if t.trim().is_empty() {
            return Err(user("title must not be empty"));
        }
        tx.execute("UPDATE tasks SET title = ?1 WHERE id = ?2", params![t, id])
            .map_err(|e| system(format!("update failed: {e}")))?;
    }
    if let Some(d) = details {
        tx.execute(
            "UPDATE tasks SET details = ?1 WHERE id = ?2",
            params![d, id],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }
    if clear_details {
        tx.execute("UPDATE tasks SET details = NULL WHERE id = ?1", params![id])
            .map_err(|e| system(format!("update failed: {e}")))?;
    }
    if let Some(p) = priority {
        tx.execute(
            "UPDATE tasks SET priority = ?1 WHERE id = ?2",
            params![p, id],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }
    for tag in add_tag {
        tx.execute(
            "INSERT OR IGNORE INTO tags(task_id, tag) VALUES(?1, ?2)",
            params![id, tag],
        )
        .map_err(|e| system(format!("tag insert failed: {e}")))?;
    }
    for tag in rm_tag {
        tx.execute(
            "DELETE FROM tags WHERE task_id = ?1 AND tag = ?2",
            params![id, tag],
        )
        .map_err(|e| system(format!("tag delete failed: {e}")))?;
    }
    for dep in add_dep {
        if *dep == id {
            return Err(user("a task cannot depend on itself"));
        }
        // verify dep exists (without ? operator in closure)
        let exists: Option<i64> = tx
            .query_row("SELECT id FROM tasks WHERE id = ?1", params![dep], |r| {
                r.get(0)
            })
            .ok();
        if exists.is_none() {
            return Err(user(format!("dependency task {dep} not found")));
        }
        if would_create_cycle(&tx, id, *dep)? {
            return Err(user(format!(
                "adding dependency {dep} would create a cycle"
            )));
        }
        tx.execute(
            "INSERT OR IGNORE INTO deps(task_id, depends_on_id) VALUES(?1, ?2)",
            params![id, dep],
        )
        .map_err(|e| system(format!("dep insert failed: {e}")))?;
    }
    for dep in rm_dep {
        tx.execute(
            "DELETE FROM deps WHERE task_id = ?1 AND depends_on_id = ?2",
            params![id, dep],
        )
        .map_err(|e| system(format!("dep delete failed: {e}")))?;
    }

    tx.commit()
        .map_err(|e| system(format!("commit failed: {e}")))?;

    let t = db::load_task(&conn, id)?;
    if json {
        format::print_task_json(&t);
    } else {
        println!("edited {id}");
    }
    Ok(())
}

fn would_create_cycle(tx: &rusqlite::Transaction, task_id: i64, new_dep: i64) -> CliResult<bool> {
    // Adding task_id -> new_dep creates a cycle iff new_dep already depends
    // (transitively) on task_id. DFS from new_dep's dependencies.
    let mut stack = vec![new_dep];
    let mut seen = std::collections::HashSet::new();
    while let Some(node) = stack.pop() {
        if node == task_id {
            return Ok(true);
        }
        if !seen.insert(node) {
            continue;
        }
        let mut stmt = tx
            .prepare("SELECT depends_on_id FROM deps WHERE task_id = ?1")
            .map_err(|e| system(format!("prepare failed: {e}")))?;
        let rows = stmt
            .query_map(params![node], |r| r.get::<_, i64>(0))
            .map_err(|e| system(format!("query failed: {e}")))?;
        for r in rows {
            stack.push(r.map_err(|e| system(format!("row failed: {e}")))?);
        }
    }
    Ok(false)
}

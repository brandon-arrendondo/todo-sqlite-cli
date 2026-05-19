use std::path::Path;

use crate::db;
use crate::error::{user, CliResult};
use crate::format;

pub fn run(db_path: &Path, json: bool, id: i64, verbose: bool, fmt: &str) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }
    let t = db::load_task(&conn, id)?;

    let effective_fmt = if fmt != "text" {
        fmt
    } else if json {
        "json"
    } else {
        "text"
    };

    match effective_fmt {
        "text" => format::print_task_text(&t, verbose),
        "json" => format::print_task_json(&t),
        "ndjson" => format::print_tasks_ndjson(std::slice::from_ref(&t)),
        other => {
            return Err(user(format!(
                "invalid --format '{other}' (expected text|json|ndjson)"
            )))
        }
    }
    Ok(())
}

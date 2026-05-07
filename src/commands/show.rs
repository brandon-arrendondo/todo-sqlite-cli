use std::path::Path;

use crate::db;
use crate::error::{user, CliResult};
use crate::format;

pub fn run(db_path: &Path, json: bool, id: i64, verbose: bool) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }
    let t = db::load_task(&conn, id)?;
    if json {
        format::print_task_json(&t);
    } else {
        format::print_task_text(&t, verbose);
    }
    Ok(())
}

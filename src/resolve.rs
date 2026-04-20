use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{user, CliResult};

pub const MARKER_FILENAME: &str = ".todo-sqlite-cli";
pub const ENV_VAR: &str = "TODO_SQLITE_CLI_DB";

pub fn resolve_db_path(flag: Option<&Path>) -> CliResult<PathBuf> {
    if let Some(p) = flag {
        return Ok(p.to_path_buf());
    }
    if let Ok(val) = env::var(ENV_VAR) {
        if !val.is_empty() {
            return Ok(PathBuf::from(val));
        }
    }
    let cwd =
        env::current_dir().map_err(|e| user(format!("cannot read current directory: {e}")))?;
    if let Some((marker_dir, db_path)) = find_marker(&cwd)? {
        let resolved = if db_path.is_absolute() {
            db_path
        } else {
            marker_dir.join(db_path)
        };
        return Ok(resolved);
    }
    Err(user(format!(
        "no database found. Set --db, ${ENV_VAR}, or run `todo-sqlite-cli init` to create one."
    )))
}

fn find_marker(start: &Path) -> CliResult<Option<(PathBuf, PathBuf)>> {
    let mut dir = Some(start.to_path_buf());
    while let Some(d) = dir {
        let candidate = d.join(MARKER_FILENAME);
        if candidate.is_file() {
            let content = fs::read_to_string(&candidate)
                .map_err(|e| user(format!("cannot read {}: {e}", candidate.display())))?;
            let first_line = content.lines().next().unwrap_or("").trim();
            if first_line.is_empty() {
                return Err(user(format!(
                    "marker file {} is empty; first line must be the DB path",
                    candidate.display()
                )));
            }
            return Ok(Some((d, PathBuf::from(first_line))));
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    Ok(None)
}

pub fn write_marker(dir: &Path, db_path: &Path) -> CliResult<PathBuf> {
    let marker = dir.join(MARKER_FILENAME);
    let content = format!("{}\n", db_path.display());
    fs::write(&marker, content)
        .map_err(|e| user(format!("cannot write {}: {e}", marker.display())))?;
    Ok(marker)
}

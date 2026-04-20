use std::env;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::db;
use crate::error::{user, CliResult};
use crate::resolve;

pub fn run(db_flag: Option<&Path>, marker_dir: Option<&Path>, json: bool) -> CliResult<()> {
    let (db_path, marker_path) = resolve_init_paths(db_flag, marker_dir)?;

    if db_path.exists() {
        return Err(user(format!(
            "database already exists at {}; refusing to clobber",
            db_path.display()
        )));
    }
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                user(format!(
                    "cannot create parent directory {}: {e}",
                    parent.display()
                ))
            })?;
        }
    }

    let conn = db::open(&db_path)?;
    db::create_schema(&conn)?;

    let written_marker = if let Some(dir) = marker_path.as_ref() {
        Some(resolve::write_marker(dir, &db_path)?)
    } else {
        None
    };

    if json {
        let out = json!({
            "db": db_path.display().to_string(),
            "marker": written_marker.as_ref().map(|p| p.display().to_string()),
            "schema_version": db::SCHEMA_VERSION,
        });
        println!("{}", serde_json::to_string(&out).unwrap());
    } else {
        println!("initialized {}", db_path.display());
        if let Some(m) = &written_marker {
            println!("wrote marker {}", m.display());
        }
    }
    Ok(())
}

fn resolve_init_paths(
    db_flag: Option<&Path>,
    marker_dir: Option<&Path>,
) -> CliResult<(PathBuf, Option<PathBuf>)> {
    if let Some(p) = db_flag {
        // With --db, no marker is written (caller is dictating the path explicitly).
        return Ok((p.to_path_buf(), None));
    }
    let dir = match marker_dir {
        Some(d) => d.to_path_buf(),
        None => {
            env::current_dir().map_err(|e| user(format!("cannot read current directory: {e}")))?
        }
    };
    let db_path = dir.join("todo-sqlite-cli.db");
    Ok((db_path, Some(dir)))
}

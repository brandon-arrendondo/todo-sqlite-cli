use std::path::{Path, PathBuf};

use serde_json::json;

use crate::db;
use crate::error::{system, user, CliResult};
use crate::merge::{merge_databases, MergeOptions, MergeReport};

#[allow(clippy::too_many_arguments)]
pub fn run(
    json: bool,
    base: Option<&Path>,
    ours: &Path,
    theirs: &Path,
    into: Option<&Path>,
    strict: bool,
) -> CliResult<()> {
    if !ours.exists() {
        return Err(user(format!(
            "--ours database not found: {}",
            ours.display()
        )));
    }
    if !theirs.exists() {
        return Err(user(format!(
            "--theirs database not found: {}",
            theirs.display()
        )));
    }

    let base_usable = base.is_some_and(is_usable_db);

    db::require_matching_schema_versions(&[
        ("ours", db::peek_schema_version(ours)?),
        ("theirs", db::peek_schema_version(theirs)?),
        (
            "base",
            if base_usable {
                db::peek_schema_version(base.unwrap())?
            } else {
                None
            },
        ),
    ])?;

    let base_conn = match base {
        Some(p) if base_usable => Some(db::open(p)?),
        _ => None,
    };
    let ours_conn = db::open(ours)?;
    let theirs_conn = db::open(theirs)?;

    let out_path = into
        .map(Path::to_path_buf)
        .unwrap_or_else(|| ours.to_path_buf());
    let tmp_path = temp_path_near(&out_path);

    let opts = MergeOptions { strict };
    let report = merge_databases(
        base_conn.as_ref(),
        &ours_conn,
        &theirs_conn,
        &tmp_path,
        opts,
    )?;

    if strict && !report.conflicts.is_empty() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(user(format!(
            "{} conflict(s) found; aborting (--strict). Re-run without --strict to auto-resolve and flag them with the '{}' tag.",
            report.conflicts.len(),
            "merge-conflict"
        )));
    }

    std::fs::rename(&tmp_path, &out_path).map_err(|e| {
        system(format!(
            "cannot write merge result to {}: {e}",
            out_path.display()
        ))
    })?;

    print_report(json, &report, &out_path);
    Ok(())
}

fn is_usable_db(p: &Path) -> bool {
    p.exists() && p.metadata().map(|m| m.len() > 0).unwrap_or(false)
}

fn temp_path_near(out_path: &Path) -> PathBuf {
    let mut tmp = out_path.to_path_buf();
    let name = tmp
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "merged".to_string());
    tmp.set_file_name(format!(".{name}.merging.tmp"));
    tmp
}

fn print_report(json: bool, report: &MergeReport, out_path: &Path) {
    if json {
        let v = json!({
            "into": out_path.display().to_string(),
            "tasks_total": report.tasks_total,
            "auto_resolved": report.auto_resolved,
            "conflicts": report.conflicts,
        });
        println!("{}", serde_json::to_string(&v).unwrap());
        return;
    }
    println!("merged into {}", out_path.display());
    println!("tasks: {}", report.tasks_total);
    if report.auto_resolved > 0 {
        println!("auto-resolved (no review needed): {}", report.auto_resolved);
    }
    if report.conflicts.is_empty() {
        println!("conflicts: 0");
    } else {
        println!(
            "conflicts: {} (tagged 'merge-conflict' — see `list --tag merge-conflict`)",
            report.conflicts.len()
        );
        for c in &report.conflicts {
            println!(
                "  task {}: {} — ours={:?} theirs={:?} ({})",
                c.task_id, c.field, c.ours, c.theirs, c.resolution
            );
        }
    }
}

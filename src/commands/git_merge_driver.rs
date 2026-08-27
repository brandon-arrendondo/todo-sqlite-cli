use std::path::Path;

use crate::db;
use crate::error::{system, user, CliResult};
use crate::merge::{merge_databases, MergeOptions};

/// Implements git's `merge.<driver>.driver` contract: called as
/// `<cmd> %O %A %B` with three temp files holding the common-ancestor,
/// "ours", and "theirs" content. The merge result must be written back into
/// the `ours` path — that's what git copies into the working tree. `base`
/// may be missing or empty when there is no common ancestor (e.g. the file
/// was added independently on both sides); that degrades to a 2-way union
/// merge instead of a 3-way field merge. Identity is the uuid primary key,
/// so a task id colliding across nodes is not a merge-time problem — see
/// `db::resolve_one`.
pub fn run(base: &Path, ours: &Path, theirs: &Path) -> CliResult<()> {
    if !ours.exists() {
        return Err(user(format!("'ours' file not found: {}", ours.display())));
    }
    if !theirs.exists() {
        return Err(user(format!(
            "'theirs' file not found: {}",
            theirs.display()
        )));
    }

    let base_present = base.exists() && base.metadata().map(|m| m.len() > 0).unwrap_or(false);

    // Peek schema versions *before* opening anything for real — see
    // `db::require_matching_schema_versions` for why.
    db::require_matching_schema_versions(&[
        ("ours", db::peek_schema_version(ours)?),
        ("theirs", db::peek_schema_version(theirs)?),
        (
            "base",
            if base_present {
                db::peek_schema_version(base)?
            } else {
                None
            },
        ),
    ])?;

    let base_conn = if base_present {
        Some(db::open(base)?)
    } else {
        None
    };
    let ours_conn = db::open(ours)?;
    let theirs_conn = db::open(theirs)?;

    let mut tmp = ours.to_path_buf();
    let name = tmp
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "merged".to_string());
    tmp.set_file_name(format!(".{name}.merging.tmp"));

    let report = merge_databases(
        base_conn.as_ref(),
        &ours_conn,
        &theirs_conn,
        &tmp,
        MergeOptions { strict: false },
    )?;

    drop(ours_conn);
    drop(theirs_conn);
    std::fs::rename(&tmp, ours).map_err(|e| {
        system(format!(
            "cannot write merge result to {}: {e}",
            ours.display()
        ))
    })?;

    println!(
        "todo-sqlite-cli: merged {} task(s), {} conflict(s)",
        report.tasks_total,
        report.conflicts.len(),
    );

    if !report.conflicts.is_empty() {
        for c in &report.conflicts {
            eprintln!(
                "  task {}: {} — ours={:?} theirs={:?} ({})",
                c.task_id, c.field, c.ours, c.theirs, c.resolution
            );
        }
        return Err(user(format!(
            "{} conflict(s) need review — tagged 'merge-conflict' in the merged file (run `todo-sqlite-cli list --tag merge-conflict` after resolving)",
            report.conflicts.len()
        )));
    }
    Ok(())
}

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::db;
use crate::error::{system, CliResult};

const CONFLICT_TAG: &str = "merge-conflict";

/// A task row plus its tags/deps, loaded independently of any resolved id
/// mapping — exactly what's on disk in one of the three input databases.
/// Identity is `uuid`; `id` is carried along purely as the display value to
/// write back out (it is never used to join across databases).
#[derive(Debug, Clone)]
struct RawTask {
    uuid: String,
    id: i64,
    title: String,
    details: Option<String>,
    status: String,
    priority: i64,
    is_gate: bool,
    created_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    tags: Vec<String>,
    deps: Vec<String>,
}

/// The merged, final form of one task, ready to write to the output db.
#[derive(Debug, Clone)]
struct MergedTask {
    uuid: String,
    id: i64,
    title: String,
    details: Option<String>,
    status: String,
    priority: i64,
    is_gate: bool,
    created_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    tags: HashSet<String>,
    deps: HashSet<String>,
    conflict: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictNote {
    pub task_id: i64,
    pub field: String,
    pub ours: String,
    pub theirs: String,
    pub resolution: String,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct MergeReport {
    pub tasks_total: usize,
    pub carried_unchanged: usize,
    pub auto_resolved: usize,
    pub conflicts: Vec<ConflictNote>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MergeOptions {
    pub strict: bool,
}

/// Run the merge purely in memory and write the result into `out_path`
/// (a fresh file; must not already exist as a non-empty db — callers
/// arrange atomic replacement via a temp path + rename).
///
/// Identity across base/ours/theirs is the `uuid` — a uuid can't collide, so
/// unlike the pre-uuid design there is no renumbering to do here. Two
/// distinct tasks legitimately ending up with the same display `id` after
/// this merge is expected; it surfaces the next time someone runs
/// `show <that id>`, via `db::resolve_one`'s ambiguity listing, not as a
/// merge-time problem.
pub fn merge_databases(
    base: Option<&Connection>,
    ours: &Connection,
    theirs: &Connection,
    out_path: &Path,
    opts: MergeOptions,
) -> CliResult<MergeReport> {
    let base_tasks = base.map(load_all).transpose()?.unwrap_or_default();
    let ours_tasks = load_all(ours)?;
    let theirs_tasks = load_all(theirs)?;

    let base_uuids: HashSet<String> = base_tasks.iter().map(|t| t.uuid.clone()).collect();
    let ours_by_uuid: HashMap<&str, &RawTask> =
        ours_tasks.iter().map(|t| (t.uuid.as_str(), t)).collect();
    let theirs_by_uuid: HashMap<&str, &RawTask> =
        theirs_tasks.iter().map(|t| (t.uuid.as_str(), t)).collect();
    let base_by_uuid: HashMap<&str, &RawTask> =
        base_tasks.iter().map(|t| (t.uuid.as_str(), t)).collect();

    let mut report = MergeReport::default();
    let mut merged: Vec<MergedTask> = Vec::new();

    // --- 1. Common tasks: uuids known to base. ---
    for uuid in &base_uuids {
        let base_t = base_by_uuid[uuid.as_str()];
        let ours_t = ours_by_uuid.get(uuid.as_str()).copied();
        let theirs_t = theirs_by_uuid.get(uuid.as_str()).copied();
        match (ours_t, theirs_t) {
            (None, None) => { /* deleted both sides */ }
            (None, Some(t)) => {
                if fields_equal(base_t, t) {
                    // deleted in ours, unchanged in theirs -> respect deletion
                } else {
                    report.conflicts.push(ConflictNote {
                        task_id: t.id,
                        field: "existence".into(),
                        ours: "deleted".into(),
                        theirs: "modified".into(),
                        resolution: "kept theirs (modified) over ours' deletion".into(),
                    });
                    merged.push(raw_to_merged(t, true));
                }
            }
            (Some(t), None) => {
                if fields_equal(base_t, t) {
                    // deleted in theirs, unchanged in ours -> respect deletion
                } else {
                    report.conflicts.push(ConflictNote {
                        task_id: t.id,
                        field: "existence".into(),
                        ours: "modified".into(),
                        theirs: "deleted".into(),
                        resolution: "kept ours (modified) over theirs' deletion".into(),
                    });
                    merged.push(raw_to_merged(t, true));
                }
            }
            (Some(o), Some(t)) => {
                merged.push(merge_common(base_t, o, t, &mut report));
            }
        }
        report.tasks_total += 1;
    }

    // --- 1b. Tasks present on both sides but unknown to base — most often
    // because there IS no base (a 2-way union merge). A shared uuid always
    // means "the same task": unlike the old id-based design, this can't be
    // coincidental, so it must still be reconciled into one row rather than
    // unioned in twice (which would violate the uuid primary key). With no
    // base to tell which side changed, an actual field difference can't be
    // auto-resolved the way `merge_common` does — it's reported as a
    // conflict and `ours` wins, same convention as every other conflict. ---
    for uuid in ours_by_uuid.keys() {
        if base_uuids.contains(*uuid) {
            continue; // already handled in step 1
        }
        if let Some(theirs_t) = theirs_by_uuid.get(uuid) {
            merged.push(merge_common_no_base(
                ours_by_uuid[uuid],
                theirs_t,
                &mut report,
            ));
            report.tasks_total += 1;
        }
    }

    // --- 2. New tasks: uuids known to only one side. Carry straight through
    // with their own id/uuid — a uuid can't collide, and a shared display id
    // is fine (see the doc comment above). ---
    for t in &ours_tasks {
        if !base_uuids.contains(&t.uuid) && !theirs_by_uuid.contains_key(t.uuid.as_str()) {
            merged.push(raw_to_merged(t, false));
            report.tasks_total += 1;
            report.carried_unchanged += 1;
        }
    }
    for t in &theirs_tasks {
        if !base_uuids.contains(&t.uuid) && !ours_by_uuid.contains_key(t.uuid.as_str()) {
            merged.push(raw_to_merged(t, false));
            report.tasks_total += 1;
            report.carried_unchanged += 1;
        }
    }

    // Drop self-loops, edges to tasks that don't exist in the merged set,
    // and any edge that would introduce a cycle.
    let alive: HashSet<String> = merged.iter().map(|t| t.uuid.clone()).collect();
    let mut adjacency: HashMap<String, HashSet<String>> = HashMap::new();
    for t in merged.iter_mut() {
        let candidates: Vec<String> = t
            .deps
            .iter()
            .filter(|d| **d != t.uuid && alive.contains(*d))
            .cloned()
            .collect();
        let mut kept = HashSet::new();
        for dep in candidates {
            if would_cycle(&adjacency, &t.uuid, &dep) {
                continue;
            }
            adjacency
                .entry(t.uuid.clone())
                .or_default()
                .insert(dep.clone());
            kept.insert(dep);
        }
        t.deps = kept;
    }

    write_output(&merged, out_path, opts)?;
    Ok(report)
}

fn would_cycle(
    adjacency: &HashMap<String, HashSet<String>>,
    task_uuid: &str,
    new_dep: &str,
) -> bool {
    // task_uuid -> new_dep creates a cycle iff new_dep already (transitively)
    // depends on task_uuid.
    let mut stack = vec![new_dep.to_string()];
    let mut seen = HashSet::new();
    while let Some(node) = stack.pop() {
        if node == task_uuid {
            return true;
        }
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(next) = adjacency.get(&node) {
            stack.extend(next.iter().cloned());
        }
    }
    false
}

fn fields_equal(a: &RawTask, b: &RawTask) -> bool {
    a.title == b.title
        && a.details == b.details
        && a.status == b.status
        && a.priority == b.priority
        && a.is_gate == b.is_gate
}

fn raw_to_merged(t: &RawTask, conflict: bool) -> MergedTask {
    MergedTask {
        uuid: t.uuid.clone(),
        id: t.id,
        title: t.title.clone(),
        details: t.details.clone(),
        status: t.status.clone(),
        priority: t.priority,
        is_gate: t.is_gate,
        created_at: t.created_at.clone(),
        started_at: t.started_at.clone(),
        completed_at: t.completed_at.clone(),
        tags: t.tags.iter().cloned().collect(),
        deps: t.deps.iter().cloned().collect(),
        conflict,
    }
}

fn status_rank(s: &str) -> i32 {
    match s {
        "pending" => 0,
        "partial" | "in-progress" => 1,
        "done" | "rejected" => 2,
        _ => 0,
    }
}

fn merge_common(
    base: &RawTask,
    ours: &RawTask,
    theirs: &RawTask,
    report: &mut MergeReport,
) -> MergedTask {
    let mut conflict = false;

    // title
    let title = if ours.title == theirs.title {
        ours.title.clone()
    } else if ours.title == base.title {
        theirs.title.clone()
    } else if theirs.title == base.title {
        ours.title.clone()
    } else {
        conflict = true;
        report.conflicts.push(ConflictNote {
            task_id: base.id,
            field: "title".into(),
            ours: ours.title.clone(),
            theirs: theirs.title.clone(),
            resolution: "kept ours".into(),
        });
        ours.title.clone()
    };

    // details: delta-concatenate when both changed differently
    let base_details = base.details.clone().unwrap_or_default();
    let details = if ours.details == theirs.details {
        ours.details.clone()
    } else if ours.details.as_deref().unwrap_or("") == base_details {
        theirs.details.clone()
    } else if theirs.details.as_deref().unwrap_or("") == base_details {
        ours.details.clone()
    } else {
        report.auto_resolved += 1;
        let ours_delta = delta(&base_details, ours.details.as_deref().unwrap_or(""));
        let theirs_delta = delta(&base_details, theirs.details.as_deref().unwrap_or(""));
        let mut combined = base_details.clone();
        if !ours_delta.is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(ours_delta);
        }
        if !theirs_delta.is_empty() && theirs_delta != ours_delta {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(theirs_delta);
        }
        if combined.is_empty() {
            None
        } else {
            Some(combined)
        }
    };

    // status (+ started_at/completed_at follow the chosen side)
    let (status, started_at, completed_at) = if ours.status == theirs.status {
        (
            ours.status.clone(),
            ours.started_at.clone(),
            ours.completed_at.clone(),
        )
    } else if ours.status == base.status {
        (
            theirs.status.clone(),
            theirs.started_at.clone(),
            theirs.completed_at.clone(),
        )
    } else if theirs.status == base.status {
        (
            ours.status.clone(),
            ours.started_at.clone(),
            ours.completed_at.clone(),
        )
    } else {
        report.auto_resolved += 1;
        let ours_rank = status_rank(&ours.status);
        let theirs_rank = status_rank(&theirs.status);
        let take_ours = if ours_rank != theirs_rank {
            ours_rank > theirs_rank
        } else {
            // equal-rank tie-break: in-progress > partial, done > rejected
            matches!(ours.status.as_str(), "in-progress" | "done")
        };
        if take_ours {
            (
                ours.status.clone(),
                ours.started_at.clone(),
                ours.completed_at.clone(),
            )
        } else {
            (
                theirs.status.clone(),
                theirs.started_at.clone(),
                theirs.completed_at.clone(),
            )
        }
    };
    let completed_at = match (ours.status.as_str(), theirs.status.as_str()) {
        ("done", "done") => earlier(ours.completed_at.as_deref(), theirs.completed_at.as_deref()),
        _ => completed_at,
    };

    // priority: lower number = more urgent, prefer it on a real conflict
    let priority = if ours.priority == theirs.priority {
        ours.priority
    } else if ours.priority == base.priority {
        theirs.priority
    } else if theirs.priority == base.priority {
        ours.priority
    } else {
        report.auto_resolved += 1;
        ours.priority.min(theirs.priority)
    };

    // is_gate
    let is_gate = if ours.is_gate == theirs.is_gate {
        ours.is_gate
    } else if ours.is_gate == base.is_gate {
        theirs.is_gate
    } else if theirs.is_gate == base.is_gate {
        ours.is_gate
    } else {
        conflict = true;
        report.conflicts.push(ConflictNote {
            task_id: base.id,
            field: "is_gate".into(),
            ours: ours.is_gate.to_string(),
            theirs: theirs.is_gate.to_string(),
            resolution: "kept ours".into(),
        });
        ours.is_gate
    };

    let tags: HashSet<String> = ours
        .tags
        .iter()
        .chain(theirs.tags.iter())
        .cloned()
        .collect();
    let deps: HashSet<String> = ours
        .deps
        .iter()
        .chain(theirs.deps.iter())
        .cloned()
        .collect();

    MergedTask {
        uuid: base.uuid.clone(),
        // The display id of a common task is never a field either side
        // edits — it just carries through unchanged from base.
        id: base.id,
        title,
        details,
        status,
        priority,
        is_gate,
        created_at: base.created_at.clone(),
        started_at,
        completed_at,
        tags,
        deps,
        conflict,
    }
}

/// Reconcile a task both sides have (same uuid) but that base doesn't know
/// about — no common ancestor to run a real 3-way diff against. A field
/// that agrees carries through unchanged; a field that disagrees can't be
/// attributed to either side, so it's a conflict: keep ours, tag it, and
/// note it in the report, same convention as `merge_common`'s conflicts.
fn merge_common_no_base(ours: &RawTask, theirs: &RawTask, report: &mut MergeReport) -> MergedTask {
    let mut conflict = false;

    macro_rules! field {
        ($name:literal, $f:ident) => {
            if ours.$f == theirs.$f {
                ours.$f.clone()
            } else {
                conflict = true;
                report.conflicts.push(ConflictNote {
                    task_id: ours.id,
                    field: $name.into(),
                    ours: format!("{:?}", ours.$f),
                    theirs: format!("{:?}", theirs.$f),
                    resolution: "kept ours (no common ancestor to reconcile against)".into(),
                });
                ours.$f.clone()
            }
        };
    }

    let title = field!("title", title);
    let details = field!("details", details);
    let status = field!("status", status);
    let priority = field!("priority", priority);
    let is_gate = field!("is_gate", is_gate);

    let (started_at, completed_at) = if status == ours.status {
        (ours.started_at.clone(), ours.completed_at.clone())
    } else {
        (theirs.started_at.clone(), theirs.completed_at.clone())
    };

    let tags: HashSet<String> = ours
        .tags
        .iter()
        .chain(theirs.tags.iter())
        .cloned()
        .collect();
    let deps: HashSet<String> = ours
        .deps
        .iter()
        .chain(theirs.deps.iter())
        .cloned()
        .collect();

    MergedTask {
        uuid: ours.uuid.clone(),
        id: ours.id,
        title,
        details,
        status,
        priority,
        is_gate,
        created_at: ours.created_at.clone(),
        started_at,
        completed_at,
        tags,
        deps,
        conflict,
    }
}

fn earlier(a: Option<&str>, b: Option<&str>) -> Option<String> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if x <= y { x } else { y }.to_string()),
        (Some(x), None) => Some(x.to_string()),
        (None, Some(y)) => Some(y.to_string()),
        (None, None) => None,
    }
}

/// The suffix of `changed` beyond the shared prefix with `base` — i.e. what
/// was appended, assuming append-only growth (mirrors `edit --append-details`).
/// Falls back to the whole `changed` string when it isn't a simple extension
/// of `base` (e.g. base text was edited in the middle, not just appended to).
fn delta<'a>(base: &str, changed: &'a str) -> &'a str {
    if let Some(rest) = changed.strip_prefix(base) {
        rest.trim_start_matches('\n')
    } else {
        changed
    }
}

fn load_all(conn: &Connection) -> CliResult<Vec<RawTask>> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM tasks ORDER BY uuid",
            db::TASK_COLUMNS
        ))
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RawTask {
                id: r.get(0)?,
                uuid: r.get(1)?,
                title: r.get(2)?,
                details: r.get(3)?,
                status: r.get(4)?,
                priority: r.get(5)?,
                is_gate: r.get(6)?,
                created_at: r.get(7)?,
                started_at: r.get(8)?,
                completed_at: r.get(9)?,
                tags: Vec::new(),
                deps: Vec::new(),
            })
        })
        .map_err(|e| system(format!("query failed: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| system(format!("row failed: {e}")))?);
    }
    for t in out.iter_mut() {
        t.tags = db::load_tags(conn, &t.uuid)?;
        t.deps = db::load_dep_uuids(conn, &t.uuid)?;
    }
    Ok(out)
}

fn write_output(merged: &[MergedTask], out_path: &Path, opts: MergeOptions) -> CliResult<()> {
    if opts.strict && merged.iter().any(|t| t.conflict) {
        return Ok(()); // caller checks report.conflicts and skips the write in strict mode
    }
    if out_path.exists() {
        std::fs::remove_file(out_path)
            .map_err(|e| system(format!("cannot remove stale {}: {e}", out_path.display())))?;
    }
    let mut conn = db::open(out_path)?;
    db::create_schema(&conn)?;

    let tx = conn
        .transaction()
        .map_err(|e| system(format!("begin tx failed: {e}")))?;
    // Insert every task before any tags/deps — a dep can point at a task
    // that sorts later in `merged`, and depends_on_uuid is a foreign key.
    for t in merged {
        tx.execute(
            "INSERT INTO tasks(uuid, id, title, details, status, priority, is_gate, created_at, started_at, completed_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                t.uuid,
                t.id,
                t.title,
                t.details,
                t.status,
                t.priority,
                t.is_gate as i64,
                t.created_at,
                t.started_at,
                t.completed_at,
            ],
        )
        .map_err(|e| system(format!("task insert failed: {e}")))?;
    }
    for t in merged {
        for tag in &t.tags {
            tx.execute(
                "INSERT OR IGNORE INTO tags(task_uuid, tag) VALUES(?1, ?2)",
                params![t.uuid, tag],
            )
            .map_err(|e| system(format!("tag insert failed: {e}")))?;
        }
        if t.conflict {
            tx.execute(
                "INSERT OR IGNORE INTO tags(task_uuid, tag) VALUES(?1, ?2)",
                params![t.uuid, CONFLICT_TAG],
            )
            .map_err(|e| system(format!("tag insert failed: {e}")))?;
        }
        for dep in &t.deps {
            tx.execute(
                "INSERT OR IGNORE INTO deps(task_uuid, depends_on_uuid) VALUES(?1, ?2)",
                params![t.uuid, dep],
            )
            .map_err(|e| system(format!("dep insert failed: {e}")))?;
        }
    }
    tx.commit()
        .map_err(|e| system(format!("commit failed: {e}")))?;
    Ok(())
}

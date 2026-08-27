use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::db;
use crate::error::{system, CliResult};

const CONFLICT_TAG: &str = "merge-conflict";

/// A task row plus its tags/deps, loaded independently of any resolved id
/// mapping — exactly what's on disk in one of the three input databases.
#[derive(Debug, Clone)]
struct RawTask {
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
    deps: Vec<i64>,
}

/// The merged, final form of one task, ready to write to the output db.
#[derive(Debug, Clone)]
struct MergedTask {
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
    deps: HashSet<i64>,
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
    pub renumbered: usize,
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

    let base_ids: HashSet<i64> = base_tasks.iter().map(|t| t.id).collect();
    let ours_by_id: HashMap<i64, &RawTask> = ours_tasks.iter().map(|t| (t.id, t)).collect();
    let theirs_by_id: HashMap<i64, &RawTask> = theirs_tasks.iter().map(|t| (t.id, t)).collect();
    let base_by_id: HashMap<i64, &RawTask> = base_tasks.iter().map(|t| (t.id, t)).collect();

    let mut report = MergeReport::default();
    let mut merged: Vec<MergedTask> = Vec::new();

    // Highest id already claimed by any task we plan to keep under its
    // current id (all base ids, all ours ids). New theirs-only ids that
    // collide get renumbered above this (and above each other).
    let mut max_id = base_ids
        .iter()
        .chain(ours_by_id.keys())
        .copied()
        .max()
        .unwrap_or(0);

    // --- 1. Common tasks: ids known to base. ---
    for id in &base_ids {
        let base_t = base_by_id[id];
        let ours_t = ours_by_id.get(id).copied();
        let theirs_t = theirs_by_id.get(id).copied();
        match (ours_t, theirs_t) {
            (None, None) => { /* deleted both sides */ }
            (None, Some(t)) => {
                if fields_equal(base_t, t) {
                    // deleted in ours, unchanged in theirs -> respect deletion
                } else {
                    report.conflicts.push(ConflictNote {
                        task_id: *id,
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
                        task_id: *id,
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

    // --- 2. New tasks: ids not known to base. ---
    let mut id_map: HashMap<i64, i64> = HashMap::new();
    let mut used_ids: HashSet<i64> = merged.iter().map(|t| t.id).collect();
    used_ids.extend(ours_by_id.keys().filter(|id| !base_ids.contains(*id)));

    // ours' new tasks keep their ids verbatim.
    for t in &ours_tasks {
        if !base_ids.contains(&t.id) {
            merged.push(raw_to_merged(t, false));
            report.tasks_total += 1;
            report.carried_unchanged += 1;
        }
    }

    // theirs' new tasks: renumber on any collision, in created_at order for
    // determinism.
    let mut theirs_new: Vec<&RawTask> = theirs_tasks
        .iter()
        .filter(|t| !base_ids.contains(&t.id))
        .collect();
    theirs_new.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));

    for t in theirs_new {
        let final_id = if used_ids.contains(&t.id) {
            max_id += 1;
            report.renumbered += 1;
            max_id
        } else {
            t.id
        };
        id_map.insert(t.id, final_id);
        used_ids.insert(final_id);
        max_id = max_id.max(final_id);

        let mut mt = raw_to_merged(t, false);
        mt.id = final_id;
        mt.deps = mt.deps.into_iter().map(|d| remap(&id_map, d)).collect();
        merged.push(mt);
        report.tasks_total += 1;
        report.carried_unchanged += 1;
    }

    // Apply the id_map to every merged task's deps now that it's complete
    // (a common task may hold an edge onto a theirs-only new task).
    for t in merged.iter_mut() {
        t.deps = t.deps.iter().map(|d| remap(&id_map, *d)).collect();
    }

    // Drop self-loops, edges to tasks that don't exist in the merged set,
    // and any edge that would introduce a cycle.
    let alive: HashSet<i64> = merged.iter().map(|t| t.id).collect();
    let mut adjacency: HashMap<i64, HashSet<i64>> = HashMap::new();
    for t in merged.iter_mut() {
        let candidates: Vec<i64> = t
            .deps
            .iter()
            .copied()
            .filter(|d| *d != t.id && alive.contains(d))
            .collect();
        let mut kept = HashSet::new();
        for dep in candidates {
            if would_cycle(&adjacency, t.id, dep) {
                continue;
            }
            adjacency.entry(t.id).or_default().insert(dep);
            kept.insert(dep);
        }
        t.deps = kept;
    }

    write_output(&merged, out_path, opts)?;
    Ok(report)
}

fn remap(id_map: &HashMap<i64, i64>, id: i64) -> i64 {
    *id_map.get(&id).unwrap_or(&id)
}

fn would_cycle(adjacency: &HashMap<i64, HashSet<i64>>, task_id: i64, new_dep: i64) -> bool {
    // task_id -> new_dep creates a cycle iff new_dep already (transitively)
    // depends on task_id.
    let mut stack = vec![new_dep];
    let mut seen = HashSet::new();
    while let Some(node) = stack.pop() {
        if node == task_id {
            return true;
        }
        if !seen.insert(node) {
            continue;
        }
        if let Some(next) = adjacency.get(&node) {
            stack.extend(next.iter().copied());
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
        deps: t.deps.iter().copied().collect(),
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
    let deps: HashSet<i64> = ours
        .deps
        .iter()
        .chain(theirs.deps.iter())
        .copied()
        .collect();

    MergedTask {
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
        .prepare(
            "SELECT id, title, details, status, priority, is_gate, created_at, started_at, completed_at
             FROM tasks ORDER BY id",
        )
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RawTask {
                id: r.get(0)?,
                title: r.get(1)?,
                details: r.get(2)?,
                status: r.get(3)?,
                priority: r.get(4)?,
                is_gate: r.get(5)?,
                created_at: r.get(6)?,
                started_at: r.get(7)?,
                completed_at: r.get(8)?,
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
        t.tags = db::load_tags(conn, t.id)?;
        t.deps = db::load_deps(conn, t.id)?;
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
    let mut max_id = 0i64;
    // Insert every task before any tags/deps — a dep can point at a task
    // that sorts later in `merged`, and depends_on_id is a foreign key.
    for t in merged {
        max_id = max_id.max(t.id);
        tx.execute(
            "INSERT INTO tasks(id, title, details, status, priority, is_gate, created_at, started_at, completed_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
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
                "INSERT OR IGNORE INTO tags(task_id, tag) VALUES(?1, ?2)",
                params![t.id, tag],
            )
            .map_err(|e| system(format!("tag insert failed: {e}")))?;
        }
        if t.conflict {
            tx.execute(
                "INSERT OR IGNORE INTO tags(task_id, tag) VALUES(?1, ?2)",
                params![t.id, CONFLICT_TAG],
            )
            .map_err(|e| system(format!("tag insert failed: {e}")))?;
        }
        for dep in &t.deps {
            tx.execute(
                "INSERT OR IGNORE INTO deps(task_id, depends_on_id) VALUES(?1, ?2)",
                params![t.id, dep],
            )
            .map_err(|e| system(format!("dep insert failed: {e}")))?;
        }
    }
    tx.commit()
        .map_err(|e| system(format!("commit failed: {e}")))?;

    if max_id > 0 {
        conn.execute("DELETE FROM sqlite_sequence WHERE name = 'tasks'", [])
            .map_err(|e| system(format!("clear sqlite_sequence failed: {e}")))?;
        conn.execute(
            "INSERT INTO sqlite_sequence(name, seq) VALUES('tasks', ?1)",
            params![max_id],
        )
        .map_err(|e| system(format!("restore sqlite_sequence failed: {e}")))?;
    }
    Ok(())
}

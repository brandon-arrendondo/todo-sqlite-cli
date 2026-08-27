use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;
use serde_json::json;

use crate::db;
use crate::error::{system, user, CliResult};

/// Post-merge (and generally post-hoc) sanity checks. Deliberately narrow —
/// an earlier draft of this also flagged "deps whose two tasks share no
/// tag" and "a pending task blocked by a done task"; both turned out to be
/// overwhelmingly legitimate noise (tags are a bad relatedness proxy, and a
/// satisfied dep on a done task is normal) and were dropped rather than
/// kept. What's left are conditions that are never expected in a healthy
/// database, plus the one condition — a duplicate display id — that's
/// expected occasionally after a merge and just needs surfacing so an
/// operator knows `show <id>` may need a uuid to disambiguate.
#[derive(Debug, Default, Serialize)]
struct DoctorReport {
    duplicate_display_ids: Vec<DuplicateId>,
    unresolved_merge_conflicts: Vec<TaskRef>,
    orphaned_tag_rows: i64,
    orphaned_dep_rows: i64,
    self_deps: i64,
    dependency_cycles: Vec<TaskRef>,
}

#[derive(Debug, Serialize)]
struct DuplicateId {
    id: i64,
    tasks: Vec<TaskRef>,
}

#[derive(Debug, Serialize, Clone)]
struct TaskRef {
    id: i64,
    uuid: String,
    title: String,
}

impl DoctorReport {
    fn is_clean(&self) -> bool {
        self.duplicate_display_ids.is_empty()
            && self.unresolved_merge_conflicts.is_empty()
            && self.orphaned_tag_rows == 0
            && self.orphaned_dep_rows == 0
            && self.self_deps == 0
            && self.dependency_cycles.is_empty()
    }
}

pub fn run(db_path: &Path, json: bool) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let mut report = DoctorReport::default();

    // Duplicate display ids: expected occasionally after a merge (identity
    // is the uuid, not this), but worth surfacing so an operator knows
    // `show <id>` may come back ambiguous until they pick a uuid.
    {
        let mut stmt = conn
            .prepare("SELECT id FROM tasks GROUP BY id HAVING COUNT(*) > 1 ORDER BY id")
            .map_err(|e| system(format!("prepare failed: {e}")))?;
        let ids: Vec<i64> = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| system(format!("query failed: {e}")))?
            .collect::<Result<_, _>>()
            .map_err(|e| system(format!("row read failed: {e}")))?;
        for id in ids {
            let tasks = task_refs_by_id(&conn, id)?;
            report.duplicate_display_ids.push(DuplicateId { id, tasks });
        }
    }

    // Tasks still tagged from an unresolved merge conflict.
    {
        let mut stmt = conn
            .prepare(
                "SELECT t.id, t.uuid, t.title FROM tasks t
                 JOIN tags g ON g.task_uuid = t.uuid
                 WHERE g.tag = 'merge-conflict' ORDER BY t.id",
            )
            .map_err(|e| system(format!("prepare failed: {e}")))?;
        report.unresolved_merge_conflicts = stmt
            .query_map([], |r| {
                Ok(TaskRef {
                    id: r.get(0)?,
                    uuid: r.get(1)?,
                    title: r.get(2)?,
                })
            })
            .map_err(|e| system(format!("query failed: {e}")))?
            .collect::<Result<_, _>>()
            .map_err(|e| system(format!("row read failed: {e}")))?;
    }

    // Rows that shouldn't exist under normal operation (foreign_keys=ON and
    // ON DELETE CASCADE should prevent these) — a defensive check in case
    // a database was hand-edited or touched by other tooling.
    report.orphaned_tag_rows = conn
        .query_row(
            "SELECT COUNT(*) FROM tags WHERE task_uuid NOT IN (SELECT uuid FROM tasks)",
            [],
            |r| r.get(0),
        )
        .map_err(|e| system(format!("query failed: {e}")))?;
    report.orphaned_dep_rows = conn
        .query_row(
            "SELECT COUNT(*) FROM deps
             WHERE task_uuid NOT IN (SELECT uuid FROM tasks)
                OR depends_on_uuid NOT IN (SELECT uuid FROM tasks)",
            [],
            |r| r.get(0),
        )
        .map_err(|e| system(format!("query failed: {e}")))?;
    report.self_deps = conn
        .query_row(
            "SELECT COUNT(*) FROM deps WHERE task_uuid = depends_on_uuid",
            [],
            |r| r.get(0),
        )
        .map_err(|e| system(format!("query failed: {e}")))?;

    // Dependency cycles: `add`/`edit` reject any edge that would create one,
    // and merge drops cycle-forming edges — this should never fire, but a
    // hand-edited or externally-written db could still have one.
    let cycle_uuids = find_cycle_participants(&conn)?;
    if !cycle_uuids.is_empty() {
        let mut refs: Vec<TaskRef> = Vec::new();
        for u in &cycle_uuids {
            refs.push(task_ref_by_uuid(&conn, u)?);
        }
        refs.sort_by_key(|t| t.id);
        report.dependency_cycles = refs;
    }

    print_report(json, &report);

    if report.is_clean() {
        Ok(())
    } else {
        Err(user(
            "doctor found issues — see above (exit 1 so this can gate a script)",
        ))
    }
}

fn task_refs_by_id(conn: &rusqlite::Connection, id: i64) -> CliResult<Vec<TaskRef>> {
    let mut stmt = conn
        .prepare("SELECT id, uuid, title FROM tasks WHERE id = ?1 ORDER BY uuid")
        .map_err(|e| system(format!("prepare failed: {e}")))?;
    let rows = stmt
        .query_map(rusqlite::params![id], |r| {
            Ok(TaskRef {
                id: r.get(0)?,
                uuid: r.get(1)?,
                title: r.get(2)?,
            })
        })
        .map_err(|e| system(format!("query failed: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| system(format!("row read failed: {e}")))?);
    }
    Ok(out)
}

fn task_ref_by_uuid(conn: &rusqlite::Connection, uuid: &str) -> CliResult<TaskRef> {
    conn.query_row(
        "SELECT id, uuid, title FROM tasks WHERE uuid = ?1",
        rusqlite::params![uuid],
        |r| {
            Ok(TaskRef {
                id: r.get(0)?,
                uuid: r.get(1)?,
                title: r.get(2)?,
            })
        },
    )
    .map_err(|e| system(format!("query failed: {e}")))
}

/// Every task uuid that participates in at least one dependency cycle,
/// found via a standard three-color DFS over the `deps` graph.
fn find_cycle_participants(conn: &rusqlite::Connection) -> CliResult<HashSet<String>> {
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT task_uuid, depends_on_uuid FROM deps")
            .map_err(|e| system(format!("prepare failed: {e}")))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| system(format!("query failed: {e}")))?;
        for r in rows {
            let (from, to) = r.map_err(|e| system(format!("row read failed: {e}")))?;
            edges.entry(from).or_default().push(to);
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum State {
        InStack,
        Done,
    }

    let mut state: HashMap<String, State> = HashMap::new();
    let mut participants: HashSet<String> = HashSet::new();
    let nodes: Vec<String> = edges.keys().cloned().collect();

    for start in nodes {
        if state.get(&start) == Some(&State::Done) {
            continue;
        }
        let mut stack: Vec<String> = Vec::new();
        let mut call_stack: Vec<(String, usize)> = vec![(start, 0)];
        while let Some((node, idx)) = call_stack.pop() {
            if idx == 0 {
                state.insert(node.clone(), State::InStack);
                stack.push(node.clone());
            }
            let next_edges = edges.get(&node).cloned().unwrap_or_default();
            if idx < next_edges.len() {
                let neighbor = next_edges[idx].clone();
                call_stack.push((node.clone(), idx + 1));
                match state.get(&neighbor) {
                    Some(State::InStack) => {
                        if let Some(pos) = stack.iter().position(|x| x == &neighbor) {
                            for p in &stack[pos..] {
                                participants.insert(p.clone());
                            }
                        }
                    }
                    Some(State::Done) => {}
                    None => call_stack.push((neighbor, 0)),
                }
            } else {
                state.insert(node.clone(), State::Done);
                stack.pop();
            }
        }
    }

    Ok(participants)
}

fn print_report(json: bool, report: &DoctorReport) {
    if json {
        let v = json!({
            "clean": report.is_clean(),
            "duplicate_display_ids": report.duplicate_display_ids,
            "unresolved_merge_conflicts": report.unresolved_merge_conflicts,
            "orphaned_tag_rows": report.orphaned_tag_rows,
            "orphaned_dep_rows": report.orphaned_dep_rows,
            "self_deps": report.self_deps,
            "dependency_cycles": report.dependency_cycles,
        });
        println!("{}", serde_json::to_string(&v).unwrap());
        return;
    }

    if report.is_clean() {
        println!("doctor: clean");
        return;
    }

    if !report.duplicate_display_ids.is_empty() {
        println!(
            "duplicate display ids ({}) — show/edit by uuid to disambiguate:",
            report.duplicate_display_ids.len()
        );
        for d in &report.duplicate_display_ids {
            for t in &d.tasks {
                println!("  id={} uuid={} title={}", t.id, t.uuid, t.title);
            }
        }
    }
    if !report.unresolved_merge_conflicts.is_empty() {
        println!(
            "unresolved merge conflicts ({}) — see `list --tag merge-conflict`:",
            report.unresolved_merge_conflicts.len()
        );
        for t in &report.unresolved_merge_conflicts {
            println!("  id={} uuid={} title={}", t.id, t.uuid, t.title);
        }
    }
    if report.orphaned_tag_rows > 0 {
        println!(
            "orphaned tag rows (task_uuid with no matching task): {}",
            report.orphaned_tag_rows
        );
    }
    if report.orphaned_dep_rows > 0 {
        println!(
            "orphaned dep rows (task_uuid/depends_on_uuid with no matching task): {}",
            report.orphaned_dep_rows
        );
    }
    if report.self_deps > 0 {
        println!(
            "self-dependencies (task_uuid = depends_on_uuid): {}",
            report.self_deps
        );
    }
    if !report.dependency_cycles.is_empty() {
        println!(
            "tasks participating in a dependency cycle ({}):",
            report.dependency_cycles.len()
        );
        for t in &report.dependency_cycles {
            println!("  id={} uuid={} title={}", t.id, t.uuid, t.title);
        }
    }
}

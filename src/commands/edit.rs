use std::path::Path;

use rusqlite::params;

use crate::db;
use crate::error::{system, user, CliResult};
use crate::format;

#[allow(clippy::too_many_arguments)]
pub fn run(
    db_path: &Path,
    json: bool,
    id: &str,
    title: Option<&str>,
    details: Option<&str>,
    append_details: Option<&str>,
    clear_details: bool,
    priority: Option<i64>,
    add_tag: &[String],
    rm_tag: &[String],
    add_dep: &[String],
    rm_dep: &[String],
    gate: bool,
    no_gate: bool,
    location: Option<&str>,
    clear_location: bool,
    add_related: &[String],
    rm_related: &[String],
) -> CliResult<()> {
    let mut conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }
    let target = db::resolve_one(&conn, id)?;
    let uuid = target.uuid.clone();
    let display_id = target.id;

    // Dependency args are resolved to specific uuids up front (each may be
    // ambiguous itself) — the transaction below only ever touches `deps` by
    // uuid.
    let add_dep_uuids = resolve_deps(&conn, &uuid, add_dep)?;
    let rm_dep_uuids = resolve_deps(&conn, &uuid, rm_dep)?;
    let add_related_uuids = resolve_related(&conn, &uuid, add_related)?;
    let rm_related_uuids = resolve_related(&conn, &uuid, rm_related)?;

    let tx = conn
        .transaction()
        .map_err(|e| system(format!("begin tx failed: {e}")))?;

    apply_title(&tx, &uuid, title)?;
    apply_details(&tx, &uuid, details, append_details, clear_details)?;
    apply_priority(&tx, &uuid, priority)?;
    apply_gate(&tx, &uuid, gate, no_gate)?;
    apply_tags(&tx, &uuid, add_tag, rm_tag)?;
    apply_deps(&tx, &uuid, &add_dep_uuids, &rm_dep_uuids)?;
    apply_location(&tx, &uuid, location, clear_location)?;
    apply_related(&tx, &uuid, &add_related_uuids, &rm_related_uuids)?;

    tx.commit()
        .map_err(|e| system(format!("commit failed: {e}")))?;

    let t = db::load_task_by_uuid(&conn, &uuid)?;
    if json {
        format::print_task_json(&t);
    } else {
        println!("edited {display_id}");
    }
    Ok(())
}

/// Resolve each `--add-dep`/`--rm-dep` argument to a specific task uuid,
/// rejecting self-deps up front by display id (a quick, friendly check —
/// the real guard is `uuid == self_uuid` once resolved).
fn resolve_deps(
    conn: &rusqlite::Connection,
    self_uuid: &str,
    raw: &[String],
) -> CliResult<Vec<String>> {
    let mut out = Vec::new();
    for r in raw {
        let t = db::resolve_one(conn, r).map_err(|e| user(format!("dependency {r}: {e}")))?;
        if t.uuid == self_uuid {
            return Err(user("a task cannot depend on itself"));
        }
        out.push(t.uuid);
    }
    Ok(out)
}

/// Resolve each `--add-related`/`--rm-related` argument to a specific task
/// uuid, rejecting self-links up front (mirrors `resolve_deps`).
fn resolve_related(
    conn: &rusqlite::Connection,
    self_uuid: &str,
    raw: &[String],
) -> CliResult<Vec<String>> {
    let mut out = Vec::new();
    for r in raw {
        let t = db::resolve_one(conn, r).map_err(|e| user(format!("related task {r}: {e}")))?;
        if t.uuid == self_uuid {
            return Err(user("a task cannot be related to itself"));
        }
        out.push(t.uuid);
    }
    Ok(out)
}

fn apply_title(tx: &rusqlite::Transaction, uuid: &str, title: Option<&str>) -> CliResult<()> {
    if let Some(t) = title {
        if t.trim().is_empty() {
            return Err(user("title must not be empty"));
        }
        tx.execute(
            "UPDATE tasks SET title = ?1 WHERE uuid = ?2",
            params![t, uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }
    Ok(())
}

/// Apply the mutually-exclusive details edits. `--details`, `--append-details`,
/// and `--clear-details` may not be combined.
fn apply_details(
    tx: &rusqlite::Transaction,
    uuid: &str,
    details: Option<&str>,
    append_details: Option<&str>,
    clear_details: bool,
) -> CliResult<()> {
    let details_mutex =
        details.is_some() as u8 + append_details.is_some() as u8 + clear_details as u8;
    if details_mutex > 1 {
        return Err(user(
            "--details, --append-details, and --clear-details are mutually exclusive",
        ));
    }
    if let Some(d) = details {
        tx.execute(
            "UPDATE tasks SET details = ?1 WHERE uuid = ?2",
            params![d, uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }
    if let Some(extra) = append_details {
        if extra.is_empty() {
            return Err(user("--append-details text must not be empty"));
        }
        let current: Option<String> = tx
            .query_row(
                "SELECT details FROM tasks WHERE uuid = ?1",
                params![uuid],
                |r| r.get(0),
            )
            .map_err(|e| system(format!("read details failed: {e}")))?;
        let new = match current.as_deref() {
            None | Some("") => extra.to_string(),
            Some(existing) => format!("{existing}\n{extra}"),
        };
        tx.execute(
            "UPDATE tasks SET details = ?1 WHERE uuid = ?2",
            params![new, uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }
    if clear_details {
        tx.execute(
            "UPDATE tasks SET details = NULL WHERE uuid = ?1",
            params![uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }
    Ok(())
}

fn apply_priority(tx: &rusqlite::Transaction, uuid: &str, priority: Option<i64>) -> CliResult<()> {
    if let Some(p) = priority {
        tx.execute(
            "UPDATE tasks SET priority = ?1 WHERE uuid = ?2",
            params![p, uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }
    Ok(())
}

fn apply_gate(tx: &rusqlite::Transaction, uuid: &str, gate: bool, no_gate: bool) -> CliResult<()> {
    // clap's `conflicts_with` already rejects passing both flags together.
    if gate {
        tx.execute(
            "UPDATE tasks SET is_gate = 1 WHERE uuid = ?1",
            params![uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }
    if no_gate {
        tx.execute(
            "UPDATE tasks SET is_gate = 0 WHERE uuid = ?1",
            params![uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }
    Ok(())
}

fn apply_tags(
    tx: &rusqlite::Transaction,
    uuid: &str,
    add_tag: &[String],
    rm_tag: &[String],
) -> CliResult<()> {
    for tag in add_tag {
        tx.execute(
            "INSERT OR IGNORE INTO tags(task_uuid, tag) VALUES(?1, ?2)",
            params![uuid, tag],
        )
        .map_err(|e| system(format!("tag insert failed: {e}")))?;
    }
    for tag in rm_tag {
        tx.execute(
            "DELETE FROM tags WHERE task_uuid = ?1 AND tag = ?2",
            params![uuid, tag],
        )
        .map_err(|e| system(format!("tag delete failed: {e}")))?;
    }
    Ok(())
}

fn apply_deps(
    tx: &rusqlite::Transaction,
    uuid: &str,
    add_dep: &[String],
    rm_dep: &[String],
) -> CliResult<()> {
    for dep in add_dep {
        if would_create_cycle(tx, uuid, dep)? {
            return Err(user("adding this dependency would create a cycle"));
        }
        tx.execute(
            "INSERT OR IGNORE INTO deps(task_uuid, depends_on_uuid) VALUES(?1, ?2)",
            params![uuid, dep],
        )
        .map_err(|e| system(format!("dep insert failed: {e}")))?;
    }
    for dep in rm_dep {
        tx.execute(
            "DELETE FROM deps WHERE task_uuid = ?1 AND depends_on_uuid = ?2",
            params![uuid, dep],
        )
        .map_err(|e| system(format!("dep delete failed: {e}")))?;
    }
    Ok(())
}

/// Apply the mutually-exclusive location edits, same mutex pattern as
/// `apply_details`'s `--details`/`--clear-details`.
fn apply_location(
    tx: &rusqlite::Transaction,
    uuid: &str,
    location: Option<&str>,
    clear_location: bool,
) -> CliResult<()> {
    if location.is_some() && clear_location {
        return Err(user(
            "--location and --clear-location are mutually exclusive",
        ));
    }
    if let Some(l) = location {
        tx.execute(
            "UPDATE tasks SET location = ?1 WHERE uuid = ?2",
            params![l, uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }
    if clear_location {
        tx.execute(
            "UPDATE tasks SET location = NULL WHERE uuid = ?1",
            params![uuid],
        )
        .map_err(|e| system(format!("update failed: {e}")))?;
    }
    Ok(())
}

/// Add/remove mutual "see also" links. Both directions are always written
/// (or removed) together so either task's `show` output reflects the link.
fn apply_related(
    tx: &rusqlite::Transaction,
    uuid: &str,
    add_related: &[String],
    rm_related: &[String],
) -> CliResult<()> {
    for r in add_related {
        tx.execute(
            "INSERT OR IGNORE INTO related(task_uuid, related_uuid) VALUES(?1, ?2)",
            params![uuid, r],
        )
        .map_err(|e| system(format!("related insert failed: {e}")))?;
        tx.execute(
            "INSERT OR IGNORE INTO related(task_uuid, related_uuid) VALUES(?1, ?2)",
            params![r, uuid],
        )
        .map_err(|e| system(format!("related insert failed: {e}")))?;
    }
    for r in rm_related {
        tx.execute(
            "DELETE FROM related WHERE task_uuid = ?1 AND related_uuid = ?2",
            params![uuid, r],
        )
        .map_err(|e| system(format!("related delete failed: {e}")))?;
        tx.execute(
            "DELETE FROM related WHERE task_uuid = ?1 AND related_uuid = ?2",
            params![r, uuid],
        )
        .map_err(|e| system(format!("related delete failed: {e}")))?;
    }
    Ok(())
}

fn would_create_cycle(
    tx: &rusqlite::Transaction,
    task_uuid: &str,
    new_dep: &str,
) -> CliResult<bool> {
    // Adding task_uuid -> new_dep creates a cycle iff new_dep already depends
    // (transitively) on task_uuid. DFS from new_dep's dependencies.
    let mut stack = vec![new_dep.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(node) = stack.pop() {
        if node == task_uuid {
            return Ok(true);
        }
        if !seen.insert(node.clone()) {
            continue;
        }
        let mut stmt = tx
            .prepare("SELECT depends_on_uuid FROM deps WHERE task_uuid = ?1")
            .map_err(|e| system(format!("prepare failed: {e}")))?;
        let rows = stmt
            .query_map(params![node], |r| r.get::<_, String>(0))
            .map_err(|e| system(format!("query failed: {e}")))?;
        for r in rows {
            stack.push(r.map_err(|e| system(format!("row failed: {e}")))?);
        }
    }
    Ok(false)
}

mod common;

use common::Sandbox;

#[test]
fn clean_db_exits_zero_and_reports_clean() {
    let sb = Sandbox::new();
    sb.add("a task");
    let out = sb.cmd().arg("doctor").output().unwrap();
    assert!(out.status.success(), "doctor failed: {:?}", out);
    assert!(String::from_utf8_lossy(&out.stdout).contains("clean"));
}

#[test]
fn duplicate_display_id_is_flagged_and_exits_nonzero() {
    let sb = Sandbox::new();
    let a = sb.add("first");
    let b = sb.add("second");

    let conn = rusqlite::Connection::open(&sb.db).unwrap();
    let b_uuid: String = conn
        .query_row(
            "SELECT uuid FROM tasks WHERE id = ?1",
            rusqlite::params![b],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE tasks SET id = ?1 WHERE uuid = ?2",
        rusqlite::params![a, b_uuid],
    )
    .unwrap();
    drop(conn);

    let out = sb.cmd().arg("doctor").output().unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("duplicate display ids"),
        "stdout: {stdout:?}"
    );
    assert!(stdout.contains(&b_uuid), "stdout: {stdout:?}");

    let json_out = sb.cmd().args(["doctor", "--json"]).output().unwrap();
    assert!(!json_out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&json_out.stdout).unwrap();
    assert_eq!(v["clean"], false);
    assert_eq!(v["duplicate_display_ids"][0]["id"].as_i64().unwrap(), a);
}

#[test]
fn duplicate_display_id_across_different_projects_is_not_flagged() {
    // A coordinator db spanning several projects has this permanently, not
    // just transiently after a merge: each project numbers its own tasks
    // independently, so id collisions across projects are expected and
    // harmless (project_name plus id, or the uuid, still disambiguates).
    let sb = Sandbox::new();
    let a = sb.add("first");
    let b = sb.add("second");

    let conn = rusqlite::Connection::open(&sb.db).unwrap();
    conn.execute(
        "UPDATE tasks SET project_name = 'proj-a' WHERE id = ?1",
        rusqlite::params![a],
    )
    .unwrap();
    let b_uuid: String = conn
        .query_row(
            "SELECT uuid FROM tasks WHERE id = ?1",
            rusqlite::params![b],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE tasks SET id = ?1, project_name = 'proj-b' WHERE uuid = ?2",
        rusqlite::params![a, b_uuid],
    )
    .unwrap();
    drop(conn);

    let out = sb.cmd().arg("doctor").output().unwrap();
    assert!(
        out.status.success(),
        "doctor should stay clean across differing projects: {:?}",
        out
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("clean"));
}

#[test]
fn duplicate_display_id_within_same_project_is_still_flagged() {
    // The positive case, but with project_name set on both sides (same
    // project) -- confirms the GROUP BY (id, project_name) still catches a
    // real duplicate, not just an unset-vs-unset one.
    let sb = Sandbox::new();
    let a = sb.add("first");
    let b = sb.add("second");

    let conn = rusqlite::Connection::open(&sb.db).unwrap();
    conn.execute(
        "UPDATE tasks SET project_name = 'proj-a' WHERE id IN (?1, ?2)",
        rusqlite::params![a, b],
    )
    .unwrap();
    let b_uuid: String = conn
        .query_row(
            "SELECT uuid FROM tasks WHERE id = ?1",
            rusqlite::params![b],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE tasks SET id = ?1 WHERE uuid = ?2",
        rusqlite::params![a, b_uuid],
    )
    .unwrap();
    drop(conn);

    let out = sb.cmd().arg("doctor").output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("duplicate display ids"));
}

#[test]
fn asymmetric_related_row_is_flagged() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    let b = sb.add("b");

    // `edit --add-related` always mirrors both directions — hand-insert only
    // one to simulate a db touched by other tooling.
    let conn = rusqlite::Connection::open(&sb.db).unwrap();
    let a_uuid: String = conn
        .query_row(
            "SELECT uuid FROM tasks WHERE id = ?1",
            rusqlite::params![a],
            |r| r.get(0),
        )
        .unwrap();
    let b_uuid: String = conn
        .query_row(
            "SELECT uuid FROM tasks WHERE id = ?1",
            rusqlite::params![b],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO related(task_uuid, related_uuid) VALUES(?1, ?2)",
        rusqlite::params![a_uuid, b_uuid],
    )
    .unwrap();
    drop(conn);

    let out = sb.cmd().arg("doctor").output().unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("asymmetric related rows"),
        "stdout: {stdout:?}"
    );

    let json_out = sb.cmd().args(["doctor", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&json_out.stdout).unwrap();
    assert_eq!(v["clean"], false);
    assert_eq!(v["asymmetric_related_rows"].as_i64().unwrap(), 1);
}

#[test]
fn orphaned_related_row_is_flagged() {
    let sb = Sandbox::new();
    let a = sb.add("a");

    let conn = rusqlite::Connection::open(&sb.db).unwrap();
    let a_uuid: String = conn
        .query_row(
            "SELECT uuid FROM tasks WHERE id = ?1",
            rusqlite::params![a],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute("PRAGMA foreign_keys = OFF", []).unwrap();
    conn.execute(
        "INSERT INTO related(task_uuid, related_uuid) VALUES(?1, 'does-not-exist')",
        rusqlite::params![a_uuid],
    )
    .unwrap();
    drop(conn);

    let out = sb.cmd().arg("doctor").output().unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("orphaned related rows"),
        "stdout: {stdout:?}"
    );
}

#[test]
fn unresolved_merge_conflict_tag_is_flagged() {
    let sb = Sandbox::new();
    let id = sb.add("conflicted");
    sb.cmd()
        .args(["edit", &id.to_string(), "--add-tag", "merge-conflict"])
        .assert()
        .success();

    let out = sb.cmd().arg("doctor").output().unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("unresolved merge conflicts"),
        "stdout: {stdout:?}"
    );
}

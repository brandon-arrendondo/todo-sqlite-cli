//! Exercises `db::resolve_one` (a display id or a full UUID) directly at the
//! CLI layer, independent of the merge scenarios that motivated it.

mod common;

use common::Sandbox;

fn uuid_of(sb: &Sandbox, id: i64) -> String {
    let out = sb
        .cmd()
        .args(["show", &id.to_string(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "show failed: {:?}", out);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["uuid"].as_str().unwrap().to_string()
}

#[test]
fn show_by_full_uuid_matches_show_by_id() {
    let sb = Sandbox::new();
    let id = sb.add("a task");
    let uuid = uuid_of(&sb, id);

    let out = sb.cmd().args(["show", &uuid, "--json"]).output().unwrap();
    assert!(out.status.success(), "show by uuid failed: {:?}", out);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["id"].as_i64().unwrap(), id);
    assert_eq!(v["title"].as_str().unwrap(), "a task");
}

#[test]
fn garbage_id_is_neither_a_number_nor_a_uuid() {
    let sb = Sandbox::new();
    let out = sb.cmd().args(["show", "not-an-id"]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not a task id or UUID"),
        "stderr: {stderr:?}"
    );
}

#[test]
fn well_formed_but_unknown_uuid_is_not_found_not_ambiguous() {
    let sb = Sandbox::new();
    sb.add("a task");
    let out = sb
        .cmd()
        .args(["show", "00000000-0000-4000-8000-000000000000"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not found"), "stderr: {stderr:?}");
}

#[test]
fn edit_add_dep_on_ambiguous_display_id_is_rejected() {
    // Simulate a post-merge db with two tasks sharing display id 1: init
    // twice and hand-splice a second row in directly (cheaper than a full
    // merge fixture for this one case).
    let sb = Sandbox::new();
    let a = sb.add("first");
    let b = sb.add("second");

    // Force `b` to display id 1 too, so `--add-dep 1` becomes ambiguous.
    let conn = rusqlite::Connection::open(&sb.db).unwrap();
    let b_uuid = uuid_of(&sb, b);
    conn.execute(
        "UPDATE tasks SET id = 1 WHERE uuid = ?1",
        rusqlite::params![b_uuid],
    )
    .unwrap();
    drop(conn);

    let out = sb
        .cmd()
        .args(["edit", &a.to_string(), "--add-dep", "1"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ambiguous"), "stderr: {stderr:?}");
}

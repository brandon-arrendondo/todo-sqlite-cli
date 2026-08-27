mod common;

use common::Sandbox;

#[test]
fn renumber_reassigns_display_id() {
    let sb = Sandbox::new();
    let a = sb.add("a task");

    let out = sb
        .cmd()
        .args(["renumber", &a.to_string(), "42"])
        .output()
        .unwrap();
    assert!(out.status.success(), "renumber failed: {:?}", out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("renumbered {a} -> 42")), "stdout: {stdout:?}");

    let out = sb.cmd().args(["show", "42", "--json"]).output().unwrap();
    assert!(out.status.success(), "show failed: {:?}", out);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["id"].as_i64().unwrap(), 42);
    assert_eq!(v["title"].as_str().unwrap(), "a task");
}

#[test]
fn renumber_by_uuid_resolves_a_duplicate_display_id() {
    // Simulate a post-merge db with two tasks sharing display id 1 (cheaper
    // than a full merge fixture for this one case), then use renumber to
    // resolve the conflict on one of them.
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

    // Plain id is now ambiguous between the two tasks.
    let out = sb.cmd().args(["show", &a.to_string()]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ambiguous"));

    let out = sb
        .cmd()
        .args(["renumber", &b_uuid, "99"])
        .output()
        .unwrap();
    assert!(out.status.success(), "renumber failed: {:?}", out);

    let out = sb.cmd().arg("doctor").output().unwrap();
    assert!(out.status.success(), "doctor failed after renumber: {:?}", out);
}

#[test]
fn renumber_onto_id_already_in_use_is_rejected_without_force() {
    let sb = Sandbox::new();
    let a = sb.add("first");
    let b = sb.add("second");

    let out = sb
        .cmd()
        .args(["renumber", &a.to_string(), &b.to_string()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already in use"), "stderr: {stderr:?}");

    let out = sb
        .cmd()
        .args(["renumber", &a.to_string(), &b.to_string(), "--force"])
        .output()
        .unwrap();
    assert!(out.status.success(), "renumber --force failed: {:?}", out);
}

#[test]
fn renumber_rejects_non_positive_new_id() {
    let sb = Sandbox::new();
    let a = sb.add("a task");

    let out = sb
        .cmd()
        .args(["renumber", &a.to_string(), "0"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("positive integer"));
}

#[test]
fn renumber_to_same_id_is_rejected() {
    let sb = Sandbox::new();
    let a = sb.add("a task");

    let out = sb
        .cmd()
        .args(["renumber", &a.to_string(), &a.to_string()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("already has id"));
}

#[test]
fn renumber_json_output() {
    let sb = Sandbox::new();
    let a = sb.add("a task");

    let out = sb
        .cmd()
        .args(["renumber", &a.to_string(), "7", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "renumber failed: {:?}", out);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["old_id"].as_i64().unwrap(), a);
    assert_eq!(v["id"].as_i64().unwrap(), 7);
    assert!(v["uuid"].as_str().is_some());
}

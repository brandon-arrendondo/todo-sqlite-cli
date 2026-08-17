mod common;

use common::Sandbox;

#[test]
fn aging_excludes_done_and_rejected() {
    let sb = Sandbox::new();
    let open = sb.add("open one");
    let done = sb.add("done one");
    let rejected = sb.add("rejected one");
    sb.cmd()
        .args(["done", &done.to_string()])
        .assert()
        .success();
    sb.cmd()
        .args(["done", &rejected.to_string(), "--rejected"])
        .assert()
        .success();

    let out = sb.cmd().args(["aging", "--json"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"].as_i64().unwrap(), open);
}

#[test]
fn aging_stale_days_flags_old_tasks() {
    let sb = Sandbox::new();
    let a = sb.add("a");

    // Backdate created_at directly so the task reads as old without waiting.
    let conn = rusqlite::Connection::open(&sb.db).unwrap();
    conn.execute(
        "UPDATE tasks SET created_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
        rusqlite::params![a],
    )
    .unwrap();

    let out = sb
        .cmd()
        .args(["aging", "--stale-days", "1", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0]["stale"].as_bool().unwrap());
    assert!(tasks[0]["age_days"].as_i64().unwrap() > 1000);
}

#[test]
fn aging_without_stale_days_never_flags() {
    let sb = Sandbox::new();
    sb.add("a");

    let out = sb.cmd().args(["aging", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(!tasks[0]["stale"].as_bool().unwrap());
}

#[test]
fn aging_tag_filter() {
    let sb = Sandbox::new();
    sb.add_with(&["tagged", "--tag", "foo"]);
    sb.add("untagged");

    let out = sb
        .cmd()
        .args(["aging", "--tag", "foo", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["title"].as_str().unwrap(), "tagged");
}

#[test]
fn aging_table_format_is_human_readable() {
    let sb = Sandbox::new();
    sb.add("plain text task");

    let out = sb.cmd().args(["aging"]).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("plain text task"));
    assert!(s.contains("age="));
}

#[test]
fn aging_sorted_oldest_first() {
    let sb = Sandbox::new();
    let newer = sb.add("newer");
    let older = sb.add("older");

    let conn = rusqlite::Connection::open(&sb.db).unwrap();
    conn.execute(
        "UPDATE tasks SET created_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
        rusqlite::params![older],
    )
    .unwrap();
    conn.execute(
        "UPDATE tasks SET created_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
        rusqlite::params![newer],
    )
    .unwrap();

    let out = sb.cmd().args(["aging", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(tasks[0]["id"].as_i64().unwrap(), older);
    assert_eq!(tasks[1]["id"].as_i64().unwrap(), newer);
}

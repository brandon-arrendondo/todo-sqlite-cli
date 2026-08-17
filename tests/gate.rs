mod common;

use common::Sandbox;

#[test]
fn add_gate_sets_flag() {
    let sb = Sandbox::new();
    let out = sb
        .cmd()
        .args(["add", "wait for sqc stability", "--gate", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["task"]["is_gate"].as_bool().unwrap());
}

#[test]
fn add_without_gate_defaults_false() {
    let sb = Sandbox::new();
    let id = sb.add("regular task");
    let out = sb
        .cmd()
        .args(["show", &id.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!v["is_gate"].as_bool().unwrap());
}

#[test]
fn edit_gate_and_no_gate_toggle() {
    let sb = Sandbox::new();
    let id = sb.add("promote me");

    sb.cmd()
        .args(["edit", &id.to_string(), "--gate"])
        .assert()
        .success();
    let out = sb
        .cmd()
        .args(["show", &id.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["is_gate"].as_bool().unwrap());

    sb.cmd()
        .args(["edit", &id.to_string(), "--no-gate"])
        .assert()
        .success();
    let out = sb
        .cmd()
        .args(["show", &id.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!v["is_gate"].as_bool().unwrap());
}

#[test]
fn edit_gate_and_no_gate_together_rejected() {
    let sb = Sandbox::new();
    let id = sb.add("a");
    sb.cmd()
        .args(["edit", &id.to_string(), "--gate", "--no-gate"])
        .assert()
        .failure();
}

#[test]
fn next_skips_unblocked_gate_and_falls_through_to_real_task() {
    let sb = Sandbox::new();
    let _gate = sb.add_with(&["a gate", "--gate", "--priority", "1"]);
    let real = sb.add_with(&["real work", "--priority", "5"]);

    let out = sb.cmd().args(["next", "--json"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["id"].as_i64().unwrap(), real);
}

#[test]
fn next_reports_nothing_when_only_gates_remain() {
    let sb = Sandbox::new();
    sb.add_with(&["only a gate", "--gate"]);

    let out = sb.cmd().args(["next", "--json"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v.is_null());
}

#[test]
fn aging_never_flags_a_gate_stale() {
    let sb = Sandbox::new();
    let gate = sb.add_with(&["ancient gate", "--gate"]);
    let conn = rusqlite::Connection::open(&sb.db).unwrap();
    conn.execute(
        "UPDATE tasks SET created_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
        rusqlite::params![gate],
    )
    .unwrap();

    let out = sb
        .cmd()
        .args(["aging", "--stale-days", "1", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(!tasks[0]["stale"].as_bool().unwrap());
    assert!(tasks[0]["is_gate"].as_bool().unwrap());
}

#[test]
fn list_kind_gate_returns_only_gates() {
    let sb = Sandbox::new();
    sb.add_with(&["a gate", "--gate"]);
    sb.add("a regular task");

    let out = sb
        .cmd()
        .args(["list", "--kind", "gate", "--status", "all", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0]["is_gate"].as_bool().unwrap());
}

#[test]
fn list_kind_task_excludes_gates() {
    let sb = Sandbox::new();
    sb.add_with(&["a gate", "--gate"]);
    sb.add("a regular task");

    let out = sb
        .cmd()
        .args(["list", "--kind", "task", "--status", "all", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(!tasks[0]["is_gate"].as_bool().unwrap());
}

#[test]
fn list_table_prefixes_gate_title() {
    let sb = Sandbox::new();
    sb.add_with(&["condition met check", "--gate"]);

    let out = sb.cmd().args(["list"]).output().unwrap();
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("[GATE] condition met check"));
}

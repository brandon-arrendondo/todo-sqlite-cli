mod common;

use common::Sandbox;
use predicates::prelude::*;

#[test]
fn only_one_in_progress_at_a_time() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    let b = sb.add("b");
    sb.cmd().args(["start", &a.to_string()]).assert().success();
    sb.cmd()
        .args(["start", &b.to_string()])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn force_overrides_single_in_progress() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    let b = sb.add("b");
    sb.cmd().args(["start", &a.to_string()]).assert().success();
    sb.cmd()
        .args(["start", &b.to_string(), "--force"])
        .assert()
        .success();
}

#[test]
fn done_is_idempotent() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    sb.cmd().args(["done", &a.to_string()]).assert().success();

    let out = sb
        .cmd()
        .args(["show", &a.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let first_completed = v["completed_at"].as_str().unwrap().to_string();

    std::thread::sleep(std::time::Duration::from_millis(1100));
    sb.cmd().args(["done", &a.to_string()]).assert().success();

    let out = sb
        .cmd()
        .args(["show", &a.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["completed_at"].as_str().unwrap(), first_completed);
}

#[test]
fn start_refuses_blocked_task_and_force_allows_it() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    let b = sb.add_with(&["b", "--depends-on", &a.to_string()]);
    sb.cmd()
        .args(["start", &b.to_string()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unmet dependencies"));
    sb.cmd()
        .args(["start", &b.to_string(), "--force"])
        .assert()
        .success();
}

#[test]
fn next_skips_blocked_pending_tasks() {
    let sb = Sandbox::new();
    let dep = sb.add("dep");
    let blocked = sb.add_with(&[
        "blocked",
        "--priority",
        "1",
        "--depends-on",
        &dep.to_string(),
    ]);
    let other = sb.add("other");

    let out = sb.cmd().args(["next", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = v["id"].as_i64().unwrap();
    assert_ne!(id, blocked, "should not recommend blocked task");
    assert!(id == dep || id == other);
}

#[test]
fn next_prefers_in_progress_over_higher_priority_pending() {
    let sb = Sandbox::new();
    let running = sb.add_with(&["running", "--priority", "5", "--start"]);
    let _higher = sb.add_with(&["urgent", "--priority", "1"]);
    let out = sb.cmd().args(["next", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["id"].as_i64().unwrap(), running);
}

#[test]
fn add_with_start_fails_if_another_task_in_progress() {
    let sb = Sandbox::new();
    let _a = sb.add_with(&["a", "--start"]);
    let mut cmd = sb.cmd();
    cmd.args(["add", "b", "--start"]);
    cmd.assert().failure().code(1);
}

#[test]
fn edit_dependency_cycle_rejected() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    let b = sb.add_with(&["b", "--depends-on", &a.to_string()]);
    sb.cmd()
        .args(["edit", &a.to_string(), "--add-dep", &b.to_string()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cycle"));
}

#[test]
fn stop_returns_in_progress_to_pending_and_preserves_started_at() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    sb.cmd().args(["start", &a.to_string()]).assert().success();

    let out = sb
        .cmd()
        .args(["show", &a.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let started = v["started_at"].as_str().unwrap().to_string();

    sb.cmd().args(["stop", &a.to_string()]).assert().success();

    let out = sb
        .cmd()
        .args(["show", &a.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["status"].as_str().unwrap(), "pending");
    assert_eq!(v["started_at"].as_str().unwrap(), started);
}

#[test]
fn stop_then_start_is_allowed_for_other_tasks() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    let b = sb.add("b");
    sb.cmd().args(["start", &a.to_string()]).assert().success();
    sb.cmd().args(["stop", &a.to_string()]).assert().success();
    sb.cmd().args(["start", &b.to_string()]).assert().success();
}

#[test]
fn stop_refuses_done_task() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    sb.cmd().args(["done", &a.to_string()]).assert().success();
    sb.cmd()
        .args(["stop", &a.to_string()])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn revert_clears_started_at() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    sb.cmd().args(["start", &a.to_string()]).assert().success();
    sb.cmd()
        .args(["revert", &a.to_string()])
        .assert()
        .success();

    let out = sb
        .cmd()
        .args(["show", &a.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["status"].as_str().unwrap(), "pending");
    assert!(v["started_at"].is_null());
}

#[test]
fn revert_refuses_done_task() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    sb.cmd().args(["done", &a.to_string()]).assert().success();
    sb.cmd()
        .args(["revert", &a.to_string()])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn rm_cascades_deps_and_tags() {
    let sb = Sandbox::new();
    let a = sb.add_with(&["a", "--tag", "t1"]);
    let _b = sb.add_with(&["b", "--depends-on", &a.to_string()]);
    sb.cmd().args(["rm", &a.to_string()]).assert().success();
    // list still works; b is now unblocked
    let out = sb.cmd().args(["list", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    for t in v["tasks"].as_array().unwrap() {
        assert_eq!(t["blocked"].as_bool().unwrap(), false);
    }
}

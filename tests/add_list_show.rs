mod common;

use common::Sandbox;
use predicates::prelude::*;

#[test]
fn add_prints_id_and_stores_task() {
    let sb = Sandbox::new();
    let id1 = sb.add("alpha");
    let id2 = sb.add("beta");
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);

    sb.cmd()
        .args(["show", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Title: alpha"));
}

#[test]
fn add_rejects_empty_title() {
    let sb = Sandbox::new();
    sb.cmd().args(["add", ""]).assert().failure().code(1);
}

#[test]
fn id_never_reused_after_rm() {
    let sb = Sandbox::new();
    let _ = sb.add("a");
    let b = sb.add("b");
    let _ = sb.add("c");
    sb.cmd().args(["rm", &b.to_string()]).assert().success();
    let next_id = sb.add("d");
    assert_eq!(next_id, 4, "AUTOINCREMENT must never reuse IDs");
}

#[test]
fn list_orders_in_progress_first_then_priority() {
    let sb = Sandbox::new();
    let low = sb.add_with(&["low", "--priority", "5"]);
    let high = sb.add_with(&["high", "--priority", "1"]);
    let mid = sb.add_with(&["mid", "--priority", "3", "--start"]);

    let out = sb.cmd().args(["list", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v["tasks"].as_array().unwrap();
    let ids: Vec<i64> = tasks.iter().map(|t| t["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, vec![mid, high, low]);
}

#[test]
fn list_ndjson_emits_one_object_per_line_no_wrapper() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    let b = sb.add("b");

    let out = sb
        .cmd()
        .args(["list", "--format", "ndjson"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = s.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2, "expected 2 NDJSON lines, got:\n{s}");

    // Each line must parse independently to a bare task object (no wrapper key).
    let ids: Vec<i64> = lines
        .iter()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            assert!(v.get("tasks").is_none(), "NDJSON line must not be wrapped");
            v["id"].as_i64().unwrap()
        })
        .collect();
    assert_eq!(ids, vec![a, b]);
}

#[test]
fn list_explicit_ndjson_format_wins_over_json_flag() {
    // `--format ndjson` is more specific than the global `--json`; verify the
    // user-visible contract from the recommendation: explicit format wins.
    let sb = Sandbox::new();
    let _a = sb.add("a");
    let _b = sb.add("b");

    let out = sb
        .cmd()
        .args(["--json", "list", "--format", "ndjson"])
        .output()
        .unwrap();
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(
        !s.contains("\"tasks\":"),
        "expected NDJSON (no wrapper), got:\n{s}"
    );
    let line_count = s.lines().filter(|l| !l.is_empty()).count();
    assert_eq!(line_count, 2);
}

#[test]
fn list_invalid_format_lists_ndjson_in_error() {
    let sb = Sandbox::new();
    sb.add("a");
    let out = sb
        .cmd()
        .args(["list", "--format", "bogus"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let s = String::from_utf8(out.stderr).unwrap();
    assert!(
        s.contains("ndjson"),
        "error must advertise ndjson, got: {s}"
    );
}

#[test]
fn show_ndjson_emits_bare_object_on_one_line() {
    let sb = Sandbox::new();
    let id = sb.add("my task");

    let out = sb
        .cmd()
        .args(["show", &id.to_string(), "--format", "ndjson"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = s.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one NDJSON line, got:\n{s}"
    );
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert!(v.get("tasks").is_none(), "ndjson must not be wrapped");
    assert_eq!(v["id"].as_i64().unwrap(), id);
    assert_eq!(v["title"].as_str().unwrap(), "my task");
}

#[test]
fn show_format_json_emits_bare_object() {
    let sb = Sandbox::new();
    let id = sb.add("another task");

    let out = sb
        .cmd()
        .args(["show", &id.to_string(), "--format", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v.get("tasks").is_none(),
        "show --format json must emit bare object"
    );
    assert_eq!(v["id"].as_i64().unwrap(), id);
}

#[test]
fn show_json_flag_still_works() {
    let sb = Sandbox::new();
    let id = sb.add("flagged task");

    let out = sb
        .cmd()
        .args(["--json", "show", &id.to_string()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["id"].as_i64().unwrap(), id);
}

#[test]
fn show_ndjson_consistent_with_list_ndjson() {
    // A task fetched via `show --format ndjson` must parse the same way as the same task
    // from `list --format ndjson` — one bare object per line, no wrapper.
    let sb = Sandbox::new();
    let id = sb.add("shared task");

    let show_out = sb
        .cmd()
        .args(["show", &id.to_string(), "--format", "ndjson"])
        .output()
        .unwrap();
    let list_out = sb
        .cmd()
        .args(["list", "--format", "ndjson"])
        .output()
        .unwrap();

    let show_v: serde_json::Value =
        serde_json::from_str(String::from_utf8(show_out.stdout).unwrap().trim()).unwrap();
    let list_v: serde_json::Value =
        serde_json::from_str(String::from_utf8(list_out.stdout).unwrap().trim()).unwrap();
    assert_eq!(show_v["id"], list_v["id"]);
    assert_eq!(show_v["title"], list_v["title"]);
}

#[test]
fn list_tag_filter_matches_only_tagged_tasks() {
    let sb = Sandbox::new();
    let a = sb.add_with(&["alpha", "--tag", "red"]);
    let _ = sb.add_with(&["beta", "--tag", "blue"]);
    let c = sb.add_with(&["gamma", "--tag", "red"]);

    let out = sb
        .cmd()
        .args(["list", "--tag", "red", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<i64> = v["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![a, c]);
}

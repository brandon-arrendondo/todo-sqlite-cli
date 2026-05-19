mod common;

use common::Sandbox;

#[test]
fn export_todo_json_shape_active_only() {
    let sb = Sandbox::new();
    let _a = sb.add_with(&["a", "--start"]);
    let _b = sb.add("b");
    let c = sb.add("c");
    sb.cmd().args(["done", &c.to_string()]).assert().success();

    let out = sb
        .cmd()
        .args(["export-todo", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2);
    for t in tasks {
        let s = t["status"].as_str().unwrap();
        assert!(s == "in-progress" || s == "pending");
    }
}

#[test]
fn export_completed_groups_by_date() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    let b = sb.add("b");
    sb.cmd().args(["done", &a.to_string()]).assert().success();
    sb.cmd().args(["done", &b.to_string()]).assert().success();

    let out = sb.cmd().args(["export-completed"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let groups = v["completed"].as_array().unwrap();
    assert!(!groups.is_empty());
    let total: usize = groups
        .iter()
        .map(|g| g["tasks"].as_array().unwrap().len())
        .sum();
    assert_eq!(total, 2);
}

#[test]
fn export_todo_ndjson_one_per_line() {
    let sb = Sandbox::new();
    let _a = sb.add("a");
    let _b = sb.add("b");

    let out = sb
        .cmd()
        .args(["export-todo", "--format", "ndjson"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = s.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2);
    for l in &lines {
        let v: serde_json::Value = serde_json::from_str(l).unwrap();
        assert!(v.get("id").is_some(), "NDJSON line must be a bare task");
        assert!(v.get("tasks").is_none(), "must not be wrapped");
    }
}

#[test]
fn export_completed_ndjson_is_flat() {
    // Default export-completed groups by date; NDJSON should drop the grouping
    // and emit each task as its own line (date is recoverable via completed_at).
    let sb = Sandbox::new();
    let a = sb.add("a");
    let b = sb.add("b");
    sb.cmd().args(["done", &a.to_string()]).assert().success();
    sb.cmd().args(["done", &b.to_string()]).assert().success();

    let out = sb
        .cmd()
        .args(["export-completed", "--format", "ndjson"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = s.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2, "expected 2 flat NDJSON tasks, got:\n{s}");
    for l in &lines {
        let v: serde_json::Value = serde_json::from_str(l).unwrap();
        assert!(
            v.get("completed_at").is_some(),
            "task must carry its own date"
        );
        assert!(v.get("completed").is_none(), "must not be wrapped/grouped");
    }
}

#[test]
fn export_completed_since_filter() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    sb.cmd().args(["done", &a.to_string()]).assert().success();

    // Since tomorrow — should be empty.
    let tomorrow = chrono::Utc::now().date_naive() + chrono::Duration::days(1);
    let out = sb
        .cmd()
        .args(["export-completed", "--since", &tomorrow.to_string()])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let total: usize = v["completed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["tasks"].as_array().unwrap().len())
        .sum();
    assert_eq!(total, 0);
}

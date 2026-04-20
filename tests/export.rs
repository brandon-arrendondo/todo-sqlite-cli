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

mod common;

use common::Sandbox;
use predicates::prelude::*;

#[test]
fn add_project_name_shows_up_on_show() {
    let sb = Sandbox::new();
    let a = sb.add_with(&["task a", "--project-name", "widgets"]);

    sb.cmd()
        .args(["show", &a.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project: widgets"));
}

#[test]
fn project_name_shows_up_as_plus_suffix_in_list_table() {
    let sb = Sandbox::new();
    sb.add_with(&["task a", "--project-name", "widgets"]);

    sb.cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("task a +widgets"));
}

#[test]
fn edit_project_name_and_clear_project_name() {
    let sb = Sandbox::new();
    let a = sb.add("task a");

    sb.cmd()
        .args(["edit", &a.to_string(), "--project-name", "gadgets"])
        .assert()
        .success();
    sb.cmd()
        .args(["show", &a.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project: gadgets"));

    sb.cmd()
        .args(["edit", &a.to_string(), "--clear-project-name"])
        .assert()
        .success();
    sb.cmd()
        .args(["show", &a.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project:").not());
}

#[test]
fn edit_project_name_and_clear_project_name_together_rejected() {
    let sb = Sandbox::new();
    let a = sb.add("task a");

    sb.cmd()
        .args([
            "edit",
            &a.to_string(),
            "--project-name",
            "gadgets",
            "--clear-project-name",
        ])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn project_name_present_in_json_show_and_list() {
    let sb = Sandbox::new();
    let a = sb.add_with(&["task a", "--project-name", "widgets"]);
    let b = sb.add("task b");

    let show_v: serde_json::Value = serde_json::from_slice(
        &sb.cmd()
            .args(["show", &a.to_string(), "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(show_v["project_name"].as_str().unwrap(), "widgets");

    let list_out = sb.cmd().args(["list", "--json"]).output().unwrap();
    let list_v: serde_json::Value = serde_json::from_slice(&list_out.stdout).unwrap();
    let tasks = list_v["tasks"].as_array().unwrap();
    let by_id = |id: i64| {
        tasks
            .iter()
            .find(|t| t["id"].as_i64().unwrap() == id)
            .unwrap()
    };
    assert_eq!(by_id(a)["project_name"].as_str().unwrap(), "widgets");
    assert!(by_id(b)["project_name"].is_null());
}

#[test]
fn list_filters_by_project_name() {
    let sb = Sandbox::new();
    let a = sb.add_with(&["task a", "--project-name", "widgets"]);
    sb.add_with(&["task b", "--project-name", "gadgets"]);
    sb.add("task c");

    let out = sb
        .cmd()
        .args(["list", "--project-name", "widgets", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"].as_i64().unwrap(), a);
}

#[test]
fn project_name_survives_a_clean_two_way_merge() {
    let sb = Sandbox::new();
    let a = sb.add_with(&["task a", "--project-name", "widgets"]);

    // A "theirs" db with the same task, untouched.
    let theirs_dir = tempfile::TempDir::new().unwrap();
    let theirs_db = theirs_dir.path().join("theirs.db");
    std::fs::copy(&sb.db, &theirs_db).unwrap();

    let mut merge_cmd = assert_cmd::Command::cargo_bin("todo-sqlite-cli").unwrap();
    merge_cmd
        .env_remove("TODO_SQLITE_CLI_DB")
        .args([
            "merge",
            "--ours",
            sb.db.to_str().unwrap(),
            "--theirs",
            theirs_db.to_str().unwrap(),
        ])
        .assert()
        .success();

    sb.cmd()
        .args(["show", &a.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project: widgets"));
}

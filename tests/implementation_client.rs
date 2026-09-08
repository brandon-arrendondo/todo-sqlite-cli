mod common;

use common::Sandbox;
use predicates::prelude::*;

#[test]
fn add_implementation_client_shows_up_on_show() {
    let sb = Sandbox::new();
    let a = sb.add_with(&["task a", "--implementation-client", "worker-1"]);

    sb.cmd()
        .args(["show", &a.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Client: worker-1"));
}

#[test]
fn implementation_client_does_not_appear_in_list_table() {
    let sb = Sandbox::new();
    sb.add_with(&["task a", "--implementation-client", "worker-1"]);

    sb.cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("worker-1").not());
}

#[test]
fn edit_implementation_client_and_clear_implementation_client() {
    let sb = Sandbox::new();
    let a = sb.add("task a");

    sb.cmd()
        .args([
            "edit",
            &a.to_string(),
            "--implementation-client",
            "worker-2",
        ])
        .assert()
        .success();
    sb.cmd()
        .args(["show", &a.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Client: worker-2"));

    sb.cmd()
        .args(["edit", &a.to_string(), "--clear-implementation-client"])
        .assert()
        .success();
    sb.cmd()
        .args(["show", &a.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Client:").not());
}

#[test]
fn edit_implementation_client_and_clear_implementation_client_together_rejected() {
    let sb = Sandbox::new();
    let a = sb.add("task a");

    sb.cmd()
        .args([
            "edit",
            &a.to_string(),
            "--implementation-client",
            "worker-2",
            "--clear-implementation-client",
        ])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn implementation_client_present_in_json_show_and_list() {
    let sb = Sandbox::new();
    let a = sb.add_with(&["task a", "--implementation-client", "worker-3"]);
    let b = sb.add("task b");

    let show_v: serde_json::Value = serde_json::from_slice(
        &sb.cmd()
            .args(["show", &a.to_string(), "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        show_v["implementation_client"].as_str().unwrap(),
        "worker-3"
    );

    let list_out = sb.cmd().args(["list", "--json"]).output().unwrap();
    let list_v: serde_json::Value = serde_json::from_slice(&list_out.stdout).unwrap();
    let tasks = list_v["tasks"].as_array().unwrap();
    let by_id = |id: i64| {
        tasks
            .iter()
            .find(|t| t["id"].as_i64().unwrap() == id)
            .unwrap()
    };
    assert_eq!(
        by_id(a)["implementation_client"].as_str().unwrap(),
        "worker-3"
    );
    assert!(by_id(b)["implementation_client"].is_null());
}

#[test]
fn implementation_client_survives_a_clean_two_way_merge() {
    let sb = Sandbox::new();
    let a = sb.add_with(&["task a", "--implementation-client", "worker-1"]);

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
        .stdout(predicate::str::contains("Client: worker-1"));
}

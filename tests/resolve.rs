mod common;

use assert_cmd::Command;
use common::Sandbox;

#[test]
fn env_var_resolves_db() {
    let sb = Sandbox::new();
    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.env("TODO_SQLITE_CLI_DB", &sb.db)
        .args(["add", "hello"])
        .assert()
        .success();
}

#[test]
fn flag_beats_env_var() {
    let sb1 = Sandbox::new();
    let sb2 = Sandbox::new();
    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.env("TODO_SQLITE_CLI_DB", &sb1.db)
        .args(["--db", sb2.db.to_str().unwrap(), "add", "flagwins"])
        .assert()
        .success();

    // sb2 should have the task, sb1 should not.
    let out2 = Command::cargo_bin("todo-sqlite-cli")
        .unwrap()
        .env_remove("TODO_SQLITE_CLI_DB")
        .args(["--db", sb2.db.to_str().unwrap(), "list", "--json"])
        .output()
        .unwrap();
    let v2: serde_json::Value = serde_json::from_slice(&out2.stdout).unwrap();
    assert_eq!(v2["tasks"].as_array().unwrap().len(), 1);

    let out1 = Command::cargo_bin("todo-sqlite-cli")
        .unwrap()
        .env_remove("TODO_SQLITE_CLI_DB")
        .args(["--db", sb1.db.to_str().unwrap(), "list", "--json"])
        .output()
        .unwrap();
    let v1: serde_json::Value = serde_json::from_slice(&out1.stdout).unwrap();
    assert_eq!(v1["tasks"].as_array().unwrap().len(), 0);
}

#[test]
fn marker_walk_up_from_nested_cwd() {
    let sb = Sandbox::raw();
    // init with marker in sb.path()
    Command::cargo_bin("todo-sqlite-cli")
        .unwrap()
        .env_remove("TODO_SQLITE_CLI_DB")
        .current_dir(sb.path())
        .arg("init")
        .assert()
        .success();
    let nested = sb.path().join("a").join("b").join("c");
    std::fs::create_dir_all(&nested).unwrap();
    Command::cargo_bin("todo-sqlite-cli")
        .unwrap()
        .env_remove("TODO_SQLITE_CLI_DB")
        .current_dir(&nested)
        .args(["add", "from-nested"])
        .assert()
        .success();
}

#[test]
fn missing_everything_errors_with_hint() {
    let sb = Sandbox::raw();
    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.env_remove("TODO_SQLITE_CLI_DB")
        .current_dir(sb.path())
        .args(["list"]);
    cmd.assert()
        .failure()
        .code(1)
        .stderr(predicates::prelude::predicate::str::contains("init"));
}

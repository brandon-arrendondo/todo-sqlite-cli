mod common;

use assert_cmd::Command;
use common::Sandbox;

#[test]
fn init_creates_db_and_marker() {
    let sb = Sandbox::raw();
    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.arg("--db").arg(&sb.db).arg("init").assert().success();
    assert!(sb.db.exists(), "db should exist after init");
}

#[test]
fn init_refuses_to_clobber_existing_db() {
    let sb = Sandbox::new(); // already initialized
    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.arg("--db").arg(&sb.db).arg("init");
    cmd.assert().failure().code(1);
}

#[test]
fn init_writes_marker_with_cwd_when_no_db_flag() {
    let sb = Sandbox::raw();
    let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
    cmd.env_remove("TODO_SQLITE_CLI_DB")
        .current_dir(sb.path())
        .arg("init");
    cmd.assert().success();
    let marker = sb.path().join(".todo-sqlite-cli");
    assert!(marker.is_file(), "marker should exist");
    let contents = std::fs::read_to_string(&marker).unwrap();
    assert!(
        contents.contains("todo-sqlite-cli.db"),
        "marker should reference DB path"
    );
}

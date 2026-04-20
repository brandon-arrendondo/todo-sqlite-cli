#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use tempfile::TempDir;

pub struct Sandbox {
    pub dir: TempDir,
    pub db: PathBuf,
}

impl Sandbox {
    /// New sandbox with an initialized DB.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("todo.db");
        let mut cmd = Command::cargo_bin("todo-sqlite-cli").unwrap();
        cmd.arg("--db").arg(&db).arg("init");
        cmd.assert().success();
        Sandbox { dir, db }
    }

    /// Sandbox without initializing — caller exercises init itself.
    pub fn raw() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("todo.db");
        Sandbox { dir, db }
    }

    pub fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("todo-sqlite-cli").unwrap();
        c.arg("--db").arg(&self.db);
        c.env_remove("TODO_SQLITE_CLI_DB");
        c
    }

    /// Command without --db; caller sets env / cwd / marker manually.
    pub fn bare_cmd(&self) -> Command {
        let mut c = Command::cargo_bin("todo-sqlite-cli").unwrap();
        c.env_remove("TODO_SQLITE_CLI_DB");
        c
    }

    pub fn add(&self, title: &str) -> i64 {
        let out = self.cmd().args(["add", title]).output().unwrap();
        assert!(out.status.success(), "add failed: {:?}", out);
        parse_id(&out)
    }

    pub fn add_with(&self, args: &[&str]) -> i64 {
        let mut c = self.cmd();
        c.arg("add");
        c.args(args);
        let out = c.output().unwrap();
        assert!(out.status.success(), "add failed: {:?}", out);
        parse_id(&out)
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

fn parse_id(out: &Output) -> i64 {
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim().parse().expect("id on stdout")
}

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

struct MergeFixture {
    dir: TempDir,
}

impl MergeFixture {
    fn new() -> Self {
        MergeFixture {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn cmd(&self, db: &Path) -> Command {
        let mut c = Command::cargo_bin("todo-sqlite-cli").unwrap();
        c.arg("--db").arg(db);
        c.env_remove("TODO_SQLITE_CLI_DB");
        c
    }

    fn init(&self, name: &str) -> PathBuf {
        let p = self.path(name);
        self.cmd(&p).arg("init").assert().success();
        p
    }

    fn copy(&self, src: &Path, name: &str) -> PathBuf {
        let dst = self.path(name);
        std::fs::copy(src, &dst).unwrap();
        dst
    }

    fn add(&self, db: &Path, args: &[&str]) -> i64 {
        let mut c = self.cmd(db);
        c.arg("add");
        c.args(args);
        let out = c.output().unwrap();
        assert!(out.status.success(), "add failed: {:?}", out);
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let mut c = Command::cargo_bin("todo-sqlite-cli").unwrap();
        c.env_remove("TODO_SQLITE_CLI_DB");
        c.arg("merge");
        c.args(args);
        c.output().unwrap()
    }

    fn show_json(&self, db: &Path, id: i64) -> serde_json::Value {
        let out = self
            .cmd(db)
            .args(["show", &id.to_string(), "--json"])
            .output()
            .unwrap();
        assert!(out.status.success(), "show failed: {:?}", out);
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn list_ids(&self, db: &Path) -> Vec<i64> {
        let out = self
            .cmd(db)
            .args(["list", "--status", "all", "--ids-only"])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.parse().unwrap())
            .collect()
    }
}

#[test]
fn one_side_field_changes_both_carry_over() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    let id = fx.add(&base, &["shared task"]);

    let ours = fx.copy(&base, "ours.db");
    let theirs = fx.copy(&base, "theirs.db");

    fx.cmd(&ours)
        .args(["start", &id.to_string()])
        .assert()
        .success();
    fx.cmd(&theirs)
        .args(["edit", &id.to_string(), "--append-details", "from theirs"])
        .assert()
        .success();

    let merged = fx.path("merged.db");
    let out = fx.run(&[
        "--base",
        base.to_str().unwrap(),
        "--ours",
        ours.to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
        "--into",
        merged.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "merge failed: {:?}", out);

    let v = fx.show_json(&merged, id);
    assert_eq!(v["status"].as_str().unwrap(), "in-progress");
    assert_eq!(v["details"].as_str().unwrap(), "from theirs");
}

#[test]
fn new_tasks_on_both_sides_union_with_renumbering() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    fx.add(&base, &["shared task"]);

    let ours = fx.copy(&base, "ours.db");
    let theirs = fx.copy(&base, "theirs.db");

    let ours_new = fx.add(&ours, &["ours-only"]);
    let theirs_new = fx.add(&theirs, &["theirs-only"]);
    // Both allocated the same next id independently.
    assert_eq!(ours_new, theirs_new);

    let merged = fx.path("merged.db");
    let out = fx.run(&[
        "--base",
        base.to_str().unwrap(),
        "--ours",
        ours.to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
        "--into",
        merged.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "merge failed: {:?}", out);

    let ids = fx.list_ids(&merged);
    assert_eq!(ids.len(), 3, "expected 3 distinct tasks, got {ids:?}");
    // ours-only kept its id; theirs-only got renumbered to something new.
    assert!(ids.contains(&ours_new));
    let renumbered: Vec<i64> = ids
        .iter()
        .copied()
        .filter(|i| *i != ours_new && *i != 1)
        .collect();
    assert_eq!(renumbered.len(), 1);
    assert_ne!(renumbered[0], ours_new);
}

#[test]
fn same_field_conflict_keeps_ours_and_tags() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    let id = fx.add(&base, &["original title"]);

    let ours = fx.copy(&base, "ours.db");
    let theirs = fx.copy(&base, "theirs.db");

    fx.cmd(&ours)
        .args(["edit", &id.to_string(), "--title", "ours title"])
        .assert()
        .success();
    fx.cmd(&theirs)
        .args(["edit", &id.to_string(), "--title", "theirs title"])
        .assert()
        .success();

    let merged = fx.path("merged.db");
    let out = fx.run(&[
        "--base",
        base.to_str().unwrap(),
        "--ours",
        ours.to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
        "--into",
        merged.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("conflicts: 1"),
        "stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    let v = fx.show_json(&merged, id);
    assert_eq!(v["title"].as_str().unwrap(), "ours title");
    assert!(v["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t.as_str() == Some("merge-conflict")));
}

#[test]
fn status_rank_resolves_divergent_status_changes() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    let id = fx.add(&base, &["task"]);

    let ours = fx.copy(&base, "ours.db");
    let theirs = fx.copy(&base, "theirs.db");

    // ours: pending -> partial-ish (start then stop)
    fx.cmd(&ours)
        .args(["start", &id.to_string()])
        .assert()
        .success();
    fx.cmd(&ours)
        .args(["stop", &id.to_string()])
        .assert()
        .success();
    // theirs: pending -> done (higher rank)
    fx.cmd(&theirs)
        .args(["done", &id.to_string()])
        .assert()
        .success();

    let merged = fx.path("merged.db");
    let out = fx.run(&[
        "--base",
        base.to_str().unwrap(),
        "--ours",
        ours.to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
        "--into",
        merged.to_str().unwrap(),
    ]);
    assert!(out.status.success());

    let v = fx.show_json(&merged, id);
    assert_eq!(v["status"].as_str().unwrap(), "done");
    // no hard conflict — status divergence is auto-resolved by rank.
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("merge-conflict")
            || !v["tags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t.as_str() == Some("merge-conflict"))
    );
}

#[test]
fn deleted_in_one_side_modified_in_other_is_flagged_and_kept() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    let id = fx.add(&base, &["task"]);

    let ours = fx.copy(&base, "ours.db");
    let theirs = fx.copy(&base, "theirs.db");

    fx.cmd(&ours)
        .args(["rm", &id.to_string()])
        .assert()
        .success();
    fx.cmd(&theirs)
        .args(["edit", &id.to_string(), "--priority", "1"])
        .assert()
        .success();

    let merged = fx.path("merged.db");
    let out = fx.run(&[
        "--base",
        base.to_str().unwrap(),
        "--ours",
        ours.to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
        "--into",
        merged.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("conflicts: 1"));

    let ids = fx.list_ids(&merged);
    assert!(ids.contains(&id), "modified side should win over deletion");
}

#[test]
fn deleted_both_sides_drops_cleanly() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    let id = fx.add(&base, &["task"]);
    fx.add(&base, &["keep me"]);

    let ours = fx.copy(&base, "ours.db");
    let theirs = fx.copy(&base, "theirs.db");
    fx.cmd(&ours)
        .args(["rm", &id.to_string()])
        .assert()
        .success();
    fx.cmd(&theirs)
        .args(["rm", &id.to_string()])
        .assert()
        .success();

    let merged = fx.path("merged.db");
    let out = fx.run(&[
        "--base",
        base.to_str().unwrap(),
        "--ours",
        ours.to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
        "--into",
        merged.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("conflicts: 0"));
    assert!(!fx.list_ids(&merged).contains(&id));
}

#[test]
fn strict_mode_aborts_and_writes_nothing_on_conflict() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    let id = fx.add(&base, &["original title"]);

    let ours = fx.copy(&base, "ours.db");
    let theirs = fx.copy(&base, "theirs.db");
    fx.cmd(&ours)
        .args(["edit", &id.to_string(), "--title", "ours title"])
        .assert()
        .success();
    fx.cmd(&theirs)
        .args(["edit", &id.to_string(), "--title", "theirs title"])
        .assert()
        .success();

    let merged = fx.path("merged-strict.db");
    let out = fx.run(&[
        "--base",
        base.to_str().unwrap(),
        "--ours",
        ours.to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
        "--into",
        merged.to_str().unwrap(),
        "--strict",
    ]);
    assert!(!out.status.success());
    assert!(
        !merged.exists(),
        "strict mode must not write a partial result"
    );
}

#[test]
fn two_way_merge_without_base_renumbers_all_overlap() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    fx.add(&base, &["shared task"]);

    let ours = fx.copy(&base, "a.db");
    let theirs = fx.copy(&base, "b.db");
    fx.add(&ours, &["a new"]);
    fx.add(&theirs, &["b new"]);

    let merged = fx.path("merged.db");
    let out = fx.run(&[
        "--ours",
        ours.to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
        "--into",
        merged.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "merge failed: {:?}", out);

    // No --base means id 1 in `ours` and id 1 in `theirs` are treated as
    // unrelated tasks, so nothing is field-merged and everything unions in.
    let ids = fx.list_ids(&merged);
    assert_eq!(ids.len(), 4, "ids: {ids:?}");
}

#[test]
fn merge_into_defaults_to_overwriting_ours() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    fx.add(&base, &["shared task"]);

    let ours = fx.copy(&base, "ours.db");
    let theirs = fx.copy(&base, "theirs.db");
    fx.add(&theirs, &["theirs new"]);

    let out = fx.run(&[
        "--base",
        base.to_str().unwrap(),
        "--ours",
        ours.to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    assert_eq!(fx.list_ids(&ours).len(), 2);
}

#[test]
fn dep_union_that_would_cycle_drops_the_new_edge() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    let a = fx.add(&base, &["a"]);
    let b = fx.add(&base, &["b"]);

    let ours = fx.copy(&base, "ours.db");
    let theirs = fx.copy(&base, "theirs.db");

    // ours: a depends on b
    fx.cmd(&ours)
        .args(["edit", &a.to_string(), "--add-dep", &b.to_string()])
        .assert()
        .success();
    // theirs: b depends on a (would create a cycle once unioned with ours' edge)
    fx.cmd(&theirs)
        .args(["edit", &b.to_string(), "--add-dep", &a.to_string()])
        .assert()
        .success();

    let merged = fx.path("merged.db");
    let out = fx.run(&[
        "--base",
        base.to_str().unwrap(),
        "--ours",
        ours.to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
        "--into",
        merged.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "merge failed: {:?}", out);

    // Whichever edge was applied first, the union must not contain both
    // (that would be a cycle) — the merged db must still be usable.
    let va = fx.show_json(&merged, a);
    let vb = fx.show_json(&merged, b);
    let a_deps: Vec<i64> = va["depends_on"]
        .as_array()
        .map(|arr| arr.iter().map(|x| x.as_i64().unwrap()).collect())
        .unwrap_or_default();
    let b_deps: Vec<i64> = vb["depends_on"]
        .as_array()
        .map(|arr| arr.iter().map(|x| x.as_i64().unwrap()).collect())
        .unwrap_or_default();
    assert!(
        !(a_deps.contains(&b) && b_deps.contains(&a)),
        "merged dep graph must not contain a cycle: a_deps={a_deps:?} b_deps={b_deps:?}"
    );
}

#[test]
fn missing_ours_file_is_a_user_error() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    let theirs = fx.copy(&base, "theirs.db");

    let out = fx.run(&[
        "--ours",
        fx.path("nope.db").to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(predicate::str::contains("not found").eval(&String::from_utf8_lossy(&out.stderr)));
}

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

    /// Like `add`, but also returns the new task's uuid (needed once a
    /// display id can collide and `show <id>` alone can't disambiguate).
    fn add_with_uuid(&self, db: &Path, args: &[&str]) -> (i64, String) {
        let mut c = self.cmd(db);
        c.arg("add").arg("--json");
        c.args(args);
        let out = c.output().unwrap();
        assert!(out.status.success(), "add failed: {:?}", out);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        (
            v["id"].as_i64().unwrap(),
            v["task"]["uuid"].as_str().unwrap().to_string(),
        )
    }

    fn show_by_uuid(&self, db: &Path, uuid: &str) -> serde_json::Value {
        let out = self
            .cmd(db)
            .args(["show", uuid, "--json"])
            .output()
            .unwrap();
        assert!(out.status.success(), "show failed: {:?}", out);
        serde_json::from_slice(&out.stdout).unwrap()
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
fn new_tasks_on_both_sides_keep_their_own_duplicate_display_id() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    fx.add(&base, &["shared task"]);

    let ours = fx.copy(&base, "ours.db");
    let theirs = fx.copy(&base, "theirs.db");

    let (ours_new_id, ours_new_uuid) = fx.add_with_uuid(&ours, &["ours-only"]);
    let (theirs_new_id, theirs_new_uuid) = fx.add_with_uuid(&theirs, &["theirs-only"]);
    // Both allocated the same next display id independently.
    assert_eq!(ours_new_id, theirs_new_id);
    assert_ne!(ours_new_uuid, theirs_new_uuid);

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

    // A uuid can't collide, so both new tasks survive distinctly — with the
    // same duplicate display id, since there's nothing to renumber anymore.
    let ids = fx.list_ids(&merged);
    assert_eq!(ids.len(), 3, "expected 3 distinct tasks, got {ids:?}");
    assert_eq!(
        ids.iter().filter(|i| **i == ours_new_id).count(),
        2,
        "both new tasks should keep display id {ours_new_id}: {ids:?}"
    );

    // The duplicate id is now ambiguous — `show` must list both rather than
    // silently picking one, and each full uuid still resolves precisely.
    let show_out = fx
        .cmd(&merged)
        .args(["show", &ours_new_id.to_string()])
        .output()
        .unwrap();
    assert!(!show_out.status.success());
    let stderr = String::from_utf8_lossy(&show_out.stderr);
    assert!(stderr.contains("ambiguous"), "stderr: {stderr:?}");
    assert!(stderr.contains(&ours_new_uuid), "stderr: {stderr:?}");
    assert!(stderr.contains(&theirs_new_uuid), "stderr: {stderr:?}");

    assert_eq!(
        fx.show_by_uuid(&merged, &ours_new_uuid)["title"]
            .as_str()
            .unwrap(),
        "ours-only"
    );
    assert_eq!(
        fx.show_by_uuid(&merged, &theirs_new_uuid)["title"]
            .as_str()
            .unwrap(),
        "theirs-only"
    );
}

#[test]
fn related_link_added_by_one_side_stays_mutual_after_merge() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    let a = fx.add(&base, &["task a"]);
    let b = fx.add(&base, &["task b"]);

    let ours = fx.copy(&base, "ours.db");
    let theirs = fx.copy(&base, "theirs.db");

    fx.cmd(&ours)
        .args(["edit", &a.to_string(), "--add-related", &b.to_string()])
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

    let va = fx.show_json(&merged, a);
    let vb = fx.show_json(&merged, b);
    assert_eq!(
        va["related"].as_array().unwrap(),
        &[serde_json::json!(b)],
        "a: {va:?}"
    );
    assert_eq!(
        vb["related"].as_array().unwrap(),
        &[serde_json::json!(a)],
        "b: {vb:?}"
    );
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
fn two_way_merge_without_base_reconciles_shared_uuid_and_unions_new() {
    let fx = MergeFixture::new();
    let base = fx.init("base.db");
    fx.add(&base, &["shared task"]);

    // `a.db`/`b.db` are byte-copies of the same file, so the "shared task"
    // row has the identical uuid on both sides — a real cross-node
    // collision is astronomically unlikely, but a shared history without
    // --base (e.g. two clones of the same db) is exactly this shape.
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

    // The shared uuid reconciles to one row (not two — a uuid identifies a
    // single task even without --base); each side's new task unions in.
    let ids = fx.list_ids(&merged);
    assert_eq!(ids.len(), 3, "ids: {ids:?}");
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

/// Reproduces the CORRUPTION_LOG.md incident: `ours` is still on an old
/// schema (no uuid column) while `theirs` has already been migrated to the
/// current one. Merging must refuse outright rather than silently
/// migrating `ours` mid-merge — that migration mints fresh, uncorrelated
/// uuids for every pre-existing row, and the merge would then union by
/// uuid and duplicate the entire backlog instead of reconciling it.
#[test]
fn refuses_to_merge_across_mismatched_schema_versions() {
    let fx = MergeFixture::new();

    // `theirs`: a normal, fully-migrated db with one pre-existing task.
    let theirs = fx.init("theirs.db");
    fx.add(&theirs, &["shared task"]);

    // `ours`: hand-crafted at schema v4 (pre-uuid-as-primary-key), with a
    // row that's logically the *same* task but has no uuid yet.
    let ours = fx.path("ours.db");
    {
        let conn = rusqlite::Connection::open(&ours).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE tasks (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                title        TEXT NOT NULL,
                details      TEXT,
                status       TEXT NOT NULL CHECK(status IN ('pending','partial','in-progress','done','rejected')),
                priority     INTEGER NOT NULL DEFAULT 3 CHECK(priority BETWEEN 1 AND 5),
                is_gate      INTEGER NOT NULL DEFAULT 0 CHECK(is_gate IN (0,1)),
                created_at   TEXT NOT NULL,
                started_at   TEXT,
                completed_at TEXT
            );
            CREATE TABLE tags (
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                tag     TEXT NOT NULL,
                PRIMARY KEY (task_id, tag)
            );
            CREATE TABLE deps (
                task_id         INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                depends_on_id   INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                PRIMARY KEY (task_id, depends_on_id)
            );
            CREATE TABLE meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO tasks(id, title, details, status, priority, created_at)
                VALUES (1, 'shared task', NULL, 'pending', 3, '2026-01-01T00:00:00Z');
            INSERT INTO meta(key, value) VALUES ('schema_version', '4');
            "#,
        )
        .unwrap();
    }

    let out = fx.run(&[
        "--ours",
        ours.to_str().unwrap(),
        "--theirs",
        theirs.to_str().unwrap(),
    ]);
    assert!(
        !out.status.success(),
        "merge across mismatched schema versions must be refused"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("schema versions differ"),
        "stderr: {stderr:?}"
    );

    // Crucially, `ours` must be untouched — no auto-migration, no
    // duplication.
    let conn = rusqlite::Connection::open(&ours).unwrap();
    let version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version, "4", "ours must not have been auto-migrated");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "ours must not have gained duplicate rows");
}

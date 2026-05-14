mod common;

use common::Sandbox;
use predicates::prelude::*;

#[test]
fn priority_accepts_p_prefix() {
    let sb = Sandbox::new();
    let id = sb.add_with(&["p2-task", "--priority", "P2"]);
    let out = sb
        .cmd()
        .args(["show", &id.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["priority"].as_i64().unwrap(), 2);
}

#[test]
fn priority_accepts_lowercase_p_prefix() {
    let sb = Sandbox::new();
    let id = sb.add_with(&["p4-task", "--priority", "p4"]);
    let out = sb
        .cmd()
        .args(["show", &id.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["priority"].as_i64().unwrap(), 4);
}

#[test]
fn priority_rejects_out_of_range() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["add", "bad", "--priority", "P9"])
        .assert()
        .failure();
}

#[test]
fn list_ids_only_text_one_per_line() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    let b = sb.add("b");
    let out = sb.cmd().args(["list", "--ids-only"]).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    let ids: Vec<i64> = s.lines().map(|l| l.trim().parse().unwrap()).collect();
    assert_eq!(ids, vec![a, b]);
}

#[test]
fn list_ids_only_json_array() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    let _b = sb.add("b");
    let out = sb
        .cmd()
        .args(["list", "--ids-only", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<i64> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_i64().unwrap())
        .collect();
    assert!(ids.contains(&a));
}

#[test]
fn list_since_filters_by_created_at() {
    let sb = Sandbox::new();
    sb.add("old");
    let tomorrow = chrono::Utc::now().date_naive() + chrono::Duration::days(1);
    let out = sb
        .cmd()
        .args(["list", "--since", &tomorrow.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["tasks"].as_array().unwrap().len(), 0);
}

#[test]
fn export_completed_compact_by_default() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    sb.cmd().args(["done", &a.to_string()]).assert().success();
    let out = sb.cmd().args(["export-completed"]).output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    // Compact JSON has no leading "  " indentation lines.
    assert!(!s.contains("\n  "), "expected compact output, got:\n{s}");
}

#[test]
fn export_completed_pretty_flag_indents() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    sb.cmd().args(["done", &a.to_string()]).assert().success();
    let out = sb
        .cmd()
        .args(["export-completed", "--pretty"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\n  "), "expected pretty output, got:\n{s}");
}

#[test]
fn export_todo_terse_markdown_drops_dependencies_none_line() {
    let sb = Sandbox::new();
    sb.add("a");
    let out = sb
        .cmd()
        .args(["export-todo", "--format", "markdown"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        !s.contains("Dependencies: none"),
        "terse markdown must not render 'Dependencies: none', got:\n{s}"
    );
}

#[test]
fn export_todo_verbose_markdown_keeps_dependencies_none_line() {
    let sb = Sandbox::new();
    sb.add("a");
    let out = sb
        .cmd()
        .args(["export-todo", "--format", "markdown", "--verbose"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("# Dependencies: none"),
        "verbose markdown should keep '# Dependencies: none', got:\n{s}"
    );
}

#[test]
fn edit_append_details_seeds_when_empty() {
    let sb = Sandbox::new();
    let id = sb.add("task");
    sb.cmd()
        .args([
            "edit",
            &id.to_string(),
            "--append-details",
            "first note",
        ])
        .assert()
        .success();
    let out = sb
        .cmd()
        .args(["show", &id.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["details"].as_str().unwrap(), "first note");
}

#[test]
fn edit_append_details_concatenates_with_newline() {
    let sb = Sandbox::new();
    let id = sb.add_with(&["task", "--details", "base"]);
    sb.cmd()
        .args(["edit", &id.to_string(), "--append-details", "more"])
        .assert()
        .success();
    sb.cmd()
        .args(["edit", &id.to_string(), "--append-details", "and more"])
        .assert()
        .success();
    let out = sb
        .cmd()
        .args(["show", &id.to_string(), "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["details"].as_str().unwrap(), "base\nmore\nand more");
}

#[test]
fn edit_append_details_rejected_with_replace_or_clear() {
    let sb = Sandbox::new();
    let id = sb.add("task");
    sb.cmd()
        .args([
            "edit",
            &id.to_string(),
            "--details",
            "x",
            "--append-details",
            "y",
        ])
        .assert()
        .failure();
    sb.cmd()
        .args([
            "edit",
            &id.to_string(),
            "--clear-details",
            "--append-details",
            "y",
        ])
        .assert()
        .failure();
}

#[test]
fn show_default_suppresses_default_status_and_priority() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    let out = sb.cmd().args(["show", &a.to_string()]).output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(!s.contains("Status: pending"), "got:\n{s}");
    assert!(!s.contains("Priority: P3"), "got:\n{s}");
    // # prefix is also dropped
    assert!(s.contains("Title: a"), "got:\n{s}");
    assert!(!s.contains("# Title:"), "got:\n{s}");
}

#[test]
fn show_verbose_includes_default_status_and_priority() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    sb.cmd()
        .args(["show", &a.to_string(), "--verbose"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Status: pending"))
        .stdout(predicate::str::contains("Priority: P3"))
        .stdout(predicate::str::contains("Created:"));
}

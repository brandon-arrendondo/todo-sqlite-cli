mod common;

use common::Sandbox;

fn backdate(sb: &Sandbox, id: i64, column: &str, value: &str) {
    let conn = rusqlite::Connection::open(&sb.db).unwrap();
    conn.execute(
        &format!("UPDATE tasks SET {column} = ?1 WHERE id = ?2"),
        rusqlite::params![value, id],
    )
    .unwrap();
}

#[test]
fn cfd_reconstructs_counts_across_transitions() {
    let sb = Sandbox::new();
    let a = sb.add("stays in backlog");
    let b = sb.add("starts mid-range");
    let c = sb.add("completes mid-range");

    backdate(&sb, a, "created_at", "2026-08-01T00:00:00Z");
    backdate(&sb, b, "created_at", "2026-08-01T00:00:00Z");
    backdate(&sb, b, "started_at", "2026-08-06T00:00:00Z");
    backdate(&sb, c, "created_at", "2026-08-01T00:00:00Z");
    backdate(&sb, c, "started_at", "2026-08-02T00:00:00Z");
    backdate(&sb, c, "completed_at", "2026-08-10T00:00:00Z");
    backdate(&sb, c, "status", "done");

    let out = sb
        .cmd()
        .args([
            "cfd",
            "--since",
            "2026-08-01",
            "--until",
            "2026-08-12",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let buckets = v["buckets"].as_array().unwrap();

    let find = |date: &str| {
        buckets
            .iter()
            .find(|b| b["date"] == date)
            .unwrap_or_else(|| panic!("missing bucket {date}"))
    };

    let day1 = find("2026-08-01");
    assert_eq!(day1["backlog"], 3);
    assert_eq!(day1["in_progress"], 0);
    assert_eq!(day1["done"], 0);

    let day6 = find("2026-08-06");
    assert_eq!(day6["backlog"], 1);
    assert_eq!(day6["in_progress"], 2);
    assert_eq!(day6["done"], 0);

    let day12 = find("2026-08-12");
    assert_eq!(day12["backlog"], 1);
    assert_eq!(day12["in_progress"], 1);
    assert_eq!(day12["done"], 1);
}

#[test]
fn cfd_bucket_dates_always_include_until_boundary() {
    let sb = Sandbox::new();
    sb.add("a");

    let out = sb
        .cmd()
        .args([
            "cfd",
            "--since",
            "2026-08-01",
            "--until",
            "2026-08-10",
            "--bucket",
            "week",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let dates: Vec<&str> = v["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["date"].as_str().unwrap())
        .collect();
    assert_eq!(dates.first(), Some(&"2026-08-01"));
    assert_eq!(dates.last(), Some(&"2026-08-10"));
}

#[test]
fn cfd_reverted_task_with_cleared_started_at_counts_as_backlog() {
    let sb = Sandbox::new();
    let a = sb.add_with(&["reverted task", "--start"]);
    sb.cmd().args(["revert", &a.to_string()]).assert().success();
    backdate(&sb, a, "created_at", "2026-08-01T00:00:00Z");

    let out = sb
        .cmd()
        .args([
            "cfd",
            "--since",
            "2026-08-01",
            "--until",
            "2026-08-01",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let bucket = &v["buckets"][0];
    assert_eq!(bucket["backlog"], 1);
    assert_eq!(bucket["in_progress"], 0);
}

#[test]
fn cfd_rejects_since_after_until() {
    let sb = Sandbox::new();
    sb.add("a");

    sb.cmd()
        .args(["cfd", "--since", "2026-08-10", "--until", "2026-08-01"])
        .assert()
        .failure();
}

#[test]
fn cfd_rejects_invalid_bucket() {
    let sb = Sandbox::new();
    sb.add("a");

    sb.cmd()
        .args(["cfd", "--bucket", "month"])
        .assert()
        .failure();
}

#[test]
fn cfd_csv_has_header_and_row_per_bucket() {
    let sb = Sandbox::new();
    sb.add("a");

    let out = sb
        .cmd()
        .args([
            "cfd",
            "--since",
            "2026-08-01",
            "--until",
            "2026-08-03",
            "--format",
            "csv",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines[0], "date,backlog,in_progress,done,rejected");
    assert_eq!(lines.len(), 4); // header + 3 daily buckets
}

#[test]
fn cfd_default_since_is_earliest_created_at() {
    let sb = Sandbox::new();
    let a = sb.add("a");
    backdate(&sb, a, "created_at", "2026-08-01T00:00:00Z");

    let out = sb
        .cmd()
        .args(["cfd", "--until", "2026-08-01", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let buckets = v["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0]["date"], "2026-08-01");
}

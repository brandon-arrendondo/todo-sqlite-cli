mod common;

use common::Sandbox;
use predicates::prelude::*;

#[test]
fn add_location_and_related_show_up_on_show() {
    let sb = Sandbox::new();
    let a = sb.add("task a");
    let b = sb.add_with(&[
        "task b",
        "--location",
        "node-7",
        "--related",
        &a.to_string(),
    ]);

    sb.cmd()
        .args(["show", &b.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Location: node-7"))
        .stdout(predicate::str::contains(format!("Related: {a}")));

    // The link is mutual: `a` sees `b` back without any edit on `a` itself.
    sb.cmd()
        .args(["show", &a.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("Related: {b}")));
}

#[test]
fn related_does_not_appear_in_list_output() {
    let sb = Sandbox::new();
    let a = sb.add("task a");
    sb.add_with(&["task b", "--related", &a.to_string()]);

    let out = sb.cmd().args(["list", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    for t in v["tasks"].as_array().unwrap() {
        assert!(
            t.get("related").is_none(),
            "list output must not carry a related key: {t:?}"
        );
    }

    sb.cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Related").not());
}

#[test]
fn location_shows_as_at_suffix_in_list() {
    let sb = Sandbox::new();
    sb.add_with(&["fix filter", "--location", "warehouse-3"]);
    sb.add("plain task");

    sb.cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fix filter @warehouse-3"))
        .stdout(
            predicate::str::contains("plain task")
                .and(predicate::str::contains("plain task @").not()),
        );
}

#[test]
fn edit_add_related_and_rm_related_are_mutual() {
    let sb = Sandbox::new();
    let a = sb.add("task a");
    let b = sb.add("task b");

    sb.cmd()
        .args(["edit", &a.to_string(), "--add-related", &b.to_string()])
        .assert()
        .success();
    sb.cmd()
        .args(["show", &b.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("Related: {a}")));

    sb.cmd()
        .args(["edit", &a.to_string(), "--rm-related", &b.to_string()])
        .assert()
        .success();
    sb.cmd()
        .args(["show", &a.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Related:").not());
    sb.cmd()
        .args(["show", &b.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Related:").not());
}

#[test]
fn edit_add_related_rejects_self_link() {
    let sb = Sandbox::new();
    let a = sb.add("task a");

    let out = sb
        .cmd()
        .args(["edit", &a.to_string(), "--add-related", &a.to_string()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("related to itself"), "stderr: {stderr:?}");
}

#[test]
fn edit_location_and_clear_location() {
    let sb = Sandbox::new();
    let a = sb.add("task a");

    sb.cmd()
        .args(["edit", &a.to_string(), "--location", "site-9"])
        .assert()
        .success();
    sb.cmd()
        .args(["show", &a.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Location: site-9"));

    sb.cmd()
        .args(["edit", &a.to_string(), "--clear-location"])
        .assert()
        .success();
    sb.cmd()
        .args(["show", &a.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Location:").not());
}

#[test]
fn edit_location_and_clear_location_together_rejected() {
    let sb = Sandbox::new();
    let a = sb.add("task a");

    sb.cmd()
        .args([
            "edit",
            &a.to_string(),
            "--location",
            "site-9",
            "--clear-location",
        ])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn location_present_in_json_show_and_list() {
    let sb = Sandbox::new();
    let a = sb.add_with(&["task a", "--location", "node-7"]);
    let b = sb.add("task b");

    let show_v: serde_json::Value = serde_json::from_slice(
        &sb.cmd()
            .args(["show", &a.to_string(), "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(show_v["location"].as_str().unwrap(), "node-7");

    let list_out = sb.cmd().args(["list", "--json"]).output().unwrap();
    let list_v: serde_json::Value = serde_json::from_slice(&list_out.stdout).unwrap();
    let tasks = list_v["tasks"].as_array().unwrap();
    let by_id = |id: i64| {
        tasks
            .iter()
            .find(|t| t["id"].as_i64().unwrap() == id)
            .unwrap()
    };
    assert_eq!(by_id(a)["location"].as_str().unwrap(), "node-7");
    assert!(by_id(b)["location"].is_null());
}

mod common;

use common::Sandbox;
use predicates::prelude::*;

// Free-text arguments (title, --details, --append-details) must accept a
// value starting with `-`/`--` without caller-side quoting tricks
// (`-- "..."` or `--flag=...`). Values like "--- new section" and
// "--detect-relevance: ..." are ordinary content, and the MCP server builds
// these invocations mechanically with no chance to reshape them.

#[test]
fn add_title_may_start_with_double_dash() {
    let sb = Sandbox::new();
    let id = sb.add("--detect-relevance: markers match inside comments");
    sb.cmd()
        .args(["show", &id.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Title: --detect-relevance: markers match inside comments",
        ));
}

#[test]
fn add_title_with_leading_dash_coexists_with_real_flags() {
    let sb = Sandbox::new();
    // Flags before and after the hyphen-leading positional must still parse as flags.
    let id = sb.add_with(&["--priority", "4", "-x dash", "--tag", "t"]);
    sb.cmd()
        .args(["show", &id.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Title: -x dash"))
        .stdout(predicate::str::contains("P4"))
        .stdout(predicate::str::contains("t"));
}

#[test]
fn add_details_may_start_with_double_dash() {
    let sb = Sandbox::new();
    let id = sb.add_with(&["plain", "--details", "--- leading separator"]);
    sb.cmd()
        .args(["show", &id.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("--- leading separator"));
}

#[test]
fn edit_append_details_may_start_with_double_dash() {
    let sb = Sandbox::new();
    let id = sb.add("plain");
    sb.cmd()
        .args([
            "edit",
            &id.to_string(),
            "--append-details",
            "--- new section below",
        ])
        .assert()
        .success();
    sb.cmd()
        .args(["show", &id.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("--- new section below"));
}

#[test]
fn edit_details_and_title_may_start_with_dash() {
    let sb = Sandbox::new();
    let id = sb.add("plain");
    sb.cmd()
        .args([
            "edit",
            &id.to_string(),
            "--title",
            "-- retitled",
            "--details",
            "-d",
        ])
        .assert()
        .success();
    sb.cmd()
        .args(["show", &id.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Title: -- retitled"))
        .stdout(predicate::str::contains("Details:\n-d"));
}

#[test]
fn unknown_flag_after_title_is_still_an_error() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["add", "ok", "--nonsense"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument '--nonsense'"));
}

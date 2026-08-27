use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{system, user, CliResult};
use crate::resolve;

const DRIVER_NAME: &str = "todo-sqlite-cli";

/// One-time setup: registers this binary as a git merge driver for the
/// resolved database file, so `git merge`/`pull`/`rebase` calls it
/// automatically instead of leaving a binary conflict. Writes a line to
/// `.gitattributes` (a tracked file — this affects every collaborator) and
/// sets `merge.todo-sqlite-cli.*` in the repo-local git config.
pub fn run(db_flag: Option<&Path>, dry_run: bool) -> CliResult<()> {
    let repo_root = git_toplevel()?;
    let db_path = resolve::resolve_db_path(db_flag)?;
    let db_abs = std::fs::canonicalize(&db_path).unwrap_or(db_path.clone());
    let rel = db_abs.strip_prefix(&repo_root).map_err(|_| {
        user(format!(
            "database {} is not inside the git repo at {}",
            db_abs.display(),
            repo_root.display()
        ))
    })?;
    let pattern = rel.display().to_string();

    let attrs_path = repo_root.join(".gitattributes");
    let attrs_line = format!("{pattern} merge={DRIVER_NAME}");
    let already_present = std::fs::read_to_string(&attrs_path)
        .map(|content| content.lines().any(|l| l.trim() == attrs_line))
        .unwrap_or(false);

    let driver_cmd = format!("{DRIVER_NAME} git-merge-driver %O %A %B");

    if dry_run {
        println!("would append to {}:", attrs_path.display());
        println!("  {attrs_line}");
        println!("would run:");
        println!("  git config merge.{DRIVER_NAME}.name \"todo-sqlite-cli 3-way merge driver\"");
        println!("  git config merge.{DRIVER_NAME}.driver \"{driver_cmd}\"");
        return Ok(());
    }

    if !already_present {
        let mut content = std::fs::read_to_string(&attrs_path).unwrap_or_default();
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&attrs_line);
        content.push('\n');
        std::fs::write(&attrs_path, content)
            .map_err(|e| system(format!("cannot write {}: {e}", attrs_path.display())))?;
        println!("added to {}: {attrs_line}", attrs_path.display());
    } else {
        println!("{} already has: {attrs_line}", attrs_path.display());
    }

    run_git_config(
        &repo_root,
        &format!("merge.{DRIVER_NAME}.name"),
        "todo-sqlite-cli 3-way merge driver",
    )?;
    run_git_config(
        &repo_root,
        &format!("merge.{DRIVER_NAME}.driver"),
        &driver_cmd,
    )?;
    println!("set git config merge.{DRIVER_NAME}.name / .driver (repo-local, .git/config)");
    println!(
        "\nDone. Commit {} to share this with collaborators; the git config \
         itself is local-only, so anyone merging the file also needs to \
         run `todo-sqlite-cli install-merge-driver` once.",
        attrs_path.display()
    );
    Ok(())
}

fn git_toplevel() -> CliResult<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| {
            user(format!(
                "cannot run git (is it installed and on PATH?): {e}"
            ))
        })?;
    if !out.status.success() {
        return Err(user(
            "not inside a git repository (git rev-parse --show-toplevel failed)",
        ));
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(PathBuf::from(s))
}

fn run_git_config(repo_root: &Path, key: &str, value: &str) -> CliResult<()> {
    let status = Command::new("git")
        .current_dir(repo_root)
        .args(["config", key, value])
        .status()
        .map_err(|e| system(format!("cannot run git config: {e}")))?;
    if !status.success() {
        return Err(system(format!("git config {key} failed")));
    }
    Ok(())
}

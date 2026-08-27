mod cli;
mod commands;
mod db;
mod error;
mod format;
mod merge;
mod resolve;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::error::CliResult;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

fn dispatch(cli: Cli) -> CliResult<()> {
    let db_flag = cli.db.as_deref();
    let json = cli.json;

    match cli.command {
        Command::Init { marker_dir } => commands::init::run(db_flag, marker_dir.as_deref(), json),

        Command::Merge {
            base,
            ours,
            theirs,
            into,
            strict,
        } => commands::merge::run(
            json,
            base.as_deref(),
            &ours,
            &theirs,
            into.as_deref(),
            strict,
        ),

        Command::GitMergeDriver { base, ours, theirs } => {
            commands::git_merge_driver::run(&base, &ours, &theirs)
        }

        Command::InstallMergeDriver { dry_run } => {
            commands::install_merge_driver::run(db_flag, dry_run)
        }

        other => {
            let db_path = resolve::resolve_db_path(db_flag)?;
            run_command(other, &db_path, json)
        }
    }
}

fn run_command(cmd: Command, db_path: &std::path::Path, json: bool) -> CliResult<()> {
    match cmd {
        Command::Init { .. } => unreachable!("Init handled upstream"),
        Command::Merge { .. } => unreachable!("Merge handled upstream"),
        Command::GitMergeDriver { .. } => unreachable!("GitMergeDriver handled upstream"),
        Command::InstallMergeDriver { .. } => unreachable!("InstallMergeDriver handled upstream"),
        Command::Doctor => commands::doctor::run(db_path, json),
        Command::Add {
            title,
            details,
            tags,
            priority,
            depends_on,
            start,
            gate,
        } => commands::add::run(
            db_path,
            json,
            &title,
            details.as_deref(),
            &tags,
            priority,
            &depends_on,
            start,
            gate,
        ),
        Command::List {
            status,
            tags,
            limit,
            format,
            since,
            ids_only,
            verbose,
            kind,
            unblocked,
        } => commands::list::run(
            db_path,
            json,
            &status,
            &tags,
            limit,
            &format,
            since.as_deref(),
            ids_only,
            verbose,
            &kind,
            unblocked,
        ),
        Command::Next => commands::next::run(db_path, json),
        Command::Start { id, force } => commands::start::run(db_path, json, &id, force),
        Command::Stop { id } => commands::stop::run(db_path, json, &id),
        Command::Revert { id } => commands::revert::run(db_path, json, &id),
        Command::Done { id, rejected } => commands::done::run(db_path, json, &id, rejected),
        Command::Show {
            id,
            verbose,
            format,
        } => commands::show::run(db_path, json, &id, verbose, &format),
        Command::Edit {
            id,
            title,
            details,
            append_details,
            clear_details,
            priority,
            add_tag,
            rm_tag,
            add_dep,
            rm_dep,
            gate,
            no_gate,
        } => commands::edit::run(
            db_path,
            json,
            &id,
            title.as_deref(),
            details.as_deref(),
            append_details.as_deref(),
            clear_details,
            priority,
            &add_tag,
            &rm_tag,
            &add_dep,
            &rm_dep,
            gate,
            no_gate,
        ),
        Command::Renumber { id, new_id, force } => {
            commands::renumber::run(db_path, json, &id, new_id, force)
        }
        Command::Rm { id } => commands::rm::run(db_path, json, &id),
        Command::ExportCompleted {
            since,
            until,
            pretty,
            format,
        } => commands::export_completed::run(
            db_path,
            json,
            since.as_deref(),
            until.as_deref(),
            pretty,
            &format,
        ),
        Command::ExportTodo { format, verbose } => {
            commands::export_todo::run(db_path, json, &format, verbose)
        }
        Command::Aging { stale_days, tags } => {
            commands::aging::run(db_path, json, stale_days, &tags)
        }
        Command::Cfd {
            since,
            until,
            bucket,
            format,
        } => commands::cfd::run(
            db_path,
            json,
            since.as_deref(),
            until.as_deref(),
            &bucket,
            &format,
        ),
    }
}

mod cli;
mod commands;
mod db;
mod error;
mod format;
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

        other => {
            let db_path = resolve::resolve_db_path(db_flag)?;
            run_command(other, &db_path, json)
        }
    }
}

fn run_command(cmd: Command, db_path: &std::path::Path, json: bool) -> CliResult<()> {
    match cmd {
        Command::Init { .. } => unreachable!("Init handled upstream"),
        Command::Add {
            title,
            details,
            tags,
            priority,
            depends_on,
            start,
        } => commands::add::run(
            db_path,
            json,
            &title,
            details.as_deref(),
            &tags,
            priority,
            &depends_on,
            start,
        ),
        Command::List {
            status,
            tags,
            limit,
            format,
        } => commands::list::run(db_path, json, &status, &tags, limit, &format),
        Command::Next => commands::next::run(db_path, json),
        Command::Start { id, force } => commands::start::run(db_path, json, id, force),
        Command::Stop { id } => commands::stop::run(db_path, json, id),
        Command::Revert { id } => commands::revert::run(db_path, json, id),
        Command::Done { id } => commands::done::run(db_path, json, id),
        Command::Show { id } => commands::show::run(db_path, json, id),
        Command::Edit {
            id,
            title,
            details,
            clear_details,
            priority,
            add_tag,
            rm_tag,
            add_dep,
            rm_dep,
        } => commands::edit::run(
            db_path,
            json,
            id,
            title.as_deref(),
            details.as_deref(),
            clear_details,
            priority,
            &add_tag,
            &rm_tag,
            &add_dep,
            &rm_dep,
        ),
        Command::Rm { id } => commands::rm::run(db_path, json, id),
        Command::ExportCompleted { since, until } => {
            commands::export_completed::run(db_path, json, since.as_deref(), until.as_deref())
        }
        Command::ExportTodo { format } => commands::export_todo::run(db_path, json, &format),
    }
}

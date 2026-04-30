use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "todo-sqlite-cli",
    version,
    about = "Per-project TODO list CLI backed by SQLite"
)]
pub struct Cli {
    /// Path to the SQLite database (overrides TODO_SQLITE_CLI_DB and .todo-sqlite-cli marker).
    #[arg(long, global = true, value_name = "PATH")]
    pub db: Option<PathBuf>,

    /// Emit machine-readable JSON output.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialize a new database. Writes .todo-sqlite-cli marker in cwd when --db is not given.
    Init {
        /// Directory in which to write the marker (defaults to cwd). Ignored when --db is passed.
        #[arg(long, value_name = "PATH")]
        marker_dir: Option<PathBuf>,
    },

    /// Add a new task. Prints the new ID.
    Add {
        title: String,
        #[arg(long)]
        details: Option<String>,
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
        #[arg(long, value_parser = clap::value_parser!(i64).range(1..=5), default_value_t = 3)]
        priority: i64,
        #[arg(long = "depends-on", value_name = "ID")]
        depends_on: Vec<i64>,
        /// Immediately move to in-progress after adding.
        #[arg(long)]
        start: bool,
    },

    /// List tasks. Default shows active work (in-progress + pending), highest priority first.
    List {
        /// pending | in-progress | done | active | all
        #[arg(long, default_value = "active")]
        status: String,
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
        #[arg(long)]
        limit: Option<i64>,
    },

    /// Print the single task to work on next (in-progress beats pending).
    Next,

    /// Move a task to in-progress.
    Start {
        id: i64,
        /// Allow >1 in-progress task and ignore unmet dependencies.
        #[arg(long)]
        force: bool,
    },

    /// Move an in-progress task back to pending. Preserves started_at.
    Stop { id: i64 },

    /// Move a task back to pending and clear started_at.
    Revert { id: i64 },

    /// Mark a task done. Idempotent.
    Done { id: i64 },

    /// Show full details for a task.
    Show { id: i64 },

    /// Edit an existing task.
    Edit {
        id: i64,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        details: Option<String>,
        #[arg(long)]
        clear_details: bool,
        #[arg(long, value_parser = clap::value_parser!(i64).range(1..=5))]
        priority: Option<i64>,
        #[arg(long = "add-tag")]
        add_tag: Vec<String>,
        #[arg(long = "rm-tag")]
        rm_tag: Vec<String>,
        #[arg(long = "add-dep", value_name = "ID")]
        add_dep: Vec<i64>,
        #[arg(long = "rm-dep", value_name = "ID")]
        rm_dep: Vec<i64>,
    },

    /// Delete a task.
    Rm { id: i64 },

    /// Export completed tasks as JSON, grouped by completion date (desc).
    ExportCompleted {
        /// Inclusive lower bound on completed_at (YYYY-MM-DD or RFC3339).
        #[arg(long)]
        since: Option<String>,
        /// Exclusive upper bound on completed_at (YYYY-MM-DD or RFC3339).
        #[arg(long)]
        until: Option<String>,
    },

    /// Export active + pending tasks.
    ExportTodo {
        /// json | markdown
        #[arg(long, default_value = "json")]
        format: String,
    },
}

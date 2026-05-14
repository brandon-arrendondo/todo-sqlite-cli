use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Parse a priority value: accepts `1`..`5`, `P1`..`P5`, or `p1`..`p5`.
fn parse_priority(s: &str) -> Result<i64, String> {
    let trimmed = s.trim();
    let digits = trimmed.strip_prefix(['P', 'p']).unwrap_or(trimmed);
    let n: i64 = digits
        .parse()
        .map_err(|_| format!("invalid priority '{s}' (expected 1-5 or P1-P5)"))?;
    if !(1..=5).contains(&n) {
        return Err(format!("priority must be between 1 and 5 (got {n})"));
    }
    Ok(n)
}

const LONG_ABOUT: &str = "\
Per-project TODO list backed by SQLite, designed for coding agents (Claude
Code and friends). Plain CLI — no MCP, no daemon, no TTY.

Output is compact-by-default to keep token use down for AI agents. Pass
--verbose (on `show`, `export-todo --format markdown`) or --pretty (on
`export-completed`) when a human is reading.

Database resolution (first match wins):
  1. --db PATH flag
  2. TODO_SQLITE_CLI_DB environment variable
  3. Walk up from cwd looking for a `.todo-sqlite-cli` marker file
     (first line = DB path; relative paths resolve against the marker dir).
  4. Otherwise exit 1.

Exit codes: 0 success, 1 user error, 2 system error. Every command supports
--json and --db.

For agent integration patterns (token-frugal flags, the start/partial/done
flow, non-obvious invariants), see examples/CLAUDE.md.snippet in the source
repo. Full reference: `man todo-sqlite-cli`.";

#[derive(Parser, Debug)]
#[command(
    name = "todo-sqlite-cli",
    version,
    about = "Per-project TODO list CLI backed by SQLite, designed for coding agents",
    long_about = LONG_ABOUT,
)]
pub struct Cli {
    /// Path to the SQLite database. Overrides $TODO_SQLITE_CLI_DB and the .todo-sqlite-cli marker.
    #[arg(long, global = true, value_name = "PATH")]
    pub db: Option<PathBuf>,

    /// Emit machine-readable JSON output. Supported on every command.
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

    /// Add a new task. Prints the new ID on stdout.
    Add {
        /// Task title (short summary; required).
        title: String,
        /// Longer free-form description.
        #[arg(long)]
        details: Option<String>,
        /// Attach a tag. Repeatable: --tag foo --tag bar.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
        /// Priority: `1`..`5` or `P1`..`P5` (1 = highest, 3 = default).
        #[arg(long, value_parser = parse_priority, default_value = "3")]
        priority: i64,
        /// Mark this task as blocked by another task ID. Repeatable.
        #[arg(long = "depends-on", value_name = "ID")]
        depends_on: Vec<i64>,
        /// Immediately move the new task to in-progress (auto-pauses any prior in-progress task).
        #[arg(long)]
        start: bool,
    },

    /// List tasks. Default shows active work (in-progress + partial + pending), in-progress first then partial then pending; within each, by priority.
    List {
        /// Filter by status: pending | partial | in-progress | done | active | all. `active` = in-progress + partial + pending.
        #[arg(long, default_value = "active")]
        status: String,
        /// Filter by tag. Repeatable; multiple tags AND together.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
        /// Cap the number of rows returned.
        #[arg(long)]
        limit: Option<i64>,
        /// Output format: table | json | markdown.
        #[arg(long, default_value = "table")]
        format: String,
        /// Only include tasks with created_at >= SINCE (YYYY-MM-DD or RFC3339). For incremental re-reads between agent turns.
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        /// Print only IDs (one per line; JSON array under --json). Cheapest way to detect change between turns.
        #[arg(long = "ids-only")]
        ids_only: bool,
        /// Use heading-per-field markdown when --format markdown (default is terse).
        #[arg(long)]
        verbose: bool,
    },

    /// Print the single task to work on next. Order: oldest in-progress, then oldest unblocked partial, then highest-priority unblocked pending. Skips tasks with unmet deps.
    Next,

    /// Move a task to in-progress. Auto-pauses any prior in-progress task to `partial` (preserves its started_at).
    Start {
        /// Task ID to start.
        id: i64,
        /// Allow more than one in-progress task at a time and ignore unmet dependencies.
        #[arg(long)]
        force: bool,
    },

    /// Move an in-progress task to `partial`. Preserves started_at so it can be resumed via `start`.
    Stop {
        /// Task ID to pause.
        id: i64,
    },

    /// Move a task back to pending and clear started_at. Discards a start that turned out to be wrong.
    Revert {
        /// Task ID to revert.
        id: i64,
    },

    /// Mark a task done. Idempotent — calling it on an already-done task does not rewrite completed_at and exits 0.
    Done {
        /// Task ID to mark done.
        id: i64,
    },

    /// Show task details. Terse-by-default: fields holding default values (status=pending, priority=P3) are omitted.
    Show {
        /// Task ID to show.
        id: i64,
        /// Print all fields, including default values (status=pending, priority=P3) and created_at.
        #[arg(long)]
        verbose: bool,
    },

    /// Edit an existing task. Provide one or more of the flags below.
    Edit {
        /// Task ID to edit.
        id: i64,
        /// New title.
        #[arg(long)]
        title: Option<String>,
        /// New details body (replaces any existing details).
        #[arg(long)]
        details: Option<String>,
        /// Clear the details field.
        #[arg(long)]
        clear_details: bool,
        /// New priority: `1`..`5` or `P1`..`P5`.
        #[arg(long, value_parser = parse_priority)]
        priority: Option<i64>,
        /// Attach a tag. Repeatable.
        #[arg(long = "add-tag", value_name = "TAG")]
        add_tag: Vec<String>,
        /// Detach a tag. Repeatable. No-op if the tag is not attached.
        #[arg(long = "rm-tag", value_name = "TAG")]
        rm_tag: Vec<String>,
        /// Add a dependency edge (this task is blocked by ID). Repeatable; rejects cycles.
        #[arg(long = "add-dep", value_name = "ID")]
        add_dep: Vec<i64>,
        /// Remove a dependency edge. Repeatable.
        #[arg(long = "rm-dep", value_name = "ID")]
        rm_dep: Vec<i64>,
    },

    /// Delete a task. Cascades to associated tags and dependency edges. IDs are never reused.
    Rm {
        /// Task ID to delete.
        id: i64,
    },

    /// Export completed tasks as JSON, grouped by completion date (descending). Compact by default.
    ExportCompleted {
        /// Inclusive lower bound on completed_at (YYYY-MM-DD or RFC3339).
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        /// Exclusive upper bound on completed_at (YYYY-MM-DD or RFC3339).
        #[arg(long, value_name = "DATE")]
        until: Option<String>,
        /// Pretty-print the JSON output (multi-line, indented). Default is compact.
        #[arg(long)]
        pretty: bool,
    },

    /// Export in-progress + partial + pending tasks.
    ExportTodo {
        /// Output format: json | markdown. Markdown is terse by default.
        #[arg(long, default_value = "json")]
        format: String,
        /// Use heading-per-field markdown when --format markdown (default is terse).
        #[arg(long)]
        verbose: bool,
    },
}

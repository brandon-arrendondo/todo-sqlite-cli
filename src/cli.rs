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
    /// On `list` / `export-todo` the array is wrapped: `{"tasks": [...]}`.
    /// On `export-completed` it is wrapped and grouped: `{"completed": [{"date": "...", "tasks": [...]}, ...]}`.
    /// Single-task commands (`next`, `add`, `start`, `stop`, `revert`, `done`, `show`, `edit`) emit a bare task object.
    /// For streaming-friendly one-object-per-line output on list/export commands, prefer `--format ndjson`.
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
        /// Mark this task as blocked by another task ID (or full UUID). Repeatable.
        #[arg(long = "depends-on", value_name = "ID")]
        depends_on: Vec<String>,
        /// Immediately move the new task to in-progress (auto-pauses any prior in-progress task).
        #[arg(long)]
        start: bool,
        /// Mark this task as a gate: a checkpoint on a condition becoming true (described in --details), not work to be done. Skipped by `next`; never flagged stale by `aging`.
        #[arg(long)]
        gate: bool,
    },

    /// List tasks. Default shows active work (in-progress + partial + pending), in-progress first then partial then pending; within each, by priority.
    List {
        /// Filter by status: pending | partial | in-progress | done | rejected | active | all. `active` = in-progress + partial + pending.
        #[arg(long, default_value = "active")]
        status: String,
        /// Filter by tag. Repeatable; multiple tags AND together.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
        /// Cap the number of rows returned.
        #[arg(long)]
        limit: Option<i64>,
        /// Output format: table | json | ndjson | markdown.
        /// `json` wraps tasks in `{"tasks": [...]}`; `ndjson` emits one task object per line (no wrapper) for streaming/grep/jq pipelines.
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
        /// Filter by kind: gate | task | all. `gate` gives a readiness dashboard of open checkpoints.
        #[arg(long, default_value = "all")]
        kind: String,
        /// Only show tasks with no unmet dependencies.
        #[arg(long)]
        unblocked: bool,
    },

    /// Print the single task to work on next. Order: oldest in-progress, then oldest unblocked partial, then highest-priority unblocked pending. Skips tasks with unmet deps.
    Next,

    /// Move a task to in-progress. Auto-pauses any prior in-progress task to `partial` (preserves its started_at).
    Start {
        /// Task ID (or full UUID) to start.
        id: String,
        /// Allow more than one in-progress task at a time and ignore unmet dependencies.
        #[arg(long)]
        force: bool,
    },

    /// Move an in-progress task to `partial`. Preserves started_at so it can be resumed via `start`.
    Stop {
        /// Task ID (or full UUID) to pause.
        id: String,
    },

    /// Move a task back to pending and clear started_at. Discards a start that turned out to be wrong.
    Revert {
        /// Task ID (or full UUID) to revert.
        id: String,
    },

    /// Mark a task done. Idempotent — calling it on a task already in the target status does not rewrite completed_at and exits 0.
    Done {
        /// Task ID (or full UUID) to mark done.
        id: String,
        /// Close the task as `rejected` (declined / won't-do) instead of `done`. Sets completed_at but does NOT unblock dependents.
        #[arg(long)]
        rejected: bool,
    },

    /// Show task details. Terse-by-default: fields holding default values (status=pending, priority=P3) are omitted.
    Show {
        /// Task ID (or full UUID) to show. Display ids can collide across a
        /// merge; an ambiguous id lists every match with its full UUID.
        id: String,
        /// Print all fields, including default values (status=pending, priority=P3) and created_at.
        #[arg(long)]
        verbose: bool,
        /// Output format: text | json | ndjson | markdown.
        /// `json` emits a bare task object; `ndjson` emits the same object as a single JSON line (consistent with list --format ndjson for scripting).
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Edit an existing task. Provide one or more of the flags below.
    Edit {
        /// Task ID (or full UUID) to edit.
        id: String,
        /// New title.
        #[arg(long)]
        title: Option<String>,
        /// Append text to the existing details, separated by a newline. Use this for incremental progress notes — it preserves prior context.
        #[arg(long, value_name = "TEXT")]
        append_details: Option<String>,
        /// REPLACES the entire details body, discarding whatever was there before. For a progress note, use --append-details instead.
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
        /// Add a dependency edge (this task is blocked by ID or full UUID). Repeatable; rejects cycles.
        #[arg(long = "add-dep", value_name = "ID")]
        add_dep: Vec<String>,
        /// Remove a dependency edge (ID or full UUID). Repeatable.
        #[arg(long = "rm-dep", value_name = "ID")]
        rm_dep: Vec<String>,
        /// Mark this task as a gate (see `add --gate`). Mutually exclusive with `--no-gate`.
        #[arg(long, conflicts_with = "no_gate")]
        gate: bool,
        /// Unmark this task as a gate, demoting it back to a regular task.
        #[arg(long)]
        no_gate: bool,
    },

    /// Reassign a task's display id — for resolving a duplicate id left by a
    /// merge (see `doctor`). Identity (the uuid) is unchanged, and
    /// dependency edges are stored by uuid internally, so dependents keep
    /// resolving correctly.
    Renumber {
        /// Task ID (or full UUID) to renumber. A plain id that matches more
        /// than one task (the conflict this command exists to fix) is
        /// rejected as ambiguous — pass the full uuid shown by `doctor` or
        /// `show` to pick one.
        id: String,
        /// New display id to assign (must be a positive integer).
        new_id: i64,
        /// Allow assigning a display id already in use by another task,
        /// leaving both sharing it.
        #[arg(long)]
        force: bool,
    },

    /// Delete a task. Cascades to associated tags and dependency edges.
    /// The display ID is not reserved after deletion — a later `add` may
    /// reuse it if it was the highest one in use (identity is the task's
    /// UUID, not the display ID; see `show`).
    Rm {
        /// Task ID (or full UUID) to delete.
        id: String,
    },

    /// Export completed tasks. Default JSON shape: `{"completed": [{"date": "YYYY-MM-DD", "tasks": [...]}, ...]}`, descending by date, compact.
    ExportCompleted {
        /// Inclusive lower bound on completed_at (YYYY-MM-DD or RFC3339).
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        /// Exclusive upper bound on completed_at (YYYY-MM-DD or RFC3339).
        #[arg(long, value_name = "DATE")]
        until: Option<String>,
        /// Pretty-print the JSON output (multi-line, indented). Default is compact. Ignored when --format=ndjson.
        #[arg(long)]
        pretty: bool,
        /// Output format: json | ndjson | markdown. `ndjson` emits one task object per line (no date grouping; each task carries its own completed_at). `markdown` emits a date-grouped checklist.
        #[arg(long, default_value = "json")]
        format: String,
    },

    /// Cumulative flow diagram data: per-bucket backlog/in-progress/done/rejected
    /// counts, reconstructed purely from created_at/started_at/completed_at
    /// (no new schema, no snapshotting).
    Cfd {
        /// Inclusive lower bound (YYYY-MM-DD or RFC3339). Defaults to the earliest created_at.
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        /// Inclusive upper bound (YYYY-MM-DD or RFC3339). Defaults to today.
        #[arg(long, value_name = "DATE")]
        until: Option<String>,
        /// Bucket granularity: day | week.
        #[arg(long, default_value = "day")]
        bucket: String,
        /// Output format: ascii | csv | json. `json` shape: `{"buckets": [{"date": "...", "backlog": N, "in_progress": N, "done": N, "rejected": N}, ...]}`.
        #[arg(long, default_value = "ascii")]
        format: String,
    },

    /// List open tasks (pending/partial/in-progress) sorted oldest-first by
    /// created_at, for spotting backlog items that have aged unnoticed.
    /// Read-only report — does not change `priority` or any ordering used by
    /// `next`/`list`; pair with `edit --priority` to act on what it surfaces.
    Aging {
        /// Flag tasks with age (in whole days since created_at) >= N as stale.
        #[arg(long, value_name = "N")]
        stale_days: Option<i64>,
        /// Filter by tag. Repeatable; multiple tags AND together.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
    },

    /// Export in-progress + partial + pending tasks.
    ExportTodo {
        /// Output format: json | ndjson | markdown. `json` wraps tasks in `{"tasks": [...]}`; `ndjson` emits one task per line for streaming; markdown is terse by default.
        #[arg(long, default_value = "json")]
        format: String,
        /// Use heading-per-field markdown when --format markdown (default is terse).
        #[arg(long)]
        verbose: bool,
    },

    /// Merge two databases (optionally against a common-ancestor --base for
    /// a real 3-way merge). Identity is each task's UUID, so a colliding
    /// display ID never causes data loss; new tasks union in, and tasks
    /// known to --base are reconciled per-field, preferring the changed
    /// side, or --ours on a genuine same-field conflict (tagged
    /// `merge-conflict` for review). Without --base, a UUID shared by both
    /// sides is still reconciled the same way; a real field disagreement
    /// with no common ancestor to attribute it to is also a conflict. Two
    /// tasks may end up sharing a display ID after merging — `show <id>`
    /// lists both if that happens; use the full UUID to pick one.
    Merge {
        /// Common-ancestor database. Omit for a 2-way union merge.
        #[arg(long, value_name = "PATH")]
        base: Option<PathBuf>,
        /// "Our" database — wins ties and unresolved same-field conflicts.
        #[arg(long, value_name = "PATH", required = true)]
        ours: PathBuf,
        /// "Their" database to merge in.
        #[arg(long, value_name = "PATH", required = true)]
        theirs: PathBuf,
        /// Where to write the merged database. Defaults to overwriting --ours in place.
        #[arg(long, value_name = "PATH")]
        into: Option<PathBuf>,
        /// Abort (exit 1, write nothing) if any hard conflict is found, instead of
        /// auto-resolving with --ours and tagging it `merge-conflict`.
        #[arg(long)]
        strict: bool,
    },

    /// Implements git's `merge.<driver>.driver = ... %O %A %B` contract for
    /// use as a registered merge driver (see `install-merge-driver`). Not
    /// usually invoked by hand.
    GitMergeDriver {
        /// %O — common-ancestor temp file (may be empty/missing).
        base: PathBuf,
        /// %A — "ours" temp file; the merge result is written back here.
        ours: PathBuf,
        /// %B — "theirs" temp file.
        theirs: PathBuf,
    },

    /// Post-merge (and general) sanity checks: duplicate display ids,
    /// unresolved `merge-conflict` tags, orphaned tag/dep rows, self-deps,
    /// and dependency cycles. Read-only. Exits 1 if anything is found so it
    /// can gate a script (e.g. run after `git merge`/`pull`).
    Doctor,

    /// One-time setup: registers this binary as the git merge driver for the
    /// resolved database (adds a `.gitattributes` line + repo-local git
    /// config), so `git merge`/`pull`/`rebase` resolves DB conflicts
    /// automatically instead of leaving a binary conflict.
    InstallMergeDriver {
        /// Print what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
}

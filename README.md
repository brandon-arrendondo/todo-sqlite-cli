# todo-sqlite-cli

A scriptable per-project TODO list backed by SQLite. Designed for coding agents
(Claude Code and friends), but it's a plain CLI — no MCP, no daemon, no TTY.

## Why

Managing TODO state for coding agents via markdown files (e.g. `PLAN.md` +
`CHANGELOG.txt`) tends to break down when:

1. The agent drops or duplicates entries while moving items between files.
2. The plan file grows and wastes context after `/clear`.
3. A project spans multiple repos.

SQLite with a thin CLI fixes all three: the agent never rewrites a growing
file, "what's next?" is a single query, and the DB lives wherever you want.

## Install

```
cargo install --path .
```

One static binary, no runtime dependencies (SQLite is bundled via
`rusqlite`'s `bundled` feature).

## Quickstart

```
$ cd my-project
$ todo-sqlite-cli init
initialized /home/you/my-project/todo-sqlite-cli.db
wrote marker /home/you/my-project/.todo-sqlite-cli

$ todo-sqlite-cli add "fix login redirect" --tag auth --priority 2
1
$ todo-sqlite-cli add "write integration test" --depends-on 1
2
$ todo-sqlite-cli start 1
started 1

$ todo-sqlite-cli next
Task ID: 1
Title: fix login redirect
Status: in-progress
Priority: P2
Tags: auth
Started: 2026-04-20T12:01:00Z

$ todo-sqlite-cli done 1
done 1

$ todo-sqlite-cli export-completed
{"completed":[{"date":"2026-04-20","tasks":[{"id":1,"title":"...","..."}]}]}
```

Output is compact-by-default to keep token use down for AI agents. Pass
`--verbose` (on `show`, `export-todo --format markdown`) or `--pretty` (on
`export-completed`) when you want a human-readable dump.

## Database location (resolution order)

Every invocation resolves the DB in this order:

1. `--db PATH` flag
2. `TODO_SQLITE_CLI_DB` environment variable
3. Walk up from the current directory looking for a `.todo-sqlite-cli` marker
   file (first line = DB path; relative paths are resolved against the marker's
   directory, exactly like git finds `.git`).
4. Fail with exit code 1 and a message pointing at `todo-sqlite-cli init`.

The DB should not be checked into git; the marker file can be.

## Commands

| Command                   | Purpose                                                        |
|---------------------------|----------------------------------------------------------------|
| `init`                    | Create DB; write marker in cwd unless `--db` given             |
| `add TITLE`               | Add a task. Flags: `--details`, `--tag` (repeatable), `--priority 1..5\|P1..P5`, `--depends-on ID` (repeatable), `--start` |
| `list`                    | List active tasks. Flags: `--status pending\|partial\|in-progress\|done\|active\|all`, `--tag`, `--limit`, `--format table\|json\|markdown`, `--since DATE`, `--ids-only`, `--verbose` |
| `next`                    | Print the next task (oldest in-progress > oldest partial > highest-priority unblocked pending) |
| `start ID`                | Move to in-progress. Auto-moves any prior in-progress task to `partial` (preserving `started_at`). `--force` keeps multiple in-progress and ignores unmet deps. |
| `stop ID`                 | Move in-progress task to `partial`. Preserves `started_at`.    |
| `revert ID`               | Move task back to pending and clear `started_at`.              |
| `done ID`                 | Mark done. Idempotent.                                         |
| `show ID`                 | Task details. Omits default-valued fields; pass `--verbose` for the full dump. |
| `edit ID`                 | `--title`, `--details`, `--clear-details`, `--priority`, `--add-tag`/`--rm-tag`, `--add-dep`/`--rm-dep` |
| `rm ID`                   | Delete (cascades tags + deps).                                 |
| `export-completed`        | JSON of completed tasks, grouped by date desc. `--since`/`--until` bound on `completed_at` (YYYY-MM-DD or RFC3339). `--pretty` for indented output. |
| `export-todo`             | Active + partial + pending tasks. `--format json\|markdown`. Markdown is terse by default; pass `--verbose` for one-heading-per-field. |

Every command supports `--json` for machine-readable output, and `--db PATH`
to override resolution.

### Exit codes

- `0` — success
- `1` — user error (bad input, missing DB, violated invariant)
- `2` — system error (unwritable path, corrupt DB)

### Invariants

- IDs are SQLite `AUTOINCREMENT` — **never reused** after `rm`.
- `start` **auto-pauses** any prior in-progress task to `partial`
  (preserving `started_at`). `--force` keeps multiple tasks in-progress.
- `done` is **idempotent** — calling it on an already-done task doesn't
  rewrite `completed_at` and exits 0.
- `next` **skips blocked tasks** (any pending or partial task with an unmet
  dependency is not recommended).
- `partial` is the paused-but-started state — `start <id>` resumes it,
  `revert <id>` discards the start (clears `started_at`, returns to pending).

## Integration with Claude Code

Drop the contents of [examples/CLAUDE.md.snippet](examples/CLAUDE.md.snippet)
into your repo's `CLAUDE.md`. Highlights:

- Default to `next` over `list` — one task in one query.
- Use `list --ids-only` (or `--ids-only --json`) to detect change between
  turns without re-reading every task body.
- `list --since YYYY-MM-DD` for incremental updates.
- `show <id>` is terse by default (skips default-valued fields); pass
  `--verbose` only when you need every field.
- `start <id>` will auto-pause any prior in-progress task to `partial` —
  agents can switch tasks without manual `stop`/`start` choreography.

## Multi-repo usage

When a logical project spans several repos, point each repo's marker at the
**same** DB file:

```
~/work/project-frontend/.todo-sqlite-cli   →   ~/work/project/todo.db
~/work/project-api/.todo-sqlite-cli        →   ~/work/project/todo.db
~/work/project-infra/.todo-sqlite-cli      →   ~/work/project/todo.db
```

Each repo's `.todo-sqlite-cli` marker is just a text file whose first line is
the absolute path to the shared DB. Agents working in any repo automatically
find the shared backlog via the walk-up resolution.

Alternatively, export `TODO_SQLITE_CLI_DB=/path/to/shared.db` in your shell
profile and skip the marker entirely.

## Schema

```sql
tasks(id INTEGER PK AUTOINCREMENT, title TEXT NOT NULL, details TEXT,
      status TEXT IN ('pending','partial','in-progress','done'),
      priority INTEGER 1..5 DEFAULT 3,
      created_at TEXT, started_at TEXT, completed_at TEXT)
tags(task_id INT, tag TEXT, PRIMARY KEY(task_id, tag))
deps(task_id INT, depends_on_id INT, PRIMARY KEY(task_id, depends_on_id))
```

WAL mode + foreign keys on. The current schema version is 2; databases
written by older versions are migrated in place on first open (the
`partial` status was added in v2). The `meta(schema_version)` row tracks
the version.

## Prior art

- **[todolist-mcp](https://github.com/wdm0006/todolist-mcp)** — closest
  analog. Exposes a SQLite-backed TODO list to agents via MCP.
  `todo-sqlite-cli` is a plain CLI instead, so it works in any agent (or
  shell script, or CI job) and as a human tool without needing MCP wiring.
- **[Claude Todo Emulator MCP](https://www.pulsemcp.com/servers/joehaddad2000-claude-todo-emulator)**
  — MCP replacement for Claude Code's built-in `TodoWrite`, with
  workspace-local persistent storage. Same "don't lose state" motivation,
  MCP-shaped delivery.
- **[Claude Code native Tasks](https://code.claude.com/docs/en/agent-sdk/todo-tracking)**
  — Anthropic's built-in task tracking under `~/.claude/tasks`.
  `todo-sqlite-cli` differs by keeping state **project-adjacent**,
  **user-chosen** (any path you want), and **queryable outside Claude** —
  the same DB can back scripts, CI jobs, and other agents.
- **[claude-mem](https://aitoolly.com/ai-news/article/2026-04-16-claude-mem-a-new-plugin-for-automated-coding-session-memory-and-context-injection-in-claude-code)**
  — complementary tool. Compresses and re-injects past session context.
  Not a task store.

## Development

```
cargo build
cargo test
cargo run -- --help
```

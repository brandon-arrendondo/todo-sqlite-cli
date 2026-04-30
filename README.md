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
# Task ID: 1
# Title: fix login redirect
# Status: in-progress
# Priority: P2
# Tags: auth
# Created: 2026-04-20T12:00:00Z
# Started: 2026-04-20T12:01:00Z

$ todo-sqlite-cli done 1
done 1

$ todo-sqlite-cli export-completed
{
  "completed": [
    { "date": "2026-04-20", "tasks": [ { "id": 1, "title": "...", ... } ] }
  ]
}
```

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
| `add TITLE`               | Add a task. Flags: `--details`, `--tag` (repeatable), `--priority 1..5`, `--depends-on ID` (repeatable), `--start` |
| `list`                    | List active tasks. Flags: `--status pending\|in-progress\|done\|active\|all`, `--tag`, `--limit` |
| `next`                    | Print the next task (oldest in-progress beats highest-priority unblocked pending) |
| `start ID`                | Move to in-progress. `--force` allows >1 in-progress and ignores unmet deps |
| `stop ID`                 | Move in-progress task back to pending. Preserves `started_at`. |
| `revert ID`               | Move task back to pending and clear `started_at`.              |
| `done ID`                 | Mark done. Idempotent.                                         |
| `show ID`                 | Full task dump.                                                |
| `edit ID`                 | `--title`, `--details`, `--clear-details`, `--priority`, `--add-tag`/`--rm-tag`, `--add-dep`/`--rm-dep` |
| `rm ID`                   | Delete (cascades tags + deps).                                 |
| `export-completed`        | JSON of completed tasks, grouped by date desc. `--since`/`--until` bound on `completed_at` (YYYY-MM-DD or RFC3339). |
| `export-todo`             | Active + pending tasks. `--format json\|markdown`.             |

Every command supports `--json` for machine-readable output, and `--db PATH`
to override resolution.

### Exit codes

- `0` — success
- `1` — user error (bad input, missing DB, violated invariant)
- `2` — system error (unwritable path, corrupt DB)

### Invariants

- IDs are SQLite `AUTOINCREMENT` — **never reused** after `rm`.
- At most **one task in-progress** at a time (override with `start --force`).
- `done` is **idempotent** — calling it on an already-done task doesn't
  rewrite `completed_at` and exits 0.
- `next` **skips blocked tasks** (any pending task with an unmet dependency is
  not recommended).

## Integration with Claude Code

Drop this snippet into your repo's `CLAUDE.md`:

```markdown
## Task tracking

This repo uses `todo-sqlite-cli` for TODOs. The DB is at `./todo-sqlite-cli.db`
(resolved via the `.todo-sqlite-cli` marker at the repo root).

**Before planning or coding, ask the DB — do not read a PLAN.md file:**

- `todo-sqlite-cli next` — the single task to work on right now.
- `todo-sqlite-cli list` — all active (in-progress + pending) tasks.
- `todo-sqlite-cli show <id>` — full details for one task.

**When picking up work:**
- `todo-sqlite-cli start <id>` before coding.
- `todo-sqlite-cli done <id>` when the change is merged or committed.

**When something new comes up:**
- `todo-sqlite-cli add "title" --details "..." --tag <area> --priority <1-5>`.

Always prefer `todo-sqlite-cli next` over scanning a plan file — it returns
the same answer in one call without loading the full backlog into context.
```

See [examples/CLAUDE.md.snippet](examples/CLAUDE.md.snippet) for a
copy-pasteable version.

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
      status TEXT IN ('pending','in-progress','done'),
      priority INTEGER 1..5 DEFAULT 3,
      created_at TEXT, started_at TEXT, completed_at TEXT)
tags(task_id INT, tag TEXT, PRIMARY KEY(task_id, tag))
deps(task_id INT, depends_on_id INT, PRIMARY KEY(task_id, depends_on_id))
```

WAL mode + foreign keys on. Concurrent writers are serialized via SQLite's
`BEGIN IMMEDIATE` — two processes calling `start` at the same time won't
violate the single-in-progress invariant.

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

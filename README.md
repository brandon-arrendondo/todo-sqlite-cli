# todo-sqlite-cli

A scriptable per-project TODO list backed by SQLite, designed for coding
agents (Claude Code and friends). CLI-first — no daemon, no TTY required.
An optional Python MCP server wraps the binary for agents that prefer tool
calls over shell commands.

`man todo-sqlite-cli` is the full reference; `--help` works on every command.

## Install

```
cargo install todo-sqlite-cli
```

Single static binary, SQLite bundled. Pre-built `.deb`, `.rpm`, and AppImage
artifacts are attached to each
[release](https://github.com/brandonarrendondo/todo-sqlite-cli/releases).

## Quickstart

```
$ todo-sqlite-cli init
$ todo-sqlite-cli add "fix login redirect" --tag auth --priority P2
$ todo-sqlite-cli next
$ todo-sqlite-cli start 1
$ todo-sqlite-cli done 1
```

The DB path is resolved from `--db`, then `$TODO_SQLITE_CLI_DB`, then a
`.todo-sqlite-cli` marker walked up from cwd (like `.git`). One DB can back
multiple repos by pointing each repo's marker at the same absolute path.

## MCP server (optional)

An optional Python MCP server in [`mcp_server/`](mcp_server/) wraps the
binary as 12 tool calls (`list_tasks`, `add_task`, `start_task`, etc.) for
agents that use MCP rather than shell commands. It delegates all storage and
logic to the Rust binary — no second database, no duplicate code.

**Requirements:** Python ≥ 3.11, `mcp >= 1.0.0` (`pip install mcp`).

**Wire it into Claude Code** (`.claude/settings.json`):

```json
"mcpServers": {
  "todo": {
    "command": "python3",
    "args": ["/path/to/mcp_server/server.py"],
    "env": {
      "TODO_SQLITE_CLI_DB": "/path/to/your/todo.db"
    }
  }
}
```

**Environment variables:**
- `TODO_SQLITE_CLI_DB` — path to the SQLite DB (passed through to the CLI).
  If unset, the CLI walks up from its cwd looking for a `.todo-sqlite-cli`
  marker, so you can also just run the server from the project root.
- `TODO_SQLITE_CLI_BIN` — path to the binary (default: `todo-sqlite-cli` on
  `PATH`).

## For coding agents

**Via direct CLI** — drop
[examples/CLAUDE.md.snippet](examples/CLAUDE.md.snippet) into your repo's
`CLAUDE.md`. It teaches an agent the token-frugal patterns (`next` over
`list`, `--ids-only` re-polls, `--since` for incremental reads).

**Via MCP server** — wire up `mcp_server/server.py` as above. The tool
descriptions carry the same invariants; no `CLAUDE.md` snippet needed.

The non-obvious invariants either way:

- IDs are `AUTOINCREMENT` and **never reused** after `rm` — safe to cite by
  ID across turns.
- `start <id>` **auto-pauses** any prior in-progress task to `partial`
  (preserving `started_at`) — no manual stop/start choreography.
- `next` **skips blocked tasks** (unmet deps).
- `done` is **idempotent**.
- Output is **compact by default**; pass `--verbose` or `--pretty` only
  when a human is reading.

Exit codes: `0` success, `1` user error, `2` system error. Every command
supports `--json` and `--db PATH`.

## Why

Markdown task tracking (`PLAN.md` + `CHANGELOG.txt`) breaks down for coding
agents: edits drop or duplicate entries, growing plan files waste context
after `/clear`, and a project may span multiple repos. SQLite with a thin
CLI fixes all three.

## Alternatives

See [ALTERNATIVES.md](ALTERNATIVES.md) for the full landscape (Rust crates,
MCP servers, Claude Code's built-in tasks, Taskwarrior, dstask, todo.txt-cli)
and when *not* to use this tool.

## Development

```
cargo build
cargo test
```

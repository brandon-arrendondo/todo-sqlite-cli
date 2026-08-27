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

## Backlog trend reporting

Two read-only, additive report commands, reconstructed from timestamps
already on `tasks` — no schema change, no snapshotting:

```
$ todo-sqlite-cli cfd --bucket week
2026-08-01  backlog=42  in_progress=3  done=410  rejected=8
2026-08-08  backlog=38  in_progress=4  done=421  rejected=8
...
$ todo-sqlite-cli aging --stale-days 14
  12  pending      P5  age=  61d  low-priority task nobody's touched
   7  pending      P4  age=  22d  another aging candidate
```

`cfd` buckets a cumulative flow diagram (`--format ascii|csv|json`) for "is
the backlog thinning or just churning." `aging` lists open tasks oldest
`created_at`-first and flags anything past `--stale-days` as a rebase
candidate — it does not change `priority` or `next`/`list` ordering itself;
pair it with `edit --priority` to act on what it surfaces.

## Gates

Some tasks aren't work to be done — they're a checkpoint on a *condition*
becoming true (e.g. "sqc has reached maintenance-mode stability", gating a
paper-finalization task). Mark one with `add --gate` (or promote/demote an
existing task with `edit --gate` / `--no-gate`); the condition is just prose
in `--details`, judged by a human, not something the CLI evaluates.

```
$ todo-sqlite-cli add "sqc reaches maintenance mode" --gate \
    --details "condition: sqc stable for 2 weeks straight"
$ todo-sqlite-cli next        # skips the gate entirely
$ todo-sqlite-cli list --kind gate
```

Being a gate changes how the CLI's own views treat the task: `next` never
surfaces it (there's no start/stop episode that makes sense for a gate —
someone re-assesses the condition and calls `done` directly); `aging` keeps
listing it but never marks it `stale`, since indefinite openness is the
correct state for a gate, not backlog rot; `list`/`show`/`export-todo`
prefix `[GATE]` before its title.

## Merging

The DB can be checked into git like any other file — many projects want the
historical record of tasks and completions that gives them. With multiple
contributors (or multiple agent nodes on the same repo) that means
concurrent edits occasionally collide as a binary conflict, since git can't
text-merge opaque SQLite. `todo-sqlite-cli` ships a real three-way merge
engine plus a git merge driver so that resolves automatically instead of
being a manual pick-one-side-and-lose-the-other's-work conflict.

```
$ todo-sqlite-cli install-merge-driver   # one-time, per clone
```

That adds `<db> merge=todo-sqlite-cli` to `.gitattributes` (tracked — share
it) and registers the driver in local git config (each collaborator runs
this once). After that, `git merge`/`pull`/`rebase` just resolves the file:
a task only one side touched keeps that side's change; tags/deps union;
status picks whichever side is further along (`done`/`rejected` beat
`in-progress`/`partial` beat `pending`); a genuine same-field clash (e.g.
both sides gave a task a different title) keeps the current branch's value
and tags the task `merge-conflict` for a quick manual look:

```
$ todo-sqlite-cli list --tag merge-conflict
```

No driver installed, or merging two databases by hand? `merge --ours
--theirs [--base] [--into]` does the same thing on demand — pass `--base`
(the common-ancestor db, e.g. from `git show <merge-base>:path/to.db`) for
a real three-way merge; without it, every overlapping task id is treated as
an unrelated collision and renumbered rather than field-merged.

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

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
[release](https://github.com/brandon-arrendondo/todo-sqlite-cli/releases).

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

## Related tasks & location

A free-text "see task 12" comment goes stale the moment a merge renumbers or
duplicates display ids. `--related`/`--add-related`/`--rm-related` link two
tasks by uuid instead, so the connection survives merges and `renumber` —
`show` always resolves it to each task's *current* display id. The link is
mutual: linking A to B makes it show up on both tasks' `show` output without
touching B directly. It's deliberately left out of `list`/`export-*` output
(where the volume would add noise) and only surfaces on `show`.

Some tasks also can't be done just anywhere — `--location` records where
(e.g. a specific node or site), separately from `--details`/`--tag` so it
doesn't get lost in prose or lose its meaning as a tag. It shows up in
`list` as an `@location` suffix on the title (todo.txt's `@context`
convention) and as a `Location:` line on `show`.

```
$ todo-sqlite-cli add "replace intake filter" --location warehouse-3
$ todo-sqlite-cli add "audit warehouse-3 filters" --related 1
$ todo-sqlite-cli list
   1  pending      P3  replace intake filter @warehouse-3
   2  pending      P3  audit warehouse-3 filters
$ todo-sqlite-cli show 1
...
Related: 2
Location: warehouse-3
```

A task can also carry an `implementation_client` — which client (an MQTT
client id) last claimed it. It's part of the optional [MQTT sync](#mqtt-sync-optional-coordinatorworker)
feature below; set it directly with `--implementation-client`/
`--clear-implementation-client` on `add`/`edit`, or let a coordinator
auto-stamp it. Unlike `started_at`, it's sticky — `stop`/`revert` never
clear it. Shows as a `Client:` line on `show` only (no `list` suffix).

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
a task only one side touched keeps that side's change; tags/deps/related union;
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

A merge can leave two unrelated tasks sharing the same display id (identity
is each task's uuid, so nothing is lost — `show <id>` just lists both).
Run `doctor` to spot these, then `renumber <uuid> <new-id>` to give one of
them a fresh id:

```
$ todo-sqlite-cli doctor
$ todo-sqlite-cli renumber 3f9c1e2a-... 42
```

If a node's local database has been sitting untouched across a schema
upgrade (e.g. it was cloned or created long ago and no command has run
against it since), don't let the merge driver be the first thing to touch
it. Migrating a database mid-merge is unsafe when the other side is already
on a newer schema — some migrations mint a fresh uuid for every pre-existing
row with no way to recognize "this row on the old side is the same task as
that row on the new side," so a subsequent uuid-based merge would union them
as unrelated tasks and duplicate the whole backlog. The merge driver detects
this (comparing schema versions before opening anything) and refuses with an
error rather than merging silently. Fix it by running any command (e.g.
`doctor`) against the stale local database on its own, *before* pulling —
that migrates it deterministically against a pristine copy — then retry the
pull/merge.

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

## MQTT sync (optional, coordinator/worker)

With several agent nodes sharing one database, [git-based merging](#merging)
means very frequent merges, and merging has already caused a real corruption
incident ([`CORRUPTION_LOG.md`](CORRUPTION_LOG.md): a stale-schema node's
merge duplicated 618 of 620 tasks). As an alternative for that situation, the
MCP server can optionally run an MQTT sync layer with a single owner of the
truth instead of eventually-consistent merging:

- **`coordinator`** — one node owns the master database directly, exactly as
  in standalone mode, plus a pending-request queue fed by workers. Nothing
  from a worker touches the master db until the coordinator's agent calls
  `approve_request`/`reject_request`.
- **`worker`** — every other node. It never writes its own database — every
  mutation (`add_task`, `edit_task`, `start_task`, `stop_task`, `done_task`,
  `revert_task`, `rm_task`) becomes a pending request; the tool call returns
  immediately with `{"request_id": ..., "status": "pending"}` instead of a
  task, and `check_request(request_id)` polls the outcome. The worker's
  local database is a **disposable read replica** — never git-add/commit/
  merge it — kept current automatically by full-db-file snapshots (not
  replayed commands, so uuids/display-ids match the master exactly) the
  coordinator publishes after every applied change. There's no git merge
  step on the worker side at all.

Requires the `mqtt` extra (`pip install todo-sqlite-cli-mcp[mqtt]`, or
`pip install paho-mqtt>=2.0`). Enable it by pointing
`TODO_SQLITE_CLI_MQTT_CONFIG` at a JSON config file:

```json
{
  "mode": "coordinator",
  "host": "mqtt.example.internal",
  "port": 8883,
  "tls": true,
  "client_id": "coordinator-main",
  "username": "todo-sqlite-cli",
  "password_env": "TODO_MQTT_PASSWORD",
  "topic_prefix": "todo/tools_sqc",
  "db_path": "/path/to/master/todo.db"
}
```

A worker's config is the same shape with `"mode": "worker"`, a unique
`client_id` (also used as the MQTT client id workers are identified by in
requests/responses/messages/presence), and `"worker_db_path"` (its local
replica file) instead of `"db_path"`. Both nodes must share the same
`topic_prefix` and broker.

The password itself is **never** written into this file — only the name of
an environment variable (`password_env`) that holds it — but the file is
still node-local connection config, not something to commit; it's covered
by `.gitignore` already, along with the coordinator's pending-request queue
(`<db>.mqtt-pending.json`), the message-inbox files (`*.mqtt-inbox.json`,
`*.mqtt-messages.json`, `*.mqtt-broadcasts.json`), the `mqtt-files/`
attachment directory, and any `*mqtt*.json` config.

Eight more MCP tools appear alongside the usual 12, active only in the
matching mode (the others raise a clear error if called in the wrong mode):
`list_pending_requests`, `approve_request`, `reject_request`, `list_workers`,
`assign_task` (coordinator); `check_request`, `sync_state`,
`check_assignments`, `report_state` (worker); `send_message`,
`check_messages`, `broadcast`\*, `check_broadcasts`\* (\*coordinator-only
`broadcast`/worker-only `check_broadcasts`; `send_message`/`check_messages`
work in both modes). See
[examples/mqtt-coordinator-instructions.md](examples/mqtt-coordinator-instructions.md)
and
[examples/mqtt-worker-instructions.md](examples/mqtt-worker-instructions.md)
for the agent-facing workflow on each side.

**Topic layout, and why every worker-owned topic is per-worker:** every
topic a worker publishes to, or reads for messages meant only for it, is
scoped to that worker's own subtopic (`.../<its client_id>`) rather than one
topic every worker shares — `requests/<id>`, `responses/<id>`,
`assign/<id>`, `messages/to-coordinator/<id>`, `messages/to-worker/<id>`,
`presence/<id>`. The coordinator subscribes to the wildcard form
(`requests/+`, etc.); a worker subscribes only to its own leaf. This is
what lets a broker ACL grant each worker write access to just its own
subtopic instead of a topic every worker must be trusted with — the same
isolation a fleet's own worker-uplink/coordinator-downlink topic split
already relies on. `broadcast` gets the same treatment for a different
reason: each broadcast is published under its own `broadcast/<message_id>`
subtopic, so a retained one occupies a permanent slot of its own instead of
silently overwriting whatever standing announcement was retained on a
shared flat topic before it.

**Assignment and work-state, separate from messaging:**

- `assign_task(worker_id, body, task_id=None, file_path=None)`
  (coordinator only) — "here is what to work on next," distinct from
  `send_message`'s free-form notes. `task_id` is informational; the worker
  still reads full detail via `show_task`. Workers poll with
  `check_assignments()`.
- `report_state(state)` (worker only) — a work-state string (e.g.
  idle/busy/blocked, whatever convention the fleet agrees on), republished
  immediately alongside online presence rather than waiting for the next
  heartbeat. Visible to the coordinator as `list_workers()`'s `work_state`
  field.

**Messaging, files, presence, and reconnection:**

- `send_message(body, worker_id=..., file_path=None)` — a free-form direct
  message (≤ `message_max_chars`, default 4096) between one worker and the
  coordinator, in either direction, with an optional attached file (≤
  `max_file_bytes`, default 1 MB, sent inline as base64). `check_messages()`
  **drains** the recipient's queue — each message is returned exactly once,
  so there's no separate "mark as read" step. A coordinator's queue can hold
  messages from any number of workers; there's no requirement to handle them
  immediately, they just wait. An attachment lands on disk under
  `mqtt-files/` (configurable via `files_dir`) and the drained message
  carries a `file_path`, never raw file bytes in tool output.
- `broadcast(body, retain=False, file_path=None)` (coordinator only) —
  publish to every worker at once; `retain=true` means a worker that
  connects (or reconnects) later gets it immediately, no resend needed.
  Workers drain their broadcast queue with `check_broadcasts()`.
- `list_workers()` (coordinator only) — presence for every worker seen so
  far: `{worker_id, status, work_state, ts}` (`work_state` is whatever a
  worker last passed to `report_state()`, null if it never has). Each
  worker announces `"online"`
  immediately on connect and republishes it every `heartbeat_interval_s`
  (default 60s); an MQTT Last-Will-Testament makes the broker publish
  `"offline"` automatically if a worker's connection drops without a clean
  disconnect. `ts` is always the coordinator's own receipt time (not
  whatever the worker's payload claims), so staleness is just "hasn't
  advanced in a few heartbeat intervals."
- All of this (plus the existing request/response/state topics) rides on a
  **persistent MQTT session** per node (`session_expiry_s`, default 24h,
  keyed by each node's `client_id`) — a node that drops offline briefly
  (a network blip, or the gap between one MCP process dying and the next
  one starting) reconnects and picks up any QoS-1 messages the broker
  queued for it meanwhile, rather than silently losing them. Since MQTT's
  "at least once" delivery combined with resubscribing on every reconnect
  can occasionally redeliver the same message twice, messages/broadcasts
  are de-duplicated by `message_id` on receipt — you will never see the
  same one twice from `check_messages`/`check_broadcasts`.

A task can also carry an `implementation_client` field recording which
client claimed it — see [Related tasks & location](#related-tasks--location)
above.

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

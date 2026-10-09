# todo-sqlite-cli

A scriptable per-project TODO list backed by SQLite, designed for coding
agents (Claude Code and friends). CLI-first — no daemon, no TTY required.
An optional Python MCP server wraps the binary for agents that prefer tool
calls over shell commands.

`--help` works on every command and is the reference for a plain
`cargo install`. A full man page also exists (`man/todo-sqlite-cli.1` in
this repo); `cargo install` does not install it into your system man path.
See Install below for obtaining and installing the companion files.

## Install

```
cargo install todo-sqlite-cli --locked
```

Single Rust executable, SQLite bundled — but `cargo install` only builds the
binary itself, nothing else in this repo. For the man page or the optional
MCP server (see below), download the
`todo-sqlite-cli-<os>-x64-<version>.tar.gz`/`.zip` archive from a
[release](https://github.com/brandon-arrendondo/todo-sqlite-cli/releases)
instead — it bundles the binary alongside `man/`, `mcp_server/`, and
licensing info. Release CI also prepares `.deb`/`.rpm`/AppImage packages
with the manual; check the release assets for available packages.

## Quickstart

```
$ todo-sqlite-cli init
$ todo-sqlite-cli add "fix login redirect" --tag auth --priority P2
$ todo-sqlite-cli next
$ todo-sqlite-cli start 1
$ todo-sqlite-cli done 1
```

For commands operating on an existing database, the DB path is resolved
from `--db`, then nonempty `$TODO_SQLITE_CLI_DB`, then a
`.todo-sqlite-cli` marker walked up from cwd (like `.git`). One DB can back
multiple repos by pointing each repo's marker at the same absolute path.
Relative marker paths resolve against the marker directory. `init` is
separate: without `--db`, it creates a database and marker in the current
directory (or `--marker-dir`) rather than using the environment or a parent
marker. See [database selection](docs/usage.rst).

## Backlog trend reporting

Two report commands reconstruct trends from timestamps already on `tasks`,
without adding event tracking or editing task rows:

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
pair it with `edit --priority` to act on what it surfaces. These reports
reconstruct history from current timestamps, not an event log: reverting
clears `started_at`, so historical work episodes are not preserved.
Report commands still use the normal database opener and can migrate an
older schema even though they do not edit task rows.

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
feature below; set it with `--implementation-client` on `add` or `edit`, clear it with
`edit --clear-implementation-client`, or let a coordinator auto-stamp it. Unlike `started_at`, it's sticky — `stop`/`revert` never
clear it. Shows as a `Client:` line on `show` only (no `list` suffix).

A coordinator's db often spans several projects at once — `--project-name`
records which one a task belongs to (todo.txt's `+project` convention). It
shows up in `list` as a `+project` suffix on the title (alongside the
`@location` suffix), as a `Project:` line on `show`, and `list
--project-name <name>` filters down to just that project's tasks.

```
$ todo-sqlite-cli add "replace intake filter" --project-name warehouse-refit --location warehouse-3
$ todo-sqlite-cli list --project-name warehouse-refit
   1  pending      P3  replace intake filter +warehouse-refit @warehouse-3
```

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
a real three-way merge; without it, matching UUIDs are still reconciled as one task, but
differing fields keep ours and are flagged. Different UUIDs remain separate
even if their display IDs match; the merge does not renumber them.

A merge can leave two unrelated tasks sharing the same display id (identity
is each task's uuid, so nothing is lost — `show <id>` fails with both matches listed).
Run `doctor` to spot these, then `renumber <uuid> <new-id>` to give one of
them a fresh id:

```
$ todo-sqlite-cli doctor
$ todo-sqlite-cli renumber 3f9c1e2a-... 42
```

Both manual merges and the Git driver check schema versions before normal
opening can migrate inputs. Mismatched versions are refused. Back up the
databases before resolving a mismatch. When upgrading a pre-UUID database,
migrate one shared snapshot and distribute it before diverging again;
independently migrating old copies generates different UUIDs for the same
historical tasks. Merely running `doctor` separately on every copy can align
schema versions while leaving incompatible identity histories. See the
[merge guide](docs/merge-engine.rst) and [incident record](docs/incidents.rst).

## MCP server (optional)

An optional Python MCP server in [`mcp_server/`](mcp_server/) wraps the
binary as 13 tool calls (`list_tasks`, `add_task`, `start_task`, etc.) for
agents that use MCP rather than shell commands. It delegates all storage and
logic to the Rust binary — no second database, no duplicate code.

Not published to PyPI — a plain `cargo install` doesn't include it, so get
`mcp_server/` either by cloning this repo or downloading a release archive
(see Install above).

**Requirements:** Python ≥ 3.11. Either install the loose dependency and
run the script in place —

```
pip install "mcp>=1,<2"
python3 /path/to/mcp_server/server.py
```

— or install `mcp_server/` itself as a local package, which also registers
a `todo-mcp-server` command on `PATH`:

```
pip install /path/to/mcp_server
```

**Example MCP client configuration** (adapt the file location and outer structure to your client):

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
incident ([`docs/incidents.rst`](docs/incidents.rst): a stale-schema node's
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

Requires `paho-mqtt>=2.0` — `pip install paho-mqtt`, or, if you installed
`mcp_server/` as a package per above, `pip install /path/to/mcp_server[mqtt]`
to pull in both dependencies at once. Enable it by pointing
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
`*.mqtt-messages.json`, `*.mqtt-broadcasts.json`), the directive, sent-log
and receipt state files (`*.mqtt-directive-seq.json`, `*.mqtt-sent.json`,
`*.mqtt-directives.json`), a legacy `mqtt-files/` attachment directory, and
any `*mqtt*.json` config.

Optional config fields, with their defaults:

- `heartbeat_interval_s` (60): how often each node (coordinator and
  worker) republishes its retained presence.
- `stale_after_heartbeats` (3): the link counts as stale after this many
  heartbeat intervals with no inbound broker traffic (see
  [Fleet state, presence, and stale links](#fleet-state-presence-and-stale-links)).
- `files_dir`: where incoming attachments land. Defaults to
  `$XDG_STATE_HOME/todo-sqlite-cli/mqtt-files/` (falling back to
  `~/.local/state/todo-sqlite-cli/mqtt-files/`), deliberately outside any
  repo checkout. Before this default, attachments landed in `mqtt-files/`
  next to the db, and one got swept into a local commit.
- `checkin_file`, `deadman_drain_after_s` (off; 10800 recommended),
  `deadman_pause_after_s` (drain + 1h): coordinator only, see
  [Dead-man switch](#dead-man-switch).
- `coordinator_offline_drain_after_s` (2h): worker only, see the same
  section.
- `message_max_chars` (4096), `max_file_bytes` (1 MB),
  `request_timeout_s` (30).

This version warns on stderr about config keys it doesn't know and
ignores them. It rejects a malformed number such as `"3h"`, or a negative
`*_after_s`, at startup.
Older servers crash on any key they don't know, so upgrade a node's server
before adding a new key such as `stale_after_heartbeats` to its config.

**Rollout order:**
1. Update the broker ACL (below).
2. Upgrade the servers, coordinator or workers in any order.
3. Add any new config keys.

A new worker on the old ACL can't read its own presence leaf, so it can't
check its link. Every reply then reports `link.error` ("subscription to …
denied by broker ACL"), and an empty drain raises that error rather than
a stale-link loop.

Twenty more MCP tools exist for MQTT sync, but each is only registered on
a node whose configured mode it's valid for — a standalone deployment (no
MQTT config) sees none of them, a coordinator sees only the coordinator (and
shared) ones, a worker only the worker (and shared) ones. This keeps a
node's tool list free of entries that would just error if called:
`list_pending_requests`, `approve_request`, `reject_request`, `list_workers`,
`assign_task`, `delete_broadcast`, `list_sent`, `set_fleet_state`,
`get_fleet_state`, `checkin` (coordinator); `check_request`, `sync_state`,
`check_assignments`, `report_state`, `fleet_state`, `latest_directive`
(worker); `send_message`,
`check_messages`, `broadcast`\*, `check_broadcasts`\* (\*coordinator-only
`broadcast`/`delete_broadcast`, worker-only `check_broadcasts`;
`send_message`/`check_messages` work in both modes). See
[examples/mqtt-coordinator-instructions.md](examples/mqtt-coordinator-instructions.md)
and
[examples/mqtt-worker-instructions.md](examples/mqtt-worker-instructions.md)
for the agent-facing workflow on each side.

**Topic layout, and why every worker-owned topic is per-worker:** every
topic a worker publishes to, or reads for messages meant only for it, is
scoped to that worker's own subtopic (`.../<its client_id>`) rather than one
topic every worker shares — `requests/<id>`, `responses/<id>`,
`assign/<id>`, `messages/to-coordinator/<id>`, `messages/to-worker/<id>`,
`presence/<id>`, `receipts/<id>`. The coordinator subscribes to the wildcard form
(`requests/+`, etc.); a worker subscribes only to its own leaf. This is
what lets a broker ACL grant each worker write access to just its own
subtopic instead of a topic every worker must be trusted with — the same
isolation a fleet's own worker-uplink/coordinator-downlink topic split
already relies on. `broadcast` gets the same treatment for a different
reason: each broadcast is published under its own `broadcast/<message_id>`
subtopic, so a retained one occupies a permanent slot of its own instead of
silently overwriting whatever standing announcement was retained on a
shared flat topic before it. The topics every worker reads, `broadcast/#`,
`state` (the db snapshot), `fleet/state` and `coordinator/presence`, are
written only by the coordinator.

A mosquitto ACL matching this layout, with `P` standing for your
`topic_prefix` and `%c` for the connecting client id:

```
user <coordinator-username>
topic readwrite P/#

pattern write     P/requests/%c
pattern read      P/responses/%c
pattern read      P/assign/%c
pattern write     P/messages/to-coordinator/%c
pattern read      P/messages/to-worker/%c
pattern readwrite P/presence/%c
pattern write     P/receipts/%c
pattern read      P/broadcast/#
pattern read      P/state
pattern read      P/fleet/state
pattern read      P/coordinator/presence
```

A worker needs `readwrite`, not just `write`, on its own `presence/%c`:
it reads its own heartbeat back to check that its link is live.

**Assignment and work-state, separate from messaging:**

- `assign_task(worker_id, body, task_id=None, kind=None, file_path=None)`
  (coordinator only) — "here is what to work on next," distinct from
  `send_message`'s free-form notes. With a `task_id` it becomes a
  sequenced directive (see
  [Directives and receipts](#directives-and-receipts)); the worker still
  reads full task detail via `show_task`. Workers poll with
  `check_assignments()`.
- `report_state(state)` (worker only) — a work-state string (e.g.
  idle/busy/blocked, whatever convention the fleet agrees on), republished
  immediately alongside online presence rather than waiting for the next
  heartbeat. Visible to the coordinator as `list_workers()`'s `work_state`
  field.

**Messaging, files, presence, and reconnection:**

- `send_message(body, worker_id=..., file_path=None, task_id=None, kind=None)` — a free-form direct
  message (≤ `message_max_chars`, default 4096) between one worker and the
  coordinator, in either direction, with an optional attached file (≤
  `max_file_bytes`, default 1 MB, sent inline as base64). `check_messages()`
  **drains** the recipient's queue — each message is returned exactly once,
  so there's no separate "mark as read" step. A coordinator's queue can hold
  messages from any number of workers; there's no requirement to handle them
  immediately, they just wait. An attachment lands on disk under the
  `files_dir` (by default in the XDG state dir, see above) and the drained
  message carries a `file_path`, never raw file bytes in tool output. On
  the coordinator, `task_id`/`kind` make the message a sequenced
  directive, as with `assign_task`, except that `kind` defaults to `info`.
- `broadcast(body, retain=False, file_path=None)` (coordinator only) —
  publish to every worker at once; `retain=true` means a worker that
  connects (or reconnects) later gets it immediately, no resend needed.
  Workers drain their broadcast queue with `check_broadcasts()`.
- `delete_broadcast(message_id)` (coordinator only) — clear a retained
  broadcast (one previously sent with `retain=true`) so a worker that
  connects or reconnects later no longer receives it. No effect on a
  broadcast that wasn't retained, and no effect on a worker that already
  drained it.
- `list_workers()` (coordinator only) — presence for every worker seen so
  far: `{worker_id, status, work_state, ts, age_s}` (`work_state` is whatever a
  worker last passed to `report_state()`, null if it never has). Each
  worker announces `"online"`
  immediately on connect and republishes it every `heartbeat_interval_s`
  (default 60s); an MQTT Last-Will-Testament makes the broker publish
  `"offline"` automatically if a worker's connection drops without a clean
  disconnect. `ts` is always the coordinator's own receipt time (not
  whatever the worker's payload claims), so staleness is just "hasn't
  advanced in a few heartbeat intervals."
- All of this (plus the existing request/response/state topics) uses a
  **clean MQTT session** per node (keyed by each node's `client_id`) —
  nodes are LAN-connected and this is a polling architecture, so a sender
  that doesn't see its message acted on just resends it rather than
  relying on the broker to queue messages for an offline client. Since
  MQTT's "at least once" delivery can still occasionally redeliver the
  same message twice, messages/broadcasts are de-duplicated by
  `message_id` on receipt — you will never see the same one twice from
  `check_messages`/`check_broadcasts`.

**Fleet state, presence, and stale links:**

- `set_fleet_state(state, note=None)` / `get_fleet_state()` (coordinator)
  set and read a retained fleet-wide state:
  - `active`: work normally.
  - `draining`: finish the current task, then pause.
  - `paused`: checkpoint, `report_state`, and stop scheduling wakeups.
  Nothing retained means `active`. This is the soft "Level A" switch: it
  relies on agents cooperating.
- Every worker MQTT tool reply (`check_messages`, `check_assignments`,
  `check_broadcasts`, `sync_state`, `check_request`, `report_state`,
  `latest_directive`, and the pending reply from every write tool) carries
  three extra keys, so an agent sees them on its next poll without a
  separate call. `fleet_state()` returns just those keys:
  - `fleet_state`: `{state, note, set_at, set_by, source, instruction}`.
    For `draining`/`paused`, `instruction` spells out what to do (finish or
    checkpoint, push the work branch, add a state note, send a final
    message, `report_state('offline')`, stop polling). Agents follow it
    literally.
  - `coordinator`: `{status: online|offline|unknown, last_seen, age_s}`.
    The coordinator heartbeats a retained `online` on `coordinator/presence`
    every `heartbeat_interval_s`, with a Last-Will `offline` for a crash and
    an explicit `offline` on a clean exit. It reads `online` only while
    heartbeats keep arriving. A coordinator too old to publish presence
    shows as `unknown`.
  - `link`: `{connected, last_rx_at, age_s, stale, stale_since,
    denied_subscriptions, error}`, this node's own broker link.
    - Any inbound traffic proves the link is alive: a message, the broker's
      ack of our heartbeat, or the heartbeat echoed back on our own presence
      topic.
    - The link is stale after a disconnect, or after
      `stale_after_heartbeats` intervals of silence.
    - `error` names subscriptions the broker ACL denies. mosquitto accepts
      a denied subscription and then delivers nothing, so the tell is
      heartbeats the broker acks that never come back.
- On a stale link, or with `link.error` set, an empty
  `check_messages`/`check_assignments` drain (on either side) and
  `latest_directive` are **errors** rather than empty results. The error says: "do not act on silence; report_state and retry
  next poll". A non-empty drain still returns its items, plus
  `link_stale_since`. This came out of a coordinator link that went silent
  for ~70 minutes while local MCP calls kept succeeding.
- If you roll the coordinator back to a version without presence, clear
  the retained `offline` it left behind (`mosquitto_pub -r -n -t
  P/coordinator/presence`), or workers keep reporting it offline.

**Dead-man switch:**

If the operator walks away without winding the fleet down, the fleet
drains and stops on its own. This needs no model turn: an idle agent
doesn't run, but its server process does.

- **Off by default.** It's enabled only when `deadman_drain_after_s` is set
  (> 0) in the coordinator's config. 10800 (3h) is the recommended value.
- **Coordinator:**
  - The coordinator's server process checks, on its heartbeat thread, the
    mtime of `checkin_file` (default
    `$XDG_STATE_HOME/todo-sqlite-cli/checkin`). The timer runs from the
    later of that mtime and the process start, so a fresh start (or a
    missing file) never drains instantly.
  - After `deadman_drain_after_s` without a check-in, it publishes
    `fleet/state` `draining`, with `set_by: "deadman"` and a `dead-man: no
    check-in since …` note. After `deadman_pause_after_s`, it publishes
    `paused`.
  - It only ever moves toward draining/paused. It never relaxes a state,
    so a hand-set `paused` stays paused.
  - It never resumes on its own. A check-in only resets the timer, and
    going back to `active` takes an explicit `set_fleet_state`.
  - It does nothing while its link is stale, or until it has learned the
    retained fleet state from the broker. That stops a stale local copy
    from overwriting a hand-set pause on reconnect.
  - It logs each state it sets to stderr. It also logs a fleet-state
    publish still unsent when the process exits.
  - An explicit `0` or `null` for `deadman_pause_after_s` disables just the
    pause stage.
  - `get_fleet_state` and `list_workers` show `deadman: {enabled,
    checkin_file, last_checkin, timer_from, drains_at, pauses_at}`.
- **Check-ins:** a Claude Code `UserPromptSubmit` hook on the coordinator
  host touches the file on each operator prompt, in `settings.json`:

  ```json
  {"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command",
    "command": "d=\"${XDG_STATE_HOME:-$HOME/.local/state}/todo-sqlite-cli\"; mkdir -p \"$d\" && touch \"$d/checkin\""}]}]}}
  ```

  **This unfiltered hook is not enough.** `UserPromptSubmit` also fires for
  prompts injected into the session, such as scheduled wakeups, loop ticks,
  cross-session messages and task notifications. Each of those would reset
  the timer and defeat the switch. The hook has to filter on its stdin
  JSON and touch the file only for a prompt the operator really typed. The `checkin(note)`
  coordinator tool touches the file too, as a manual fallback. Agents
  should call it only when the operator asks.
- **Worker fallback:** a worker whose coordinator is gone for
  `coordinator_offline_drain_after_s` reports an effective
  `fleet_state: {state: "draining", source: "coordinator-offline", since,
  retained: {…}}`. "Gone" means a Last-Will/clean `offline`, or heartbeats
  that stopped, which includes the worker's own link being down.
  - The clock never starts before the worker's own process start, so a
    worker started long after the coordinator left still gets the full
    grace period.
  - At twice the threshold it reports `paused`.
  - It's purely local: nothing is published, and it lapses as soon as the
    coordinator is back.
  - `latest_directive`'s `ok_to_act` follows it.
  - A coordinator that has never published presence (an older version)
    never triggers it.

**Directives and receipts:**

- An `assign_task`/`send_message` with a `task_id` is a **directive**. The
  coordinator resolves the task (display id or uuid) against the master db
  and stamps `task_uuid`, a `kind` (`approve|hold|go|info`) and a `seq`
  that strictly increases per (worker, task). A seq is never dense: it's
  `max(last + 1, now in ms)`, so a lost counter file can't hand out a lower
  one.
- The worker marks each drained directive `superseded: true` when a newer
  `approve`/`hold`/`go` for the same task has arrived, on either channel.
  A `hold` therefore supersedes an earlier `go`. An `info` never supersedes
  anything.
- `latest_directive(task_id)` (worker) resolves `task_id` through the
  local replica and matches on uuid only, so a display id that
  `renumber_task` moved to another task never inherits the old task's
  directive. An exact uuid already in the directive ledger is answered
  from the ledger, without the replica. Any other id the replica can't
  resolve is an error; the fix is for the coordinator to re-send. The
  coordinator publishes a db snapshot before every sequenced
  approve/hold/go directive, so a task added by raw CLI on its host still
  reaches the replica. That snapshot is skipped when the db hasn't changed
  since the last one published (sha256 of the checkpointed file), and for
  `info` entirely. Write and approve paths always publish. It
  returns the effective directive, plus:
  - `ok_to_act`: true only for `go`/`approve`, with the fleet not paused
    and a fresh link.
  - `draining`: finish the current task only, and don't pick up a new
    one.
  Agents should re-check it immediately before any irreversible step, such
  as a push to main.
- Sending a `go`/`approve` while the fleet isn't `active` still sends it,
  with a `warning` in the reply. A `hold`/`info` never warns, since a
  `hold` is exactly what the coordinator sends while pausing.
- Receipts: a worker publishes to its own `receipts/<id>` when its server
  receives a message or assignment (`delivered`) and when its agent drains
  it (`read`). `list_sent(worker_id=None, unread_only=False)` (coordinator)
  shows the last 1000 sends with `sent_at`/`delivered_at`/`read_at` and
  `status: sent|delivered|read`. A worker too old to send receipts stays
  at `sent`.

**Mixed versions:** every new field is optional on the wire. An old worker
still works against a new coordinator: it ignores the new keys and never
acks. A new worker still works against an old coordinator: the coordinator
shows as `unknown` and assignments arrive unsequenced. Neither case changes
the task db schema.

**Regression check:** `mcp_server/tests/mqtt_fleet_scenario.py` starts a
throwaway local mosquitto with the ACL above, then drives one coordinator
and two workers over MCP stdio through every feature above: fleet state, a
`kill -9`'d coordinator, a broker outage, go/hold supersede, receipts,
attachments, ACL denials, and the dead-man switch and worker fallback
(with timers in seconds). It needs `mosquitto`, `mosquitto_passwd`,
`mcp<2` and `paho-mqtt`, and skips cleanly (exit 0) if any is missing:

```
python mcp_server/tests/mqtt_fleet_scenario.py --compat-ref <older-git-ref>
```

`--compat-ref` is optional. It adds mixed-version checks against the
server as of that ref.

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

- UUIDs are task identity. Display IDs use `MAX(id) + 1` at insertion;
  deleting the highest ID can let a later task reuse it, and merges can
  leave duplicates. Use full UUIDs for durable references.
- `start <id>` **auto-pauses** any prior in-progress task to `partial`
  (preserving `started_at`) — no manual stop/start choreography.
- `next` checks dependencies for partial and pending tasks; its in-progress
  tier returns current work without rechecking dependencies.
- `done` is **idempotent**.
- Output is **compact by default**; pass `--verbose` or `--pretty` only
  when a human is reading.

Exit codes: `0` success, `1` runtime user error, `2` runtime system error
or command-line parsing failure. `--json` and `--db PATH` are global flags;
the merge commands use their explicit database paths, and the Git driver
and installer do not produce JSON.

## Why

Markdown task tracking (`PLAN.md` + `CHANGELOG.txt`) breaks down for coding
agents: edits drop or duplicate entries, growing plan files waste context
after `/clear`, and a project may span multiple repos. SQLite with a thin
CLI fixes all three.

## Alternatives

See [ALTERNATIVES.md](ALTERNATIVES.md) for the full landscape (Rust crates,
MCP servers, Claude Code's built-in tasks, Taskwarrior, dstask, todo.txt-cli)
and when *not* to use this tool.

## AI Assistance

todo-sqlite-cli was developed with assistance from [Claude](https://claude.ai) (Anthropic), used for code generation, bug fixes, release packaging, and documentation. From October 2026, [Codex](https://openai.com/codex/) (OpenAI) also contributed the tooling that enforces the agent and commit guidelines. Each of its changes was reviewed before it was merged.

Many earlier commits have a `Co-Authored-By: Claude` trailer, but not every AI-assisted commit does, so the trailers are not a complete record. From October 2026 the contribution is acknowledged once, here, and not with a co-author trailer on each commit.

## Documentation and development

The [Sphinx guide](docs/index.rst) covers installation, database selection,
CLI behavior, architecture, merge semantics, and troubleshooting:

- [Installation](docs/installation.rst)
- [CLI and database selection](docs/usage.rst)
- [Merge engine](docs/merge-engine.rst)
- [Troubleshooting](docs/troubleshooting.rst)
- [Contributor setup and checks](docs/development.rst)

```sh
cargo build --locked
cargo test --locked
pre-commit run --all-files
```

Read [AGENTS.md](AGENTS.md) and [CLAUDE.md](CLAUDE.md) before contributing.
The repository toolchain tracks stable; Cargo.toml declares Rust 1.75.

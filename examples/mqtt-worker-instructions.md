## Task tracking — MQTT worker node

This node runs `todo-sqlite-cli`'s MCP server in **worker** mode (optional
MQTT coordinator/worker sync — see the top-level `README.md`'s "MQTT sync"
section). A coordinator node elsewhere owns the master database; this node
never writes its own.

**The write tools don't apply immediately — they submit a request:**

- `add_task`, `edit_task`, `start_task`, `stop_task`, `done_task`,
  `revert_task`, `rm_task` all return
  `{"request_id": "...", "status": "pending"}` instead of a task object.
  Nothing has changed yet.
- Call `check_request(request_id)` to poll the outcome:
  `{"status": "pending"}` (still waiting — the coordinator's agent hasn't
  looked at it yet), `{"status": "approved", "task": {...}}` (applied — the
  task object is the same shape `show_task` returns), or
  `{"status": "rejected", "error": "..."}` (not applied — read the reason;
  it usually means the coordinator's agent judged the request invalid or
  unwanted, not a transient failure, so don't just retry the identical
  request).
- There's no fixed SLA on approval — it depends on the coordinator's agent
  being active. Don't spin tightly on `check_request`; a task list doesn't
  usually need a decision within the same second. If a request sits pending
  for a long time and you need it sooner, `send_message` the coordinator
  about it directly (see below) rather than just re-submitting the same
  request.

**Reads work exactly like standalone mode** (`list_tasks`, `next_task`,
`show_task`, `export_todo`, `export_completed`) — they read this node's
local replica database, which is kept current automatically in the
background as the coordinator publishes updates. You don't need to sync
before reading. `sync_state()` reports the last-synced timestamp if you want
to confirm the replica isn't stale (e.g. after a long gap or suspected
connectivity issue) — it's a freshness check, not a required step.

**The local replica database is disposable — never commit it, never merge
it.** It exists only to make reads fast between MQTT updates and gets
overwritten wholesale on every coordinator snapshot. The master database on
the coordinator's node is the only one with a real history worth keeping in
git.

**`implementation_client`** on a task shows which client claimed it — the
coordinator auto-stamps it with this node's client id when it approves a
`start` request, so you don't need to set it yourself.

**Messaging the coordinator directly** (separate from write requests):

- `send_message(body, file_path=None)` — a free-form note to the
  coordinator (up to 4096 chars by default), with an optional file
  attached (up to 1 MB by default — a config snippet, a short log excerpt,
  a small screenshot; not a bulk transfer mechanism).
- `check_messages()` **drains** whatever the coordinator has sent back
  since your last call — each message is returned exactly once, so there's
  no need to track what you've already seen. An incoming attachment shows
  up as a local `file_path` to `Read`, not raw content in the tool result.
- `check_broadcasts()` drains standing instructions or announcements the
  coordinator has sent to every worker. Check it periodically — a
  `retain=true` broadcast is delivered to you immediately on connect even
  if it was published before this session started, so don't assume you've
  seen everything just because your session is new.

**You don't need to do anything for presence** — this node announces
"online" automatically on connect and keeps refreshing it; the broker
announces "offline" on your behalf if this node's connection drops
uncleanly. No tool call needed on your end for either.

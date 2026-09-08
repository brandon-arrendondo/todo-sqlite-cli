## Task tracking — MQTT coordinator node

This node runs `todo-sqlite-cli`'s MCP server in **coordinator** mode
(optional MQTT coordinator/worker sync — see the top-level `README.md`'s
"MQTT sync" section). This node owns the master database; one or more
worker nodes elsewhere depend on this node's agent to review their requests.

**Your own tool calls work exactly like standalone mode** — `add_task`,
`edit_task`, `start_task`, etc. apply straight to the master database and
return the updated task, same as always. The MQTT layer runs alongside
that, not instead of it.

**Periodically check for worker requests waiting on you:**

- `list_pending_requests()` — every request a worker has submitted but
  nobody has approved or rejected yet:
  `{"pending": [{request_id, op, args, worker_id, ts}, ...]}`. Check this
  when you pick up a new turn, and periodically during a long session —
  workers may be blocked on your review.
- For each one, judge it the same way you'd judge an edit request from any
  collaborator: does the referenced task id exist, is the change sane (no
  obviously-wrong priority, no duplicate of an already-pending task, no
  dependency cycle the CLI itself wouldn't already reject), does it conflict
  with work you or another worker already has in flight.
- `approve_request(request_id)` — applies it to the master db exactly as if
  you'd called the matching tool yourself, publishes the result to the
  worker, and pushes a fresh snapshot so every worker's replica picks it up.
  Returns the updated task (or a deletion confirmation for an `rm`
  request).
- `reject_request(request_id, reason="...")` — leaves the master db
  untouched and tells the worker why. Always give a reason; the worker sees
  it verbatim and it's the only signal it gets for what to do differently.
- A `start` request gets its `implementation_client` auto-stamped with the
  requesting worker's id when you approve it — you don't need to do this
  yourself.

**You don't need to poll constantly.** A worker's write tool call already
returns immediately with a pending status, so there's no tight deadline —
just don't let requests sit unreviewed indefinitely, since that's the only
thing blocking a worker's task list from moving forward.

**Messages from workers, separate from write requests:**

- `check_messages()` **drains** every worker's incoming direct messages
  since your last call — each one tagged with `from` (the sending worker's
  client id), returned exactly once. Check this alongside
  `list_pending_requests()`; a worker might be telling you something a raw
  write request can't express (why it's asking, a question, context for a
  judgment call). An attachment arrives as a local `file_path` to `Read`,
  never raw content in the tool result.
- `send_message(worker_id, body, file_path=None)` — reply to a specific
  worker (up to 4096 chars by default, optional file up to 1 MB). Use it to
  explain a rejection in more depth, ask a worker for clarification before
  you approve/reject, or just prod a worker that's gone quiet.

**Broadcasting to every worker at once:**

- `broadcast(body, retain=False, file_path=None)` — a standing instruction
  or one-off announcement, delivered to every worker. Pass `retain=true`
  for something that should still apply to a worker that connects (or
  reconnects) later — it'll get it immediately without you resending
  anything; leave it `false` for a one-off that's only relevant right now.

**Knowing who's around:**

- `list_workers()` — `{worker_id, status, ts}` for every worker seen so
  far. A worker announces itself on connect and keeps refreshing that
  automatically, so you don't need to ask it to check in; `ts` not
  advancing for a while (a few multiples of its heartbeat interval, default
  60s) means it's gone quiet, and an unclean disconnect flips it to
  `"offline"` on its own. Check this before assuming a worker will act on
  something you send it.

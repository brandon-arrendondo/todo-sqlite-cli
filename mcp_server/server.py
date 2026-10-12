"""MCP server wrapping todo-sqlite-cli.

Database resolution (first match wins):
  1. TODO_SQLITE_CLI_DB environment variable
  2. Walk-up from cwd looking for a .todo-sqlite-cli marker file
  3. Exit 1

Binary resolution:
  1. TODO_SQLITE_CLI_BIN environment variable
  2. `todo-sqlite-cli` on PATH
"""

import json
import os
import subprocess
import threading
from functools import wraps
from typing import Annotated

from mcp.server import MCPServer
from mcp.server.mcpserver.exceptions import ToolError

import cli_ops

BIN = os.environ.get("TODO_SQLITE_CLI_BIN", "todo-sqlite-cli")

mcp = MCPServer("todo-sqlite-cli")

# Optional MQTT coordinator/worker sync (see README.md "MQTT sync"). Loaded
# lazily so a standalone deployment never needs the `paho-mqtt` dependency.
_mqtt = None
_mqtt_config = None
if os.environ.get("TODO_SQLITE_CLI_MQTT_CONFIG"):
    import mqtt_service

    _mqtt_config = mqtt_service.load_config()
    if _mqtt_config.mode == "coordinator":
        _mqtt = mqtt_service.CoordinatorService(_mqtt_config, run=lambda *a: _run(*a))
    elif _mqtt_config.mode == "worker":
        _mqtt = mqtt_service.WorkerService(_mqtt_config, run=lambda *a: _run(*a))
    else:
        raise RuntimeError(
            f"unknown MQTT mode '{_mqtt_config.mode}' (expected coordinator|worker)"
        )


# SDK v2 runs synchronous handlers in worker threads. Keep tool operations
# serialized as in v1: writes include a CLI mutation, a follow-up read, and
# sometimes snapshot publication. MQTT background callbacks retain their own locks.
_tool_lock = threading.Lock()


def _tool():
    """Register a serialized tool and preserve expected CLI/MQTT errors."""
    def decorator(func):
        @wraps(func)
        def invoke(*args, **kwargs):
            with _tool_lock:
                try:
                    return func(*args, **kwargs)
                except (RuntimeError, OSError, json.JSONDecodeError) as error:
                    raise ToolError(str(error)) from error
        return mcp.tool()(invoke)
    return decorator


def _mqtt_tool(*modes: str):
    """Like @_tool(), but only actually registers the tool when MQTT sync
    is configured for one of `modes` — an unregistered tool costs the agent
    nothing, whereas a registered-but-unusable one burns schema tokens on
    every request in every deployment that never uses MQTT sync at all.
    """

    def decorator(func):
        if _mqtt is not None and _mqtt_config.mode in modes:
            return _tool()(func)
        return func

    return decorator


def _run(*args: str) -> str:
    """Run the CLI, raise RuntimeError on non-zero exit, return stdout.

    In coordinator/worker mode, every invocation is pinned to that role's
    specific db file (the master db, or the local replica) via `--db`,
    rather than relying on ambient env/marker-file resolution.
    """
    db_override = None
    if _mqtt_config is not None:
        db_override = (
            _mqtt_config.db_path
            if _mqtt_config.mode == "coordinator"
            else _mqtt_config.worker_db_path
        )
    full_args = (["--db", db_override] if db_override else []) + list(args)
    result = subprocess.run([BIN, *full_args], capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or f"CLI exited {result.returncode}")
    return result.stdout.strip()


# ---------------------------------------------------------------------------
# Read commands
# ---------------------------------------------------------------------------


@_tool()
def list_tasks(
    status: str = "active",
    tags: list[str] | None = None,
    project_name: str | None = None,
    limit: int | None = None,
    since: str | None = None,
    unblocked: bool = False,
    compact: bool = False,
) -> str:
    """List task inventory as {"tasks": [...]} JSON; omit limit for all matching tasks.

    status: pending | partial | in-progress | done | rejected | active | all.
    tags: require ALL tags; project_name: exact project; limit: cap rows.
    since: created_at >= date (YYYY-MM-DD or RFC3339).
    unblocked: exclude tasks with unmet dependencies.
    compact: omit details and timestamps; fetch full data with show_task.
    Each task includes id, uuid and project_name; use uuid if IDs collide.
    """
    args = ["list", "--status", status, "--format", "json"]
    for tag in tags or []:
        args += ["--tag", tag]
    if project_name:
        args += ["--project-name", project_name]
    if limit is not None:
        args += ["--limit", str(limit)]
    if since:
        args += ["--since", since]
    if unblocked:
        args += ["--unblocked"]
    result = _run(*args)
    if not compact:
        return result
    listing = json.loads(result)
    listing["tasks"] = [_compact_task(task) for task in listing["tasks"]]
    return json.dumps(listing, separators=(",", ":"), ensure_ascii=False)


def _compact_task(task: dict) -> dict:
    """Keep selection, identity and ownership fields without the task body."""
    fields = ("id", "uuid", "title", "status", "priority", "project_name", "tags",
              "blocked", "depends_on", "is_gate", "location", "implementation_client")
    return {key: task[key] for key in fields if key in task}


@_tool()
def next_task(project_name: str | None = None) -> str:
    """Return one task as JSON, or empty string if none: oldest in-progress,
    then oldest unblocked partial, then highest-priority unblocked pending.
    project_name: exact project; set it on a multi-project coordinator DB
    to avoid selecting another project's work.
    """
    args = ["next", "--json"]
    if project_name:
        args += ["--project-name", project_name]
    return _run(*args)


@_tool()
def show_task(id: int | str) -> str:
    """Return full task details as a bare JSON object.
    IDs accept display IDs or full UUIDs. Ambiguity errors list every id/uuid;
    retry with the full UUID.
    """
    return _run("show", str(id), "--format", "json")


@_tool()
def show_tasks(ids: list[int | str]) -> str:
    """Read full task bodies for planning in one call (up to 50 IDs/UUIDs).
    Prefer UUIDs when display IDs collide. Returns tasks in input order and
    per-reference errors; successful reads survive another reference failing.
    """
    if len(ids) > 50:
        raise RuntimeError("show_tasks accepts at most 50 references; split the request")
    tasks, errors = [], []
    for reference in ids:
        try:
            tasks.append(json.loads(_run("show", str(reference), "--format", "json")))
        except (RuntimeError, OSError, json.JSONDecodeError) as error:
            errors.append({"id": reference, "error": str(error)})
    return json.dumps({"tasks": tasks, "errors": errors}, separators=(",", ":"))


@_tool()
def export_todo() -> str:
    """Export ALL active tasks (in-progress, partial, pending) as {"tasks": [...]}.
    Use filtered list_tasks or next_task for smaller responses.
    """
    return _run("export-todo", "--format", "json")


@_tool()
def export_completed(
    since: str | None = None,
    until: str | None = None,
) -> str:
    """Export completed tasks as {"completed": [{"date": ..., "tasks": [...]}, ...]},
    newest dates first. since: inclusive completed_at lower bound; until:
    exclusive upper bound. Dates accept YYYY-MM-DD or RFC3339.
    """
    args = ["export-completed", "--format", "json"]
    if since:
        args += ["--since", since]
    if until:
        args += ["--until", until]
    return _run(*args)


# ---------------------------------------------------------------------------
# Write commands
# ---------------------------------------------------------------------------


def _dispatch_write(op: str, args: dict) -> str:
    """Apply a write op directly (standalone/coordinator) or, in worker
    mode, submit it to the coordinator over MQTT and return immediately
    with a pending request id instead of a task.
    """
    if _mqtt is not None and _mqtt_config.mode == "worker":
        return json.dumps(_mqtt.submit(op, args))
    result = cli_ops.apply_op(_run, op, args)
    if _mqtt is not None and _mqtt_config.mode == "coordinator":
        _mqtt.publish_snapshot()
    return result


@_tool()
def add_task(
    title: str,
    details: str | None = None,
    tags: list[str] | None = None,
    priority: int = 3,
    depends_on: list[int | str] | None = None,
    start: bool = False,
    location: str | None = None,
    related: list[int | str] | None = None,
    implementation_client: str | None = None,
    project_name: str | None = None,
) -> str:
    """Create a task; return its JSON, including id, uuid and project_name.
    title: required summary; details: body; tags: strings; priority: 1–5
    (1 highest). depends_on: blocking IDs/UUIDs; related: mutual nonblocking
    IDs/UUIDs. start: immediately in-progress. location: work site.
    implementation_client: optional MQTT claimant, usually left unset;
    project_name: project.
    IDs accept display IDs or full UUIDs. Ambiguity errors list every id/uuid;
    retry with the full UUID.
    Worker mode submits to the coordinator; it does not write or delete locally.
    In worker mode, returns a pending request_id; poll check_request for the outcome.
    """
    return _dispatch_write(
        "add",
        dict(
            title=title,
            details=details,
            tags=tags,
            priority=priority,
            depends_on=depends_on,
            start=start,
            location=location,
            related=related,
            implementation_client=implementation_client,
            project_name=project_name,
        ),
    )


@_tool()
def start_task(id: int | str, force: bool = False) -> str:
    """Start a task; return updated JSON. Pauses current in-progress work
    to partial. force allows multiple in-progress tasks and bypasses dependencies.
    IDs accept display IDs or full UUIDs. Ambiguity errors list every id/uuid;
    retry with the full UUID.
    Worker mode submits to the coordinator; it does not write or delete locally.
    In worker mode, returns a pending request_id; poll check_request for the outcome.
    """
    return _dispatch_write("start", dict(id=id, force=force))


@_tool()
def stop_task(id: int | str) -> str:
    """Pause an in-progress task to partial; return updated JSON.
    IDs accept display IDs or full UUIDs. Ambiguity errors list every id/uuid;
    retry with the full UUID.
    Worker mode submits to the coordinator; it does not write or delete locally.
    In worker mode, returns a pending request_id; poll check_request for the outcome.
    """
    return _dispatch_write("stop", dict(id=id))


@_tool()
def revert_task(id: int | str) -> str:
    """Move a task to pending and clear started_at; return updated JSON.
    IDs accept display IDs or full UUIDs. Ambiguity errors list every id/uuid;
    retry with the full UUID.
    Worker mode submits to the coordinator; it does not write or delete locally.
    In worker mode, returns a pending request_id; poll check_request for the outcome.
    """
    return _dispatch_write("revert", dict(id=id))


@_tool()
def done_task(id: int | str, rejected: bool = False) -> str:
    """Mark done idempotently; return updated JSON. rejected marks rejected
    instead, records completed_at, and does NOT unblock dependents.
    IDs accept display IDs or full UUIDs. Ambiguity errors list every id/uuid;
    retry with the full UUID.
    Worker mode submits to the coordinator; it does not write or delete locally.
    In worker mode, returns a pending request_id; poll check_request for the outcome.
    """
    return _dispatch_write("done", dict(id=id, rejected=rejected))


@_tool()
def edit_task(
    id: int | str,
    title: str | None = None,
    append_details: str | None = None,
    details: str | None = None,
    clear_details: bool = False,
    priority: int | None = None,
    add_tags: list[str] | None = None,
    rm_tags: list[str] | None = None,
    add_deps: list[int | str] | None = None,
    rm_deps: list[int | str] | None = None,
    location: str | None = None,
    clear_location: bool = False,
    add_related: list[int | str] | None = None,
    rm_related: list[int | str] | None = None,
    implementation_client: str | None = None,
    clear_implementation_client: bool = False,
    project_name: str | None = None,
    clear_project_name: bool = False,
) -> str:
    """Edit supplied fields; omitted fields stay unchanged. Return updated JSON.
    Use append_details for progress notes; it appends with a newline; details REPLACES
    the entire body. clear_details removes it. add_tags/rm_tags modify tags;
    add_deps/rm_deps modify blocking links; add_related/rm_related modify
    mutual nonblocking links (no self-links). All link values accept IDs/UUIDs.
    location, implementation_client and project_name set those fields;
    their clear_* flags unset them and conflict with setting the same field.
    title and priority replace those values.
    IDs accept display IDs or full UUIDs. Ambiguity errors list every id/uuid;
    retry with the full UUID.
    Worker mode submits to the coordinator; it does not write or delete locally.
    In worker mode, returns a pending request_id; poll check_request for the outcome.
    """
    return _dispatch_write(
        "edit",
        dict(
            id=id,
            title=title,
            append_details=append_details,
            details=details,
            clear_details=clear_details,
            priority=priority,
            add_tags=add_tags,
            rm_tags=rm_tags,
            add_deps=add_deps,
            rm_deps=rm_deps,
            location=location,
            clear_location=clear_location,
            add_related=add_related,
            rm_related=rm_related,
            implementation_client=implementation_client,
            clear_implementation_client=clear_implementation_client,
            project_name=project_name,
            clear_project_name=clear_project_name,
        ),
    )


@_tool()
def rm_task(id: int | str) -> str:
    """Permanently delete a task and its tags/dependency edges; return {"deleted": id}.
    IDs accept display IDs or full UUIDs. Ambiguity errors list every id/uuid;
    retry with the full UUID.
    Worker mode submits to the coordinator; it does not write or delete locally.
    In worker mode, returns a pending request_id; poll check_request for the outcome.
    """
    return _dispatch_write("rm", dict(id=id))


@_tool()
def renumber_task(id: int | str, new_id: int, force: bool = False) -> str:
    """Change a display ID; return updated JSON. UUID and dependency identity
    stay unchanged. new_id must be positive. force permits an existing ID,
    creating a collision; use only for deliberate renumbering.
    An ambiguous plain ID is rejected; pass the full UUID.
    IDs accept display IDs or full UUIDs. Ambiguity errors list every id/uuid;
    retry with the full UUID.
    Worker mode submits to the coordinator; it does not write or delete locally.
    In worker mode, returns a pending request_id; poll check_request for the outcome.
    """
    return _dispatch_write("renumber", dict(id=id, new_id=new_id, force=force))


# ---------------------------------------------------------------------------
# Optional MQTT coordinator/worker sync commands
# ---------------------------------------------------------------------------


@_mqtt_tool("coordinator")
def poll_coordinator() -> str:
    """One fleet poll: drain messages; list pending write requests and workers.
    Returns messages, pending, workers, deadman, fleet_state, link and
    channel_errors. Errors invalidate empty channels; inspect link health.
    Pending requests are not approved or rejected. Git relay checks remain separate.
    """
    return _mqtt.poll_coordinator()


@_mqtt_tool("worker")
def poll_worker(request_ids: list[str] | None = None) -> str:
    """Drain assignments, direct messages and broadcasts; check supplied request_ids.
    Returns all three arrays, requests keyed by ID, shared fleet_state/coordinator/
    link and channel_errors. Inspect errors and health before trusting silence;
    items remain visible even if another channel errors. Drains consume messages
    and preserve read receipts and superseded flags. This grants no assignment:
    wait for the coordinator, read full task details, and keep latest_directive
    checks before irreversible steps. report_state remains explicit.
    """
    return _mqtt.poll_worker(request_ids)


@_mqtt_tool("coordinator")
def list_pending_requests() -> str:
    """Coordinator only. List worker requests awaiting approve_request/
    reject_request, as {"pending": [{request_id, op, args, worker_id, ts}, ...]}.
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("list_pending_requests requires MQTT coordinator mode")
    return _mqtt.list_pending()


@_mqtt_tool("coordinator")
def approve_request(request_id: str) -> str:
    """Coordinator only. Apply a pending worker request to the master db,
    publish the result and a fresh state snapshot, and return the updated
    task as JSON (or a deletion confirmation for an 'rm' request).
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("approve_request requires MQTT coordinator mode")
    return _mqtt.approve(request_id)


@_mqtt_tool("coordinator")
def reject_request(request_id: str, reason: str | None = None) -> str:
    """Coordinator only. Reject a pending worker request without touching
    the master db. The worker's check_request(request_id) will report
    status 'rejected' with this reason.
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("reject_request requires MQTT coordinator mode")
    return _mqtt.reject(request_id, reason)


@_mqtt_tool("worker")
def check_request(request_id: str) -> str:
    """Worker only. Poll the outcome of a request id returned by a write
    tool: {"status": "pending"} / {"status": "approved", "task": {...}} /
    {"status": "rejected", "error": "..."}, each plus the fleet_state,
    coordinator and link keys (see fleet_state).
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("check_request requires MQTT worker mode")
    return json.dumps(_mqtt.check(request_id))


@_mqtt_tool("worker")
def sync_state() -> str:
    """Worker only. Report freshness of the local read replica. The
    replica is kept current automatically by a background subscriber, so
    this is just a freshness check, not something that needs to be called
    before every read.

    Returns {synced, last_synced, fleet_state, coordinator, link}.
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("sync_state requires MQTT worker mode")
    return json.dumps(_mqtt.sync_state())


@_mqtt_tool("coordinator")
def assign_task(
    worker_id: str,
    body: str,
    task_id: int | str | None = None,
    kind: str | None = None,
    file_path: str | None = None,
) -> str:
    """Coordinator only. Publish a work assignment to one worker's own
    assign topic — distinct from send_message: this is specifically "here
    is what to work on next," polled by the worker via check_assignments().

    task_id: optional task (display id or uuid) this assignment refers to.
        When given, it's resolved against the master db and the assignment
        becomes a sequenced directive: it carries task_uuid, a per-(worker,
        task) seq, and kind, so the worker can tell a newer directive for
        the same task superseded this one.
    kind: approve | hold | go | info (default "go"; requires task_id).
        A later hold supersedes an earlier go/approve for the same task.
    file_path: see send_message.

    Returns {"message_id", "status": "sent", task_id?, task_uuid?, seq?,
    kind?, warning?}. warning is set when a go/approve is sent while the
    fleet state isn't active (it's still sent; hold/info never warn).
    Delivery and read receipts show up in list_sent().
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("assign_task requires MQTT coordinator mode")
    return _mqtt.assign(worker_id, body, task_id=task_id, file_path=file_path, kind=kind)


@_mqtt_tool("worker")
def check_assignments() -> str:
    """Worker only. Drain work assignments from the coordinator since the
    last call, as {"assignments": [{message_id, from, task_id?, task_uuid?,
    seq?, kind?, superseded?, body, file_path?, ts}, ...], fleet_state,
    coordinator, link}. See check_messages for superseded, the fleet_state/
    coordinator/link keys, and the stale-link error.
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("check_assignments requires MQTT worker mode")
    return _mqtt.check_assignments()


@_mqtt_tool("worker")
def report_state(state: str) -> str:
    """Worker only. Report a work-state string (e.g. idle/busy/blocked —
    whatever convention the fleet agrees on) alongside your online
    presence, refreshed immediately rather than waiting for the next
    heartbeat. Visible to the coordinator via list_workers()'s
    "work_state" field.

    Returns {"worker_id": ..., "work_state": ..., fleet_state, coordinator,
    link}.
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("report_state requires MQTT worker mode")
    return json.dumps(_mqtt.report_state(state))


@_mqtt_tool("worker")
def fleet_state() -> str:
    """Worker only. The current fleet state and link health — the same
    three keys every worker MQTT tool reply already carries:

    fleet_state: {state: active|draining|paused, note, set_at, set_by,
        source, instruction}. active: work normally. draining/paused:
        follow `instruction` literally. source "default" means nothing is
        retained on the broker, which counts as active; source
        "coordinator-offline" means the coordinator has been gone long
        enough that this worker derived draining/paused on its own (with
        `since`, and the real retained state under `retained`).
    coordinator: {status: online|offline|unknown, last_seen, age_s}.
    link: {connected, last_rx_at, age_s, stale, stale_since,
        denied_subscriptions, error} — this node's own broker link. error
        is set when the broker ACL denies a subscription this version
        needs.
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("fleet_state requires MQTT worker mode")
    return json.dumps(_mqtt.fleet_state())


@_mqtt_tool("worker")
def latest_directive(task_id: int | str) -> str:
    """Worker only. The effective (newest approve/hold/go) coordinator
    directive for one task. Re-check this immediately before any
    irreversible step (e.g. a push to main).

    task_id: display id or uuid, resolved through this worker's replica
        (so a display id that renumber_task moved to another task never
        inherits the old task's directive). An id the replica can't
        resolve is an error.

    Returns {task_id, task_uuid, effective: {kind, seq, message_id, body,
    ts, channel, task_id, task_uuid} | null, ok_to_act, draining,
    fleet_state, coordinator, link}. ok_to_act is true only when the
    effective kind is go/approve, the fleet isn't paused, and the link is
    fresh. draining: true means finish the current task only; don't pick
    up a new one. Raises an error on a stale link, or when the broker ACL
    denies a subscription (link.error). Don't act then.
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("latest_directive requires MQTT worker mode")
    return _mqtt.latest_directive(task_id)


@_mqtt_tool("coordinator", "worker")
def send_message(
    body: str,
    worker_id: str | None = None,
    file_path: str | None = None,
    task_id: int | str | None = None,
    kind: str | None = None,
) -> str:
    """Send a free-form direct message (up to 4096 chars by default,
    configurable via the MQTT config's message_max_chars).

    worker_id: coordinator only, required — which worker to message.
        Ignored (message always goes to the coordinator) in worker mode.
    file_path: optional local file to attach (path on this node's own
        filesystem). Rejected if it exceeds the MQTT config's
        max_file_bytes (default 1 MB). The recipient gets a local file
        path back from check_messages(), not raw file content.

    task_id / kind: coordinator only — make this a sequenced directive
        for a task, exactly as in assign_task, except kind defaults to
        "info" (an info never supersedes a go/hold). Ignored in worker mode.

    Returns {"message_id": ..., "status": "sent", ...}. On the coordinator,
    list_sent() shows whether the worker has received and read it (workers
    older than receipts support never ack, so theirs stay "sent").
    """
    if _mqtt is None:
        raise RuntimeError("send_message requires MQTT coordinator or worker mode")
    if _mqtt_config.mode == "coordinator":
        if not worker_id:
            raise RuntimeError("send_message requires worker_id in coordinator mode")
        return _mqtt.send_message(worker_id, body, file_path, task_id=task_id, kind=kind)
    return _mqtt.send_message(body, file_path)


@_mqtt_tool("coordinator", "worker")
def check_messages() -> str:
    """Drain direct messages addressed to this node since the last call —
    reading consumes the queue, so a message is only ever returned once.

    Coordinator: messages from any worker, each item carrying its own
    "from" (the sending worker's client id).
    Worker: messages from the coordinator addressed to this worker.

    Returns {"messages": [{message_id, from, to, body, file_path?, ts,
    task_id?, task_uuid?, seq?, kind?, superseded?}, ...], fleet_state,
    link} (a worker's reply also carries coordinator). superseded: true
    means a newer approve/hold/go for the same task has arrived — act on
    latest_directive(task_id), not on this item.

    If this node's broker link is stale (disconnected, or no broker
    traffic for several heartbeat intervals), an empty drain is an error
    rather than an empty list; a non-empty one adds link_stale_since.
    """
    if _mqtt is None:
        raise RuntimeError("check_messages requires MQTT coordinator or worker mode")
    return _mqtt.check_messages()


@_mqtt_tool("coordinator")
def broadcast(body: str, retain: bool = False, file_path: str | None = None) -> str:
    """Coordinator only. Publish a message to every worker at once.

    retain: if true, the message is retained by the broker — a worker that
        connects (or reconnects) later receives it immediately without the
        coordinator needing to resend it. Use this for standing instructions;
        leave it false for one-off announcements.
    file_path: see send_message.

    Returns {"message_id": ..., "status": "sent", "retained": bool}.
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("broadcast requires MQTT coordinator mode")
    return _mqtt.broadcast(body, retain, file_path)


@_mqtt_tool("coordinator")
def delete_broadcast(message_id: str) -> str:
    """Coordinator only. Clear a retained broadcast (one previously sent
    with retain=True) so a worker that connects or reconnects later no
    longer receives it. Has no effect on workers that already drained it,
    and no effect if that broadcast wasn't retained in the first place.

    message_id: the id returned by the original broadcast() call.

    Returns {"message_id": ..., "status": "deleted"}.
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("delete_broadcast requires MQTT coordinator mode")
    return _mqtt.delete_broadcast(message_id)


@_mqtt_tool("worker")
def check_broadcasts() -> str:
    """Worker only. Drain broadcast messages from the coordinator since the
    last call (including any retained broadcast received on connect).

    Returns {"broadcasts": [{message_id, from, body, retain, file_path?,
    ts}, ...], fleet_state, coordinator, link} (plus link_stale_since on a
    stale link — an empty drain is not an error here).
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("check_broadcasts requires MQTT worker mode")
    return _mqtt.check_broadcasts()


@_mqtt_tool("coordinator")
def list_workers() -> str:
    """Coordinator only. Current presence snapshot from every worker seen
    so far, built from an immediate "online" announcement plus a periodic
    heartbeat each worker publishes (and an automatic "offline" the broker
    publishes on an unclean disconnect). There's no fixed staleness cutoff
    here — judge a worker gone if its ts hasn't advanced in a few multiples
    of its heartbeat interval.

    Returns {"workers": [{worker_id, status, work_state, ts, age_s}, ...],
    deadman, link}. work_state is whatever a worker last passed to report_state()
    (null if it never has); age_s is seconds since its last presence
    update; link is this coordinator's own broker link health.
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("list_workers requires MQTT coordinator mode")
    return _mqtt.list_workers()


@_mqtt_tool("coordinator")
def list_sent(worker_id: str | None = None, unread_only: bool = False) -> str:
    """Coordinator only. Messages and assignments this coordinator has
    sent (the most recent 1000), with delivery receipts.

    worker_id: only those sent to this worker.
    unread_only: only those not yet read.

    Returns {"sent": [{message_id, worker_id, channel: message|assignment,
    task_id, task_uuid, seq, kind, sent_at, delivered_at, read_at,
    status: sent|delivered|read}, ...], link}. delivered = the worker's
    MCP server received it; read = its agent drained it via
    check_messages/check_assignments. Workers too old to send receipts
    stay at "sent".
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("list_sent requires MQTT coordinator mode")
    return _mqtt.list_sent(worker_id, unread_only)


@_mqtt_tool("coordinator")
def set_fleet_state(state: str, note: str | None = None) -> str:
    """Coordinator only. Set the fleet-wide state, retained on the broker
    so every worker sees it on its next poll (and on reconnect).

    state: active | draining | paused. draining: workers finish their
        current task, then pause. paused: workers checkpoint, report
        state, and stop scheduling wakeups. The coordinator should honour
        it too: no new go/approve directives or subagent launches.
    note: optional free-form reason, shown to workers alongside it.

    Going back to active always takes an explicit set_fleet_state: the
    dead-man switch never resumes the fleet by itself.

    Returns {state, note, set_at, set_by, source, instruction, deadman,
    link, warning?}.
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("set_fleet_state requires MQTT coordinator mode")
    return _mqtt.set_fleet_state(state, note)


@_mqtt_tool("coordinator")
def checkin(note: str | None = None) -> str:
    """Coordinator only. Record an operator check-in, resetting the
    dead-man timer. It does NOT resume a drained or paused fleet (use
    set_fleet_state for that). Call it only when the operator explicitly
    asks you to: calling it on your own defeats the dead-man switch.
    Normally the operator's prompt hook touches the file instead.

    Returns {deadman, fleet_state}.
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("checkin requires MQTT coordinator mode")
    return _mqtt.checkin(note)


@_mqtt_tool("coordinator")
def get_fleet_state() -> str:
    """Coordinator only. The current fleet state as {state, note, set_at,
    set_by, source, instruction, deadman, link}. source "default" means
    nothing is retained on the broker, which counts as active. set_by
    "deadman" means the dead-man switch set it.

    deadman: {enabled, checkin_file, last_checkin, timer_from, drains_at,
    pauses_at}. Off unless deadman_drain_after_s is configured. When on,
    the coordinator's server process itself drains, then
    pauses, the fleet when no check-in (the checkin file's mtime, touched
    by the operator's prompt hook) has happened for the configured time.
    It only ever moves toward draining/paused, and never resumes.
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("get_fleet_state requires MQTT coordinator mode")
    return _mqtt.get_fleet_state()


def main() -> None:
    mcp.run()


if __name__ == "__main__":
    main()

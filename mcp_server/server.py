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
from typing import Annotated

from mcp.server.fastmcp import FastMCP

import cli_ops

BIN = os.environ.get("TODO_SQLITE_CLI_BIN", "todo-sqlite-cli")

mcp = FastMCP("todo-sqlite-cli")

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
        _mqtt = mqtt_service.WorkerService(_mqtt_config)
    else:
        raise RuntimeError(
            f"unknown MQTT mode '{_mqtt_config.mode}' (expected coordinator|worker)"
        )


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


@mcp.tool()
def list_tasks(
    status: str = "active",
    tags: list[str] | None = None,
    project_name: str | None = None,
    limit: int | None = None,
    since: str | None = None,
    unblocked: bool = False,
) -> str:
    """List tasks as JSON.

    status: pending | partial | in-progress | done | rejected | active | all
    tags: filter to tasks carrying ALL listed tags
    project_name: filter to tasks in this project (exact match)
    limit: cap number of rows
    since: only tasks with created_at >= DATE (YYYY-MM-DD or RFC3339)
    unblocked: only include tasks with no unmet dependencies

    Returns {"tasks": [...]} JSON.
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
    return _run(*args)


@mcp.tool()
def next_task(project_name: str | None = None) -> str:
    """Return the single highest-priority task to work on next as JSON.

    Order: oldest in-progress → oldest unblocked partial → highest-priority
    unblocked pending. Returns a bare task object, or empty string if none.

    project_name: scope to just this project (exact match) — pass yours on
        a coordinator db spanning several projects, or it may hand back
        another project's task if it outranks everything in your own.
    """
    args = ["next", "--json"]
    if project_name:
        args += ["--project-name", project_name]
    return _run(*args)


@mcp.tool()
def show_task(id: int) -> str:
    """Show full details for a task as JSON (bare task object)."""
    return _run("show", str(id), "--format", "json")


@mcp.tool()
def export_todo() -> str:
    """Export all active (in-progress + partial + pending) tasks as JSON.

    Returns {"tasks": [...]} JSON. Equivalent to list_tasks but always
    covers all active statuses with no filters.
    """
    return _run("export-todo", "--format", "json")


@mcp.tool()
def export_completed(
    since: str | None = None,
    until: str | None = None,
) -> str:
    """Export completed tasks grouped by date as JSON.

    since: inclusive lower bound on completed_at (YYYY-MM-DD or RFC3339)
    until: exclusive upper bound on completed_at (YYYY-MM-DD or RFC3339)

    Returns {"completed": [{"date": "YYYY-MM-DD", "tasks": [...]}, ...]}
    descending by date.
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


@mcp.tool()
def add_task(
    title: str,
    details: str | None = None,
    tags: list[str] | None = None,
    priority: int = 3,
    depends_on: list[int] | None = None,
    start: bool = False,
    location: str | None = None,
    related: list[int] | None = None,
    implementation_client: str | None = None,
    project_name: str | None = None,
) -> str:
    """Add a new task. Returns the new task as JSON.

    title: short summary (required)
    details: longer free-form description
    tags: list of tag strings
    priority: 1 (highest) to 5 (lowest), default 3
    depends_on: list of task IDs this task is blocked by
    start: immediately move to in-progress
    location: where this work must be done (e.g. a specific node/site)
    related: list of task IDs to link as related work (mutual — also shows
        up on those tasks). Not blocking, unlike depends_on.
    implementation_client: MQTT client id claiming this task (optional
        coordinator/worker sync feature) — usually left unset.
    project_name: which project this task belongs to, for a coordinator db
        spanning several projects.

    In worker mode (optional MQTT sync), this submits a request to the
    coordinator instead of writing locally, and returns
    {"request_id": ..., "status": "pending"} — poll check_request(id).
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


@mcp.tool()
def start_task(id: int, force: bool = False) -> str:
    """Move a task to in-progress. Returns the updated task as JSON.

    Automatically pauses any current in-progress task to 'partial'.
    force: allow multiple in-progress tasks and skip dependency check.

    In worker mode (optional MQTT sync), this submits a request to the
    coordinator instead of writing locally, and returns
    {"request_id": ..., "status": "pending"} — poll check_request(id).
    """
    return _dispatch_write("start", dict(id=id, force=force))


@mcp.tool()
def stop_task(id: int) -> str:
    """Pause an in-progress task (moves to 'partial'). Returns updated task as JSON.

    In worker mode (optional MQTT sync), this submits a request to the
    coordinator instead of writing locally, and returns
    {"request_id": ..., "status": "pending"} — poll check_request(id).
    """
    return _dispatch_write("stop", dict(id=id))


@mcp.tool()
def revert_task(id: int) -> str:
    """Move a task back to pending, clearing started_at. Returns updated task as JSON.

    In worker mode (optional MQTT sync), this submits a request to the
    coordinator instead of writing locally, and returns
    {"request_id": ..., "status": "pending"} — poll check_request(id).
    """
    return _dispatch_write("revert", dict(id=id))


@mcp.tool()
def done_task(id: int, rejected: bool = False) -> str:
    """Mark a task done. Idempotent. Returns the updated task as JSON.

    rejected: close the task as 'rejected' (declined / won't-do) instead of
        'done'. Records completed_at but does NOT unblock dependents.

    In worker mode (optional MQTT sync), this submits a request to the
    coordinator instead of writing locally, and returns
    {"request_id": ..., "status": "pending"} — poll check_request(id).
    """
    return _dispatch_write("done", dict(id=id, rejected=rejected))


@mcp.tool()
def edit_task(
    id: int,
    title: str | None = None,
    append_details: str | None = None,
    details: str | None = None,
    clear_details: bool = False,
    priority: int | None = None,
    add_tags: list[str] | None = None,
    rm_tags: list[str] | None = None,
    add_deps: list[int] | None = None,
    rm_deps: list[int] | None = None,
    location: str | None = None,
    clear_location: bool = False,
    add_related: list[int] | None = None,
    rm_related: list[int] | None = None,
    implementation_client: str | None = None,
    clear_implementation_client: bool = False,
    project_name: str | None = None,
    clear_project_name: bool = False,
) -> str:
    """Edit an existing task. Returns the updated task as JSON.

    Provide one or more fields to change; omitted fields are left as-is.

    For progress notes, use append_details — it adds text to the existing
    body with a newline separator, preserving prior context. Use details
    only when you actually want to REPLACE the entire body (it discards
    whatever was there before).

    location/clear_location: where the work must be done, or unset it
        (mutually exclusive).
    add_related/rm_related: link/unlink related work by task ID. Mutual —
        also updates the other task; rejects linking a task to itself.
    implementation_client/clear_implementation_client: MQTT client id
        claiming this task (optional coordinator/worker sync feature),
        or unset it (mutually exclusive).
    project_name/clear_project_name: which project this task belongs to,
        or unset it (mutually exclusive).

    In worker mode (optional MQTT sync), this submits a request to the
    coordinator instead of writing locally, and returns
    {"request_id": ..., "status": "pending"} — poll check_request(id).
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


@mcp.tool()
def rm_task(id: int) -> str:
    """Delete a task permanently. Cascades to tags and dependency edges.

    Returns a confirmation message with the deleted task ID.

    In worker mode (optional MQTT sync), this submits a request to the
    coordinator instead of deleting locally, and returns
    {"request_id": ..., "status": "pending"} — poll check_request(id).
    """
    return _dispatch_write("rm", dict(id=id))


# ---------------------------------------------------------------------------
# Optional MQTT coordinator/worker sync commands
# ---------------------------------------------------------------------------


@mcp.tool()
def list_pending_requests() -> str:
    """Coordinator only. List worker requests awaiting approve_request/
    reject_request, as {"pending": [{request_id, op, args, worker_id, ts}, ...]}.
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("list_pending_requests requires MQTT coordinator mode")
    return _mqtt.list_pending()


@mcp.tool()
def approve_request(request_id: str) -> str:
    """Coordinator only. Apply a pending worker request to the master db,
    publish the result and a fresh state snapshot, and return the updated
    task as JSON (or a deletion confirmation for an 'rm' request).
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("approve_request requires MQTT coordinator mode")
    return _mqtt.approve(request_id)


@mcp.tool()
def reject_request(request_id: str, reason: str | None = None) -> str:
    """Coordinator only. Reject a pending worker request without touching
    the master db. The worker's check_request(request_id) will report
    status 'rejected' with this reason.
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("reject_request requires MQTT coordinator mode")
    return _mqtt.reject(request_id, reason)


@mcp.tool()
def check_request(request_id: str) -> str:
    """Worker only. Poll the outcome of a request id returned by a write
    tool: {"status": "pending"} / {"status": "approved", "task": {...}} /
    {"status": "rejected", "error": "..."}.
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("check_request requires MQTT worker mode")
    return json.dumps(_mqtt.check(request_id))


@mcp.tool()
def sync_state() -> str:
    """Worker only. Report freshness of the local read replica. The
    replica is kept current automatically by a background subscriber, so
    this is just a freshness check, not something that needs to be called
    before every read.
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("sync_state requires MQTT worker mode")
    return json.dumps(_mqtt.sync_state())


@mcp.tool()
def assign_task(
    worker_id: str,
    body: str,
    task_id: int | None = None,
    file_path: str | None = None,
) -> str:
    """Coordinator only. Publish a work assignment to one worker's own
    assign topic — distinct from send_message: this is specifically "here
    is what to work on next," polled by the worker via check_assignments().

    task_id: optional task id this assignment refers to (informational —
        the worker still reads full detail via show_task).
    file_path: see send_message.

    Returns {"message_id": ..., "status": "sent"}.
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("assign_task requires MQTT coordinator mode")
    return _mqtt.assign(worker_id, body, task_id, file_path)


@mcp.tool()
def check_assignments() -> str:
    """Worker only. Drain work assignments from the coordinator since the
    last call, as {"assignments": [{message_id, from, task_id?, body,
    file_path?, ts}, ...]}.
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("check_assignments requires MQTT worker mode")
    return _mqtt.check_assignments()


@mcp.tool()
def report_state(state: str) -> str:
    """Worker only. Report a work-state string (e.g. idle/busy/blocked —
    whatever convention the fleet agrees on) alongside your online
    presence, refreshed immediately rather than waiting for the next
    heartbeat. Visible to the coordinator via list_workers()'s
    "work_state" field.

    Returns {"worker_id": ..., "work_state": ...}.
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("report_state requires MQTT worker mode")
    return json.dumps(_mqtt.report_state(state))


@mcp.tool()
def send_message(
    body: str,
    worker_id: str | None = None,
    file_path: str | None = None,
) -> str:
    """Send a free-form direct message (up to 4096 chars by default,
    configurable via the MQTT config's message_max_chars).

    worker_id: coordinator only, required — which worker to message.
        Ignored (message always goes to the coordinator) in worker mode.
    file_path: optional local file to attach (path on this node's own
        filesystem). Rejected if it exceeds the MQTT config's
        max_file_bytes (default 1 MB). The recipient gets a local file
        path back from check_messages(), not raw file content.

    Returns {"message_id": ..., "status": "sent"}. Delivery is fire-and-
    forget from the sender's side — there's no read receipt; check with
    the recipient directly if you need confirmation.
    """
    if _mqtt is None:
        raise RuntimeError("send_message requires MQTT coordinator or worker mode")
    if _mqtt_config.mode == "coordinator":
        if not worker_id:
            raise RuntimeError("send_message requires worker_id in coordinator mode")
        return _mqtt.send_message(worker_id, body, file_path)
    return _mqtt.send_message(body, file_path)


@mcp.tool()
def check_messages() -> str:
    """Drain direct messages addressed to this node since the last call —
    reading consumes the queue, so a message is only ever returned once.

    Coordinator: messages from any worker, each item carrying its own
    "from" (the sending worker's client id).
    Worker: messages from the coordinator addressed to this worker.

    Returns {"messages": [{message_id, from, to, body, file_path?, ts}, ...]}.
    """
    if _mqtt is None:
        raise RuntimeError("check_messages requires MQTT coordinator or worker mode")
    return _mqtt.check_messages()


@mcp.tool()
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


@mcp.tool()
def check_broadcasts() -> str:
    """Worker only. Drain broadcast messages from the coordinator since the
    last call (including any retained broadcast received on connect).

    Returns {"broadcasts": [{message_id, from, body, retain, file_path?, ts}, ...]}.
    """
    if _mqtt is None or _mqtt_config.mode != "worker":
        raise RuntimeError("check_broadcasts requires MQTT worker mode")
    return _mqtt.check_broadcasts()


@mcp.tool()
def list_workers() -> str:
    """Coordinator only. Current presence snapshot from every worker seen
    so far, built from an immediate "online" announcement plus a periodic
    heartbeat each worker publishes (and an automatic "offline" the broker
    publishes on an unclean disconnect). There's no fixed staleness cutoff
    here — judge a worker gone if its ts hasn't advanced in a few multiples
    of its heartbeat interval.

    Returns {"workers": [{worker_id, status, work_state, ts}, ...]}.
    work_state is whatever a worker last passed to report_state() (null if
    it never has).
    """
    if _mqtt is None or _mqtt_config.mode != "coordinator":
        raise RuntimeError("list_workers requires MQTT coordinator mode")
    return _mqtt.list_workers()


if __name__ == "__main__":
    mcp.run()

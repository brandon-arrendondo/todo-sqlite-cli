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

BIN = os.environ.get("TODO_SQLITE_CLI_BIN", "todo-sqlite-cli")

mcp = FastMCP("todo-sqlite-cli")


def _run(*args: str) -> str:
    """Run the CLI, raise RuntimeError on non-zero exit, return stdout."""
    result = subprocess.run([BIN, *args], capture_output=True, text=True)
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
    limit: int | None = None,
    since: str | None = None,
) -> str:
    """List tasks as JSON.

    status: pending | partial | in-progress | done | active | all
    tags: filter to tasks carrying ALL listed tags
    limit: cap number of rows
    since: only tasks with created_at >= DATE (YYYY-MM-DD or RFC3339)

    Returns {"tasks": [...]} JSON.
    """
    args = ["list", "--status", status, "--format", "json"]
    for tag in tags or []:
        args += ["--tag", tag]
    if limit is not None:
        args += ["--limit", str(limit)]
    if since:
        args += ["--since", since]
    return _run(*args)


@mcp.tool()
def next_task() -> str:
    """Return the single highest-priority task to work on next as JSON.

    Order: oldest in-progress → oldest unblocked partial → highest-priority
    unblocked pending. Returns a bare task object, or empty string if none.
    """
    return _run("next", "--json")


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


@mcp.tool()
def add_task(
    title: str,
    details: str | None = None,
    tags: list[str] | None = None,
    priority: int = 3,
    depends_on: list[int] | None = None,
    start: bool = False,
) -> str:
    """Add a new task. Returns the new task as JSON.

    title: short summary (required)
    details: longer free-form description
    tags: list of tag strings
    priority: 1 (highest) to 5 (lowest), default 3
    depends_on: list of task IDs this task is blocked by
    start: immediately move to in-progress
    """
    args = ["add", title, "--priority", str(priority)]
    if details:
        args += ["--details", details]
    for tag in tags or []:
        args += ["--tag", tag]
    for dep in depends_on or []:
        args += ["--depends-on", str(dep)]
    if start:
        args.append("--start")
    new_id = int(_run(*args))
    return _run("show", str(new_id), "--format", "json")


@mcp.tool()
def start_task(id: int, force: bool = False) -> str:
    """Move a task to in-progress. Returns the updated task as JSON.

    Automatically pauses any current in-progress task to 'partial'.
    force: allow multiple in-progress tasks and skip dependency check.
    """
    args = ["start", str(id)]
    if force:
        args.append("--force")
    _run(*args)
    return _run("show", str(id), "--format", "json")


@mcp.tool()
def stop_task(id: int) -> str:
    """Pause an in-progress task (moves to 'partial'). Returns updated task as JSON."""
    _run("stop", str(id))
    return _run("show", str(id), "--format", "json")


@mcp.tool()
def revert_task(id: int) -> str:
    """Move a task back to pending, clearing started_at. Returns updated task as JSON."""
    _run("revert", str(id))
    return _run("show", str(id), "--format", "json")


@mcp.tool()
def done_task(id: int) -> str:
    """Mark a task done. Idempotent. Returns the updated task as JSON."""
    _run("done", str(id))
    return _run("show", str(id), "--format", "json")


@mcp.tool()
def edit_task(
    id: int,
    title: str | None = None,
    details: str | None = None,
    append_details: str | None = None,
    clear_details: bool = False,
    priority: int | None = None,
    add_tags: list[str] | None = None,
    rm_tags: list[str] | None = None,
    add_deps: list[int] | None = None,
    rm_deps: list[int] | None = None,
) -> str:
    """Edit an existing task. Returns the updated task as JSON.

    Provide one or more fields to change; omitted fields are left as-is.
    details replaces the existing body; append_details appends with a newline.
    """
    args = ["edit", str(id)]
    if title:
        args += ["--title", title]
    if details:
        args += ["--details", details]
    if append_details:
        args += ["--append-details", append_details]
    if clear_details:
        args.append("--clear-details")
    if priority is not None:
        args += ["--priority", str(priority)]
    for tag in add_tags or []:
        args += ["--add-tag", tag]
    for tag in rm_tags or []:
        args += ["--rm-tag", tag]
    for dep in add_deps or []:
        args += ["--add-dep", str(dep)]
    for dep in rm_deps or []:
        args += ["--rm-dep", str(dep)]
    _run(*args)
    return _run("show", str(id), "--format", "json")


@mcp.tool()
def rm_task(id: int) -> str:
    """Delete a task permanently. Cascades to tags and dependency edges.

    Returns a confirmation message with the deleted task ID.
    """
    _run("rm", str(id))
    return json.dumps({"deleted": id})


if __name__ == "__main__":
    mcp.run()

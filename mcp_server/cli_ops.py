"""Pure `todo-sqlite-cli` argument builders for the 7 write operations.

Shared by two callers that must build byte-identical CLI invocations:
`server.py`'s MCP tools (standalone/coordinator mode, applying directly) and
`mqtt_service.CoordinatorService.approve` (applying a worker's queued
request). Keeping the arg-building logic here, in one place with no MCP/MQTT
dependencies, is what keeps those two paths from drifting apart.
"""

import json

WRITE_OPS = ("add", "edit", "start", "stop", "done", "revert", "rm")


def add_args(
    *,
    title,
    details=None,
    tags=None,
    priority=3,
    depends_on=None,
    start=False,
    location=None,
    related=None,
    implementation_client=None,
):
    args = ["add", title, "--priority", str(priority)]
    if details:
        args += ["--details", details]
    for tag in tags or []:
        args += ["--tag", tag]
    for dep in depends_on or []:
        args += ["--depends-on", str(dep)]
    if start:
        args.append("--start")
    if location:
        args += ["--location", location]
    for r in related or []:
        args += ["--related", str(r)]
    if implementation_client:
        args += ["--implementation-client", implementation_client]
    return args


def edit_args(
    *,
    id,
    title=None,
    append_details=None,
    details=None,
    clear_details=False,
    priority=None,
    add_tags=None,
    rm_tags=None,
    add_deps=None,
    rm_deps=None,
    location=None,
    clear_location=False,
    add_related=None,
    rm_related=None,
    implementation_client=None,
    clear_implementation_client=False,
):
    args = ["edit", str(id)]
    if title:
        args += ["--title", title]
    if append_details:
        args += ["--append-details", append_details]
    if details:
        args += ["--details", details]
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
    if location:
        args += ["--location", location]
    if clear_location:
        args.append("--clear-location")
    for r in add_related or []:
        args += ["--add-related", str(r)]
    for r in rm_related or []:
        args += ["--rm-related", str(r)]
    if implementation_client:
        args += ["--implementation-client", implementation_client]
    if clear_implementation_client:
        args.append("--clear-implementation-client")
    return args


def start_args(*, id, force=False):
    args = ["start", str(id)]
    if force:
        args.append("--force")
    return args


def stop_args(*, id):
    return ["stop", str(id)]


def revert_args(*, id):
    return ["revert", str(id)]


def done_args(*, id, rejected=False):
    args = ["done", str(id)]
    if rejected:
        args.append("--rejected")
    return args


def rm_args(*, id):
    return ["rm", str(id)]


OP_BUILDERS = {
    "add": add_args,
    "edit": edit_args,
    "start": start_args,
    "stop": stop_args,
    "done": done_args,
    "revert": revert_args,
    "rm": rm_args,
}


def apply_op(run, op, args):
    """Apply one write op via `run` (a callable like server._run, raising on
    CLI failure) and return the resulting task as a JSON string (or a
    deletion confirmation for `rm`), exactly matching each direct MCP tool's
    current return shape.
    """
    if op == "add":
        new_id = int(run(*add_args(**args)))
        return run("show", str(new_id), "--format", "json")
    if op == "rm":
        run(*rm_args(**args))
        return json.dumps({"deleted": args["id"]})
    run(*OP_BUILDERS[op](**args))
    return run("show", str(args["id"]), "--format", "json")

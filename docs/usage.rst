CLI and database selection
==========================

``todo-sqlite-cli [--db PATH] [--json] COMMAND [ARGS...]`` runs without a
TTY or daemon. Use ``--help`` on a subcommand for its complete option set.
This page describes behavior that matters when scripting it.

Database selection
------------------

For normal database commands, first match wins:

1. ``--db PATH``.
2. Nonempty ``TODO_SQLITE_CLI_DB``.
3. The first ``.todo-sqlite-cli`` found by walking up from cwd.
4. Otherwise an error with exit 1.

A marker's first line is trimmed and used as its path. Relative paths are
resolved against the marker's directory; relative flag/environment paths
are resolved from cwd. An empty marker is an error, not a signal to continue
searching further up.

``init`` has separate resolution. With ``--db`` it creates that path and
writes no marker; ``--marker-dir`` is ignored. Without ``--db`` it creates
``todo-sqlite-cli.db`` and a marker in cwd or ``--marker-dir``; it does not
honor the environment or existing ancestor markers. Existing databases are
not overwritten. Absolute ``--marker-dir`` paths avoid ambiguities from a
relative directory name recorded in the marker.

``merge`` and ``git-merge-driver`` take explicit paths rather than normal
resolution. ``install-merge-driver`` uses normal resolution and requires the
database to be inside the current Git repository.

Opening a database uses a read/write SQLite connection, enables WAL and
foreign keys, and migrates older supported schemas. Even a reporting command
can change database metadata or schema. A newer unsupported schema is refused.
Use :doc:`merge-engine` for migration and merging constraints.

Identity and task selection
---------------------------

UUID is identity; the display ID is an alias. ``add`` allocates ``MAX(id)+1``.
Deleting the highest ID can allow reuse. Merges can retain duplicate aliases,
and ``renumber`` changes an alias. Use full UUIDs for durable references.
A duplicate numeric ID fails with a list of matches on stderr, including
for ``show``; use an unambiguous ID or full UUID. UUID prefixes are not supported.

``next`` excludes gates and optionally filters exact ``--project-name``:

1. Oldest in-progress task, ordered by started_at then display ID. This tier
   does not recheck dependencies.
2. Unblocked partial task, ordered by priority, started_at, then display ID.
3. Unblocked pending task, ordered by priority, created_at, then display ID.

Only dependencies in ``done`` unblock a task; ``rejected`` does not.
Empty selection exits 0 with empty text stdout or JSON ``null``. Blocked tasks
or gates can still remain; text mode includes a hint when open gates exist.

``list`` defaults to active statuses and orders in-progress, partial, pending,
then priority and creation time. Repeated tags AND together. ``--since`` filters
creation time, not modification time; ``--ids-only`` reports membership,
not field edits. ``--limit`` is applied before ``--unblocked`` filtering, so
a limited query may return fewer unblocked tasks than expected.

``start`` auto-pauses earlier in-progress tasks to partial unless ``--force``
is used; force also bypasses dependency checks. This operates across the
whole database, even when tasks have different project names. ``stop`` keeps
started_at; ``revert`` clears it. ``done`` is idempotent for the requested
terminal status. ``aging`` and ``cfd`` reconstruct trends from current row
timestamps, not an event log of every work episode.

Output and exits
----------------

``list`` and ``export-todo`` JSON use ``{"tasks": [...]}``; export-completed
uses date groups under ``completed``. Single-task commands generally emit a
bare task object; ``next`` can emit null. NDJSON is available on list/export commands. Output format flags
are command-specific; an explicit nondefault format can take precedence over
``--json``. The Git driver and driver installer retain text output even with
the global JSON flag.

Exit 0 means success, exit 1 a runtime user error, and exit 2 a runtime system
error or clap argument-parsing error. Errors are text on stderr even when
JSON output was requested. A merge can return success while recording
conflicts unless strict mode was requested; the Git driver returns nonzero
for hard conflicts after writing its usable result.

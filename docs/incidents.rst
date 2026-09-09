Incidents
=========

Real production incidents that shaped current safeguards in the merge
engine. Kept as a permanent record — the guard each incident produced only
makes sense in light of what it prevents.

.. contents:: Contents
   :local:
   :depth: 1

---------------------------------------------------------------------------

v2→v3 merge mass-duplicated tools_sqc's task history
-----------------------------------------------------

:Date: 2026-08-27
:Reporter: Claude Code session working in ``~/data-enterprise/tools_sqc`` (bench node)
:Repo affected: ``tools_sqc``'s ``todo-sqlite-cli.db``
:Severity: High — every pre-existing task in the shared DB was duplicated
   (618 of 620 tasks affected; 1238 rows after the merge instead of 620)
:Status: **Resolved.** ``git_merge_driver::run`` now calls
   ``db::require_matching_schema_versions`` on all three sides' *pre*-migration
   schema versions before opening any of them for real, and refuses the
   merge outright on a mismatch. See :doc:`merge-engine` for where this
   sits in the merge flow.

Summary
~~~~~~~

``tools_sqc``'s bench node had been running against an **old v2-schema**
local ``todo-sqlite-cli.db`` all session (no ``uuid`` column — ``id INTEGER
PRIMARY KEY AUTOINCREMENT``). A commit landed on ``origin/main`` with a DB
already migrated to the **v3 schema** (``uuid TEXT PRIMARY KEY``, ``id
INTEGER NOT NULL`` demoted to a display alias, real UUIDs already assigned
to every pre-existing task).

When the bench node ran ``git pull``, git's registered merge driver fired
to merge the bench node's v2 DB against origin's v3 DB. The result:
**every one of the 618 pre-existing tasks was duplicated** — same display
id, same title, two different UUIDs. ``todo-sqlite-cli doctor`` correctly
flagged this (``duplicate_display_ids: 618``), which is how it was caught
before push.

Root cause
~~~~~~~~~~

The v2→v3 migration was **not coordinated across nodes**. Each node's
``todo-sqlite-cli`` client auto-migrates a v2 database to v3 on first
v3-aware access, and that migration **mints a fresh random UUID for every
existing v2 row** with no shared "this v2 row = that other node's v3 row"
identity to anchor against. So:

- Origin's DB had already been migrated on some other node, assigning e.g.
  ``ce83cf74-1e5c-4f81-b1af-3d9a3e3688ae`` to task id=1 ("MEM30-C
  field-level free tracking").
- The bench node's DB was still plain v2 (no uuid column at all) at merge
  time.
- The merge driver, needing a UUID identity to merge by, minted its own
  fresh UUID for the bench node's id=1 (``50fe14af-32bb-4f54-8013-2a8ece3c3c43``,
  per ``doctor``'s output) instead of recognizing it as the same logical
  task as origin's ``ce83cf74...``.
- The merge then correctly unioned by UUID **as designed** — but since the
  two UUIDs for the same logical task never matched, the union produced
  two rows instead of a reconciled one.

This reproduced for **every** pre-existing task, because the bench node's
entire DB was still on the old schema — not a narrow "two nodes
independently created a task that happened to land on the same display
id" case (an expected, rare residual risk ``doctor`` already catches).
This was the *whole historical backlog* colliding at once because a
**local node never went through the v3 migration before merging against
an already-migrated remote**.

Evidence
~~~~~~~~

Confirming query (same logical task, two UUIDs after merge)::

    $ sqlite3 tools_sqc-mynode-pre-merge-v2schema.db "select id,title from tasks where id=1;"
    1|MEM30-C field-level free tracking

    $ sqlite3 tools_sqc-origin-pre-merge-v3schema.db "select id,uuid,title from tasks where id=1;"
    1|ce83cf74-1e5c-4f81-b1af-3d9a3e3688ae|MEM30-C field-level free tracking

    # After merge, doctor reports BOTH of these under id=1:
    #   id=1 uuid=50fe14af-32bb-4f54-8013-2a8ece3c3c43 title=MEM30-C field-level free tracking
    #   id=1 uuid=ce83cf74-1e5c-4f81-b1af-3d9a3e3688ae title=MEM30-C field-level free tracking

``doctor`` output sample from the corrupted merge (first few of 618 groups)::

    duplicate display ids (618) — show/edit by uuid to disambiguate:
      id=1 uuid=50fe14af-32bb-4f54-8013-2a8ece3c3c43 title=MEM30-C field-level free tracking
      id=1 uuid=ce83cf74-1e5c-4f81-b1af-3d9a3e3688ae title=MEM30-C field-level free tracking
      id=2 uuid=6856635f-2fb5-43f6-9d9d-cf8e6098013c title=MEM31-C ownership model
      id=2 uuid=9a10887b-48a6-4380-afa9-d64f5edfdb66 title=MEM31-C ownership model
      id=3 uuid=6eae9bcb-22e2-4246-bce8-f3f91a128f59 title=DCL13-C alias tracking
      id=3 uuid=aeffccf1-b96b-4a2d-9efe-21f64c0a522c title=DCL13-C alias tracking
      ... (615 more groups, essentially the entire pre-existing backlog)

Recovery
~~~~~~~~

Per the user's explicit instruction (not a unilateral decision — this was
a shared-DB incident and the user was asked before any destructive action
was taken):

1. Copied the corrupted merged DB out to evidence storage before touching
   anything.
2. Reset the bench node's local ``todo-sqlite-cli.db`` to the clean
   upstream copy (``origin/main``, ``doctor: clean``, 620 tasks) —
   discarding the corrupted merge result rather than attempting to
   hand-deduplicate 618 rows.
3. Re-applied the bench node's own in-session progress notes by hand,
   since those specific edits had only ever existed in the corrupted
   local DB and hadn't reached a clean push yet. Nothing else from the
   corrupted merge was carried forward.
4. Did **not** attempt to fix the merge driver or the v2→v3 migration path
   itself at the time — that became this repo's follow-up, resolved as
   noted above.

Fix
~~~

``db::peek_schema_version`` reads a database's schema version without
triggering ``open``'s auto-migration. ``git_merge_driver::run`` calls it on
all three sides (``base`` treated as absent if the file is missing or
empty) and passes the results to
``db::require_matching_schema_versions``, which refuses the merge with an
explicit message pointing at ``doctor`` if they disagree — before any side
is ever ``open``-ed (and thus auto-migrated) for real. This closes the gap
this incident exposed: a node that never migrated before merging against
an already-migrated remote now gets a loud, actionable refusal instead of
a silent mass-duplication.

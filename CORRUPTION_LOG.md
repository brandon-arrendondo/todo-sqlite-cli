# Corruption incident: v2→v3 merge mass-duplicated tools_sqc's task history

**Date:** 2026-08-27
**Reporter:** Claude Code session working in `~/data-enterprise/tools_sqc` (bench node)
**Repo affected:** `tools_sqc`'s `todo-sqlite-cli.db`
**Severity:** High — every pre-existing task in the shared DB was duplicated (618 of 620
tasks affected; 1238 rows after the merge instead of 620)

## Summary

`tools_sqc`'s bench node had been running against an **old v2-schema** local
`todo-sqlite-cli.db` all session (no `uuid` column — `id INTEGER PRIMARY KEY AUTOINCREMENT`).
Commit `47b4c484` ("docs: adopt todo-sqlite-cli v3.0.0 semantics; close task 615") landed on
`origin/main` with a DB already migrated to the **v3 schema** (`uuid TEXT PRIMARY KEY`,
`id INTEGER NOT NULL` demoted to a display alias, real UUIDs already assigned to every
pre-existing task).

When the bench node ran `git pull`, git's registered merge driver
(`merge.todo-sqlite-cli.driver = todo-sqlite-cli git-merge-driver %O %A %B`) fired to merge
the bench node's v2 DB against origin's v3 DB. The result: **every one of the 618
pre-existing tasks was duplicated** — same display id, same title, two different UUIDs.
`todo-sqlite-cli doctor` correctly flagged this (`duplicate_display_ids: 618`), which is how
it was caught before push.

## Root cause (confirmed via evidence below)

The v2→v3 migration was **not coordinated across nodes**. Each node's `todo-sqlite-cli`
client apparently auto-migrates a v2 database to v3 on first v3-aware access, and that
migration **mints a fresh random UUID for every existing v2 row** with no shared "this v2
row = that other node's v3 row" identity to anchor against. So:

- Origin's DB had already been migrated on some other node, assigning e.g.
  `ce83cf74-1e5c-4f81-b1af-3d9a3e3688ae` to task id=1 ("MEM30-C field-level free tracking").
- The bench node's DB was still plain v2 (no uuid column at all) at merge time.
- The merge driver, needing a UUID identity to merge by, evidently minted its own fresh
  UUID for the bench node's id=1 (`50fe14af-32bb-4f54-8013-2a8ece3c3c43`, per `doctor`'s
  output) instead of recognizing it as the same logical task as origin's `ce83cf74...`.
- The merge then correctly unioned by UUID **as designed** — but since the two UUIDs for the
  same logical task never matched, the union produced two rows instead of a reconciled one.

This reproduced for **every** pre-existing task, because the bench node's entire DB was
still on the old schema — it isn't a narrow "two nodes independently created a task that
happened to land on the same display id" case (which `47b4c484`'s CLAUDE.md update already
documents as an expected, rare residual risk `doctor` should catch). This is the *whole
historical backlog* colliding at once because of a **local node never having gone through
the v3 migration before merging against an already-migrated remote**.

## Evidence (preserved in `corruption-evidence/`, gitignored — `*.db`)

- `tools_sqc-mynode-pre-merge-v2schema.db` — the bench node's local DB right before the
  problematic pull (v2 schema, no `uuid` column, 618 tasks, `doctor` reports "clean" against
  it because the concept doesn't apply to a schema with no uuid column).
- `tools_sqc-origin-pre-merge-v3schema.db` — `origin/main`'s DB at commit `c53b813e` (the tip
  just before the bench node's merge commit), v3 schema, 620 tasks, `doctor: clean`.
- `tools_sqc-20260827-post-v3-merge.db` — the merged result the bench node's `git pull`
  produced: 1238 tasks, 618 duplicate-display-id groups (`doctor` output at the bottom of
  this file for a sample).

Confirming query (same logical task, two UUIDs after merge):

```
$ sqlite3 tools_sqc-mynode-pre-merge-v2schema.db "select id,title from tasks where id=1;"
1|MEM30-C field-level free tracking

$ sqlite3 tools_sqc-origin-pre-merge-v3schema.db "select id,uuid,title from tasks where id=1;"
1|ce83cf74-1e5c-4f81-b1af-3d9a3e3688ae|MEM30-C field-level free tracking

# After merge, doctor reports BOTH of these under id=1:
#   id=1 uuid=50fe14af-32bb-4f54-8013-2a8ece3c3c43 title=MEM30-C field-level free tracking
#   id=1 uuid=ce83cf74-1e5c-4f81-b1af-3d9a3e3688ae title=MEM30-C field-level free tracking
```

## What the bench node did to recover

Per the user's explicit instruction (not a unilateral decision — this was a shared-DB
incident and the user was asked before any destructive action was taken):

1. Copied the corrupted merged DB out to `corruption-evidence/` before touching anything
   (see files above).
2. Reset the bench node's local `todo-sqlite-cli.db` to the clean upstream copy
   (`origin/main` at `c53b813e`, `doctor: clean`, 620 tasks) — discarding the corrupted
   merge result rather than attempting to hand-deduplicate 618 rows.
3. Re-applied the bench node's own in-session progress notes (task 388) by hand, since those
   specific edits had only ever existed in the corrupted local DB and hadn't reached a clean
   push yet. Nothing else from the corrupted merge was carried forward.
4. Did **not** attempt to fix the merge driver or the v2→v3 migration path itself — that's
   this repo's concern, not `tools_sqc`'s.

## Open questions for this repo to investigate

- Does `todo-sqlite-cli`'s v3 migration path have any way to detect "this v2 row and that
  v3 row are the same task" across nodes when a node migrates late, other than by lucky
  content match? If not, every node that pulls v3-schema history into a still-v2 local DB
  is at risk of this exact mass-duplication.
- Should the merge driver refuse (or warn loudly, not just via a separate `doctor` step) when
  it detects it's merging a v2-schema DB against a v3-schema DB, rather than silently
  auto-migrating and producing a technically-valid-but-massively-duplicated result?
- `47b4c484`'s own CLAUDE.md update tells users to run `doctor` after every merge that
  touches the DB — that worked here (caught before push), but only because the bench node's
  session happened to run it. Is there a way to make this same-schema-mismatch case fail
  the merge (or the git hook) automatically rather than relying on a human/agent
  remembering the follow-up step and reading its output before pushing?
- Recommend an explicit "how to onboard an existing v2 node to v3" doc/runbook step (e.g.
  "pull once on a v2 node BEFORE making any new local edits, so the merge driver's
  migration path is exercised cleanly against a pristine local copy") if one doesn't already
  exist — the user's own read on this incident was "we likely should have onboarded the new
  schema BEFORE adding new tasks."

## `doctor` output sample from the corrupted merge (first few of 618 groups)

```
duplicate display ids (618) — show/edit by uuid to disambiguate:
  id=1 uuid=50fe14af-32bb-4f54-8013-2a8ece3c3c43 title=MEM30-C field-level free tracking
  id=1 uuid=ce83cf74-1e5c-4f81-b1af-3d9a3e3688ae title=MEM30-C field-level free tracking
  id=2 uuid=6856635f-2fb5-43f6-9d9d-cf8e6098013c title=MEM31-C ownership model
  id=2 uuid=9a10887b-48a6-4380-afa9-d64f5edfdb66 title=MEM31-C ownership model
  id=3 uuid=6eae9bcb-22e2-4246-bce8-f3f91a128f59 title=DCL13-C alias tracking
  id=3 uuid=aeffccf1-b96b-4a2d-9efe-21f64c0a522c title=DCL13-C alias tracking
  ... (615 more groups, essentially the entire pre-existing backlog)
```

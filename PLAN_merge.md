# Plan: merge two todo databases

Motivation: several coding-agent nodes can work against the same repo and
each commit their local `.db` (or a repo-shared one on divergent branches).
Because the file is opaque SQLite, git cannot text-merge it — any concurrent
edit is a binary conflict the user currently has to resolve by hand (pick
one side, lose the other's work). This adds a real merge engine plus two
ways to invoke it: a manual `merge` subcommand, and a git merge driver so
`git merge`/`pull`/`rebase` resolves the file automatically in the common
case and only flags what it can't safely decide.

## Identity problem

`tasks.id` is a per-database `AUTOINCREMENT` counter. Two databases that
diverged from a common point will each keep allocating from where they
left off — so id 7 in "ours" and id 7 in "theirs" are only the *same task*
if both descend from a row that already had id 7 at the point they forked.
A merge can't assume same-id-means-same-task without knowing what existed
at the fork point.

## The three-way core

`merge_databases(base: Option<&Connection>, ours, theirs, out, opts) -> MergeReport`
in `src/merge.rs` is the single engine both entry points call.

- `base_ids` = the set of task ids present in `base` (empty set if `base`
  is `None` — no common ancestor, e.g. the manual 2-way form, or git's `%O`
  for a file added independently on both sides).
- **Common tasks** (`id ∈ base_ids`, present in `ours` and/or `theirs`):
  reconciled per-field against the base row — this is the real 3-way merge.
  - Present in base, missing from one side, unchanged in the other → the
    deletion wins (dropped, tags/deps cascade, dangling edges from the
    other side onto it are dropped too).
  - Present in base, missing from one side, *changed* in the other →
    modify/delete conflict: keep the modified (undeleted) version, flag it.
  - Present in base, present in both → per-field merge (see below).
- **New tasks** (`id ∉ base_ids`): a task new to exactly one side is kept
  as-is. A task new to *both* sides sharing the same id is a pure id
  collision (two unrelated tasks that happened to get the same
  autoincrement value) — never field-merged. `ours`'s new tasks keep their
  ids; any of `theirs`'s new tasks that collide (with an `ours` id or with
  each other after remapping) get renumbered above the current max id, in
  `created_at` order for determinism. The renumbering map is applied to
  `theirs`'s tags and dep edges (both endpoints) before anything is unioned
  in, including edges from common tasks that reference a renumbered id.
- This means the 2-way form (`base = None`) isn't a separate code path —
  it's the 3-way engine with an empty base, which naturally makes *every*
  overlapping id a collision to renumber. One engine, two entry points.

### Per-field rules for a common task

Let `changed(x) = x_side != x_base`.

| field | rule |
|---|---|
| `title` | only one side changed → take it. Both changed to the *same* value → fine. Both changed to *different* values → **hard conflict**: keep `ours`, tag `merge-conflict`, note the clash in `details`. |
| `details` | only one side changed → take it. Both changed → diff each side's delta against the base text and concatenate both deltas (mirrors `edit --append-details`'s own append semantics) instead of duplicating the shared prefix. Not a hard conflict. |
| `status` | only one side changed → take it. Both changed to different values → rank order `pending(0) < {partial,in-progress}(1) < {done,rejected}(2)`, higher rank wins; equal-rank tie-break `in-progress` over `partial`, `done` over `rejected`. Auto-resolved, not flagged. `started_at`/`completed_at` follow whichever row's status was selected (earlier `completed_at` if both sides independently reached `done`). |
| `priority` | only one side changed → take it. Both changed differently → take the more urgent (lower number). Auto-resolved. |
| `is_gate` | only one side changed → take it. Both changed to different booleans → **hard conflict**, keep `ours`, flag. |
| `created_at` | always the base value (immutable). |
| `tags` | plain union of `ours` ∪ `theirs`. Never a conflict. |
| `deps` | union of both sides' edges (after id remapping), skipping self-loops and any edge that would introduce a cycle in the merged graph (dropped silently — cycles can only arise from the union of two acyclic graphs in pathological cross-referencing new-task cases). |

A **hard conflict** means: the affected task gets tagged `merge-conflict`
and a line appended to `details` recording both values, so
`list --tag merge-conflict` / `show <id>` surface it for a human to
resolve with `edit`, same as any other task. `--strict` changes this: on
the first hard conflict, abort entirely (exit 1, `--into` file untouched)
instead of writing a best-effort result.

## Entry points

1. **`merge --ours PATH --theirs PATH [--base PATH] [--into PATH] [--strict]`**
   — manual/ad-hoc use. `--into` defaults to overwriting `--ours` in place.
   Prints a summary (counts + conflict list); `--json` for machine-readable.
2. **`git-merge-driver <base> <ours> <theirs>`** — implements git's
   `merge.<driver>.driver = ... %O %A %B` contract directly: reads the temp
   files git hands it (an empty/missing `%O` becomes `base = None`), writes
   the merged result back into the `ours` path (git's requirement), and
   exits non-zero when any hard conflict was recorded so git still reports
   the merge as needing attention even though a usable file was written.
3. **`install-merge-driver`** — one-time setup helper: adds
   `<db-relative-path> merge=todo-sqlite-cli` to `.gitattributes` and runs
   `git config merge.todo-sqlite-cli.name/.driver` in the current repo.
   Only run when the user explicitly asks — it edits a tracked file that
   affects every collaborator's merge behavior.

Both `merge` and `git-merge-driver` take explicit paths and bypass the
normal `--db`/marker resolution (like `init` does), since they're operating
on specific files handed to them, not "the" project database.

## Output DB construction

Read `base`/`ours`/`theirs` fully into memory as plain Rust structs (each
opened read-only via `db::open`, which auto-migrates it to
`SCHEMA_VERSION` first — no new migration logic needed), compute the
merged task/tag/dep sets purely in memory, then write the result into a
fresh file at a temp path via `db::create_schema` + explicit-id inserts,
restore `sqlite_sequence` to the merged max id, and `fs::rename` into
place. Never mutate `ours`/`theirs`/`base` in place; never write partial
output on error.

## Testing

`tests/merge.rs`: independent new-task union with id-collision renumbering
(and dep/tag remap along with it); one-side-only field changes on a common
task; genuine same-field conflict tags `merge-conflict` and keeps `ours`;
status rank resolution; details delta-concatenation; delete/modify
conflict; dep union that would create a cycle drops the offending edge;
`--strict` aborts and writes nothing; 2-way (no `--base`) degenerate case.

# Plan: backlog trend reporting (CFD + aging)

Motivation: consuming projects (tools_sqc first) want a weekly-rebase signal
for "is the backlog actually thinning, or just churning" and "which low-
priority tasks have been aging unnoticed and should get pulled up." Both are
fully reconstructable from data already in `tasks` — no schema change, no
new instrumentation, no migration. This doc scopes two additive, read-only
reporting commands.

## Feasibility (why no migration is needed)

`tasks` already carries the three timestamps needed to reconstruct history
after the fact:

- `created_at` — task entered the backlog
- `started_at` — task first entered `in-progress` (see caveat below)
- `completed_at` — task closed, either `done` or `rejected` (both set this
  column; `done --rejected` just changes the target status, per
  `src/commands/done.rs`)

Caveat to document in the command's `--help` and README, not to solve now:
`start`/`stop`/`revert` can pause and resume a task, and `revert` clears
`started_at` entirely. So `started_at` reflects the *current* work episode's
start, not necessarily the first time work ever began, for tasks that were
reverted. This under-counts historical in-progress time for reverted tasks.
Acceptable for a trend/rebase signal; call it out rather than silently
presenting it as exact.

## 1. `cfd` — cumulative flow diagram data (commit to building)

```
todo-sqlite-cli cfd [--since DATE] [--until DATE] [--bucket day|week] [--json] [--format ascii|csv]
```

- Buckets the timeline from `--since` (default: earliest `created_at`) to
  `--until` (default: today) at `--bucket` granularity (default `day`).
- For each bucket boundary date `d`, compute cumulative counts as of end-of-`d`:
  - `backlog` = count where `created_at <= d` AND (`started_at IS NULL OR started_at > d`) AND (`completed_at IS NULL OR completed_at > d`)
  - `in_progress` = count where `started_at <= d` AND (`completed_at IS NULL OR completed_at > d`)
  - `done` = count where `completed_at <= d` AND status ended as `done`
  - `rejected` = count where `completed_at <= d` AND status ended as `rejected`
  - Note: `status` on the row only reflects the *current* status, not the
    status as of `d`. For done/rejected tasks this is fine (terminal, doesn't
    change again). For open tasks reconstructing backlog vs in-progress as of
    a past `d`, current status is irrelevant — only the timestamp comparisons
    above matter. This works because backlog/in-progress/done/rejected as
    defined here are mutually exclusive and derived purely from timestamp
    presence, not from the mutable `status` column.
  - All four counts are computed with pure SQL against the existing schema —
    no new table, no snapshotting required. A single query with `created_at`,
    `started_at`, `completed_at` compared against each bucket boundary,
    looped in Rust over the bucket list (or one SQL query per boundary; task
    counts are small enough — hundreds, not millions — that N queries for N
    buckets is fine, no need to over-optimize).
- Output:
  - `--format ascii` (default, human): a compact stacked/row-per-bucket table,
    e.g. `2026-08-10  backlog=42  in_progress=3  done=410  rejected=8`, or a
    simple sparkline per series if a terminal chart is wanted later — start
    with the table, it's enough to eyeball trend direction.
  - `--format csv`: same columns, for dropping into a spreadsheet/plotting tool.
  - `--json`: `{"buckets": [{"date": "...", "backlog": N, "in_progress": N, "done": N, "rejected": N}, ...]}`.
- Filtering: no `--tag`/`--status` filter needed for v1 — whole-project trend
  is the primary use case. Add `--tag` filtering later only if a project asks
  (would need the same bucketed reconstruction joined against `tags`).

### Implementation sketch
- New file `src/commands/cfd.rs`, following the shape of `export_completed.rs`
  (date-bucketed output) and `list.rs` (query building).
- New `Command::Cfd { since, until, bucket, format }` variant in `src/cli.rs`.
- Bucket-boundary generation: reuse or extract whatever date-parsing helper
  `export-completed --since/--until` already uses if one exists (check
  `resolve.rs`/`format.rs` first — don't hand-roll RFC3339/date-only parsing
  twice).
- Tests: seed a small set of tasks with fixed created_at/started_at/
  completed_at (tests already do this per `tests/migration.rs` style raw
  SQL), assert bucket counts at a few boundary dates including edge cases
  (task created and completed within the same bucket; task still open at
  `--until`; reverted task with cleared `started_at`).

## 2. Surfacing age — open design question, NOT yet decided

Two options were raised; pick one (or both, but sequence them — see
recommendation) before implementing:

**Option A — `aging` command (read-only view, additive).**
```
todo-sqlite-cli aging [--stale-days N] [--tag TAG] [--json]
```
Lists open tasks (pending/partial/in-progress) sorted by
`now - created_at` descending, alongside current `priority`. With
`--stale-days`, flags tasks over the threshold as rebase candidates. Does
**not** change `next`/`list` ordering or the `priority` value itself — purely
a report a human (or a weekly-rebase agent) reads and then manually
`edit --priority`s the flagged tasks.

**Option B — age-weighted computed priority.**
Would change actual task ordering (`next`, `list`) by blending stored
`priority` with age into an *effective* priority, so aging low-priority tasks
automatically rise in the queue without a manual edit step.

### Recommendation: build Option A first, defer Option B

Reasons:
- Option A is purely additive and read-only — zero risk to `next`'s existing
  ordering contract, which other tooling/agents already depend on
  (`next` order is documented and load-bearing across every consuming repo).
- Option B changes the *meaning* of `priority` (stored value vs. effective/
  displayed value) and would need real design work: what's the aging curve
  (linear? capped? per-priority-band different rates?), does `list`'s default
  sort change, does `--json` expose both raw and effective priority, does
  this interact with `depends_on`/blocked filtering. That's a real feature
  with its own edge cases, not a drop-in.
- The stated goal — "pull something from P5 into higher priority as part of
  a weekly rebase" — is satisfied by Option A plus a human-in-the-loop
  `edit --priority` step. That's likely the right amount of automation for a
  weekly cadence: a person (or reviewing agent) should be looking at *why* a
  task aged, not just auto-promoting it blind.
- If Option A's aging report is used for a few rebase cycles and manual
  promotion proves to be pure toil with no judgment involved, that's the
  signal to come back and build Option B — not before.

Ship `cfd` and `aging` (Option A) together as v1; leave Option B out of scope
for this pass.

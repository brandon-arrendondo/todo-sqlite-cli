# Plan: first-class "gate" task kind

Motivation: some tasks aren't work to be done — they're a checkpoint on a
*condition* becoming true (e.g. "sqc has reached maintenance-mode stability"
gating a paper-finalization task). `depends_on` already models the blocking
relationship correctly, but a plain task modeled as a gate today is a bad
fit for every trend/staleness view built so far:

- `aging --stale-days N` will (correctly, for a normal task) flag it as
  rotting the longer it sits open — but a gate is *supposed* to sit open
  indefinitely until its condition is met. Staleness is not a smell here,
  it's the design.
- `next` will eventually surface it as "the task to work on" — but there's
  no actionable next-step on a gate. Nobody `start`s a gate and makes
  incremental progress; someone periodically re-assesses whether the world
  now satisfies the condition described in its `details`, and only then
  calls `done`.
- Nothing today visually distinguishes "a real task that's been open 60
  days and needs attention" from "a gate that's supposed to be open 60
  days and is fine."

A working stand-in exists today (title-prefixed `GATE: ...` + a `gate` tag,
per tools_sqc task #463 and the earlier ad-hoc `GATE: eclipse-mosquitto#3666`
task #176) — this plan turns that convention into a real, enforced kind so
the CLI's own views (aging, next, list) treat it correctly instead of
relying on humans reading title prefixes.

## Schema change (real migration, unlike the additive cfd/aging commands)

Add one column, following the exact `migrate_vN_to_vN+1` table-rebuild
pattern already used for the `partial` and `rejected` status additions
(`src/db.rs::migrate_v1_to_v2` / `migrate_v2_to_v3`):

```sql
-- v3 -> v4
CREATE TABLE tasks_new (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    title        TEXT NOT NULL,
    details      TEXT,
    status       TEXT NOT NULL CHECK(status IN ('pending','partial','in-progress','done','rejected')),
    priority     INTEGER NOT NULL DEFAULT 3 CHECK(priority BETWEEN 1 AND 5),
    is_gate      INTEGER NOT NULL DEFAULT 0 CHECK(is_gate IN (0,1)),
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    completed_at TEXT
);
INSERT INTO tasks_new(id, title, details, status, priority, created_at, started_at, completed_at)
    SELECT id, title, details, status, priority, created_at, started_at, completed_at FROM tasks;
-- is_gate defaults to 0 for all pre-existing rows, which is correct.
DROP TABLE tasks;
ALTER TABLE tasks_new RENAME TO tasks;
DROP INDEX IF EXISTS idx_tasks_status_priority;
CREATE INDEX idx_tasks_status_priority ON tasks(status, priority, created_at);
UPDATE meta SET value = '4' WHERE key = 'schema_version';
```

Reuse the same `sqlite_sequence` save/restore dance around the `BEGIN...COMMIT`
block that both prior migrations use — copy `migrate_v2_to_v3` almost
verbatim and add the `is_gate` column.

A plain boolean is deliberately chosen over a `kind TEXT` enum: there is
exactly one non-default kind needed today, and the CHECK-constrained-integer
style matches how `status`/`priority` are already modeled. Don't add an
enum for a single boolean distinction.

## CLI surface

- `add --gate` — new flag, sets `is_gate = 1` on creation. No new required
  fields; the gate's condition is just prose in `--details`, same as any
  task. Don't invent a separate structured "criteria" field — that's
  premature structure for something a human reads and judges, not something
  the CLI evaluates.
- `edit --gate` / `--no-gate` — toggle after creation (e.g. promoting an
  existing task to gate status, as tools_sqc will want to do with its #463
  stand-in once this ships).
- `list` / `show` / `export-todo`: prefix `[GATE]` before the title in
  table/markdown output (mirrors the existing `[STALE]` suffix convention
  in `aging`'s ascii output). `--json`/`--format ndjson` include the raw
  `is_gate` boolean field on every task object — no gating of the gating
  field behind a flag.
- `list --kind gate|task|all` — new filter, default `all` (don't change
  default `list` behavior). `gate` gives a "readiness dashboard" view: every
  open checkpoint and what it's blocking.
- `next`: **exclude gates from selection.** A gate is never "the task to
  work on next" — there's no start/stop/partial episode that makes sense
  for it. If every remaining unblocked task happens to be a gate, `next`
  should report nothing-actionable rather than surface one, with a message
  pointing at `list --kind gate` instead.
- `aging`: gates **remain listed** (so you can still see how long a
  checkpoint has been open) but are **never marked `[STALE]`** regardless
  of `--stale-days`, since indefinite openness is the correct state for a
  gate, not backlog rot. Filter it out of the "flag as rebase candidate"
  logic specifically, not out of the report entirely.
- `cfd`: no change. A gate counts toward `backlog`/`in_progress`/`done` like
  any other task — splitting the CFD by kind is unnecessary complexity
  unless a project actually asks for it.

## Migration note for existing ad-hoc gates

tools_sqc currently has at least two tasks using the `GATE: ...` title-
prefix convention pre-dating this feature (task #176, task #463). Once
`edit --gate` ships, those should be converted (`edit --gate`) rather than
left as prose-only gates — that's a tools_sqc-side follow-up, not part of
this implementation, but worth a one-line callout in the release notes so
existing users of the convention know to migrate.

## Tests

Follow the existing migration test shape in `tests/migration.rs` (raw-SQL
seed of a pre-migration DB, run migration, assert post-state) — add a
v3-seeded fixture, run `migrate_v3_to_v4`, assert `is_gate` defaults to 0
for pre-existing rows and the column accepts 0/1 only. Add command-level
tests for: `add --gate` sets the flag; `next` skips an unblocked gate and
falls through to the next real task (or reports nothing if only gates
remain); `aging --stale-days` never marks a gate `[STALE]` no matter its
age; `list --kind gate` returns only gates.

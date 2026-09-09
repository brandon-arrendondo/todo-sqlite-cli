Architecture
============

This document describes ``todo-sqlite-cli``'s internal structure and the
design decisions behind features whose reasoning isn't obvious from reading
the code alone.

.. contents:: Contents
   :local:
   :depth: 2

---------------------------------------------------------------------------

Overview
--------

``todo-sqlite-cli`` is a single Rust binary (~5 100 lines) backed by SQLite
(bundled via ``rusqlite``'s ``bundled`` feature — no system SQLite
dependency). An optional Python MCP server (``mcp_server/``) wraps the
binary for coding-agent tool use; it is a thin subprocess wrapper, not a
reimplementation, and a plain ``cargo install`` never needs it.

The codebase has three conceptual layers:

::

    ┌────────────────────────────────────────────────────────────┐
    │  CLI parsing + dispatch  (src/cli.rs, src/main.rs)         │
    │  clap derive Command enum · argument resolution             │
    ├────────────────────────────────────────────────────────────┤
    │  Commands  (src/commands/*.rs)                              │
    │  one file per subcommand — add, edit, list, next, merge, …  │
    ├────────────────────────────────────────────────────────────┤
    │  Data layer  (src/db.rs, src/merge.rs, src/format.rs)      │
    │  schema + migrations · 3-way merge engine · output shaping  │
    └────────────────────────────────────────────────────────────┘

``src/db.rs`` (~880 lines) owns the schema, all ``migrate_vN_to_vN+1``
functions, and ``resolve_one``/uuid-vs-display-id lookup. ``src/merge.rs``
(~760 lines) is the standalone 3-way merge engine — see
:doc:`merge-engine`. ``src/format.rs`` (~340 lines) shapes task
output across table/json/ndjson/markdown.

---------------------------------------------------------------------------

Design decisions
-----------------

The sections below record *why* a shipped feature works the way it does,
for cases where the reasoning would otherwise only live in old planning
notes or a commit message.

Gate tasks are a boolean, not a ``kind`` enum
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

A **gate** (``is_gate`` column, schema v4+) is a checkpoint on a condition
becoming true (e.g. "dependency X reaches maintenance-mode stability"),
not work to be done. It needed to be a real, enforced kind rather than the
title-prefix convention (``GATE: ...`` + a ``gate`` tag) used before this
shipped, because several views need to treat it differently from an
ordinary task:

- ``next`` **excludes gates from selection** — there's no start/stop
  episode that makes sense for a gate, so it should never be "the task to
  work on next." If every remaining unblocked task is a gate, ``next``
  reports nothing actionable rather than surfacing one, with a hint
  pointing at ``list --kind gate``.
- ``aging`` still lists gates (so you can see how long a checkpoint has
  been open) but **never flags one ``[STALE]``**, regardless of
  ``--stale-days`` — indefinite openness is the correct state for a gate,
  not backlog rot.
- ``list``/``show``/``export-todo`` prefix ``[GATE]`` on the title and
  expose the raw ``is_gate`` boolean unconditionally in json/ndjson.
- ``cfd`` makes no distinction — a gate counts toward
  backlog/in-progress/done like any other task; splitting the CFD by kind
  was judged unnecessary complexity unless a project actually asks for it.

A plain boolean was deliberately chosen over a ``kind TEXT`` enum: there is
exactly one non-default kind needed, and the CHECK-constrained-integer
style already matches how ``status``/``priority`` are modeled. Don't add an
enum for a single boolean distinction.

The gate's condition is just prose in ``--details``, same as any task —
there's no separate structured "criteria" field, since that would be
premature structure for something a human reads and judges, not something
the CLI evaluates.

Backlog trend: age-weighted priority was considered and deferred
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

``cfd`` and ``aging`` (both read-only, additive, no schema change — every
timestamp they need was already on ``tasks``) ship a "which low-priority
tasks have aged unnoticed" signal for periodic backlog rebases. Two designs
were considered for turning that signal into action:

- **Option A (shipped)** — ``aging --stale-days N`` reports tasks over the
  threshold; a human (or reviewing agent) reads the report and manually
  ``edit --priority``\ s the ones worth pulling forward.
- **Option B (not built)** — an age-weighted *effective* priority that
  automatically blends stored ``priority`` with age, changing ``next``'s
  and ``list``'s actual ordering.

Option A shipped alone. Reasons Option B was deferred rather than built
alongside it:

- Option A is purely additive and read-only — zero risk to ``next``'s
  existing ordering contract, which other tooling/agents already depend on
  (documented and load-bearing across every consuming repo).
- Option B changes the *meaning* of ``priority`` (stored vs. effective/
  displayed value) and raises real unresolved design questions: what aging
  curve (linear? capped? per-priority-band rates?), does ``list``'s default
  sort change, does ``--json`` expose both raw and effective priority, how
  does it interact with ``depends_on``/blocked filtering.
- The motivating goal — pulling a task from P5 into higher priority during
  a weekly rebase — is satisfied by Option A plus a human-in-the-loop edit.
  That's plausibly the right amount of automation for a weekly cadence: a
  person should be looking at *why* a task aged, not auto-promoting it
  blind.

If Option A's report is used for a few rebase cycles and manual promotion
turns out to be pure toil with no judgment involved, that's the signal to
revisit Option B — not before. As of this writing it remains an open,
deliberately-unbuilt design question rather than a rejected one.

``started_at`` under-counts reverted work
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

``cfd`` and ``aging`` reconstruct history from ``created_at``/
``started_at``/``completed_at`` alone — no new instrumentation. One known
gap: ``start``/``stop``/``revert`` can pause and resume a task, and
``revert`` clears ``started_at`` entirely, so it reflects the *current*
work episode's start, not necessarily the first time work ever began, for
tasks that were reverted at least once. This under-counts historical
in-progress time for those tasks. Acceptable for a trend/rebase signal, but
worth calling out explicitly rather than presenting it as exact — this page
is that callout; neither ``--help`` nor the README mentions it yet.

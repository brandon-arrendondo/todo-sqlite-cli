Merge Engine
============

Several coding-agent nodes can work against the same repo and each commit
their local ``.db`` (or a repo-shared one on divergent branches). Because
the file is opaque SQLite, git cannot text-merge it — any concurrent edit
is normally a binary conflict a user has to resolve by hand, picking one
side and losing the other's work. ``todo-sqlite-cli`` instead ships a real
merge engine plus two ways to invoke it: a manual ``merge`` subcommand, and
a git merge driver so ``git merge``/``pull``/``rebase`` resolves the file
automatically in the common case and only flags what it can't safely
decide.

.. contents:: Contents
   :local:
   :depth: 2

---------------------------------------------------------------------------

Identity problem
-----------------

A task's ``uuid`` (schema v5+; ``id`` before that) is the merge identity.
Two databases that diverged from a common point may each have allocated
new tasks independently — an id/uuid present in both "ours" and "theirs"
is only the *same task* if both descend from a row that already had that
identity at the fork point. A merge can't assume matching identity means
matching task without knowing what existed at the fork point. See
:doc:`incidents` for what goes wrong when this assumption is violated by a
node merging across mismatched schema versions.

---------------------------------------------------------------------------

The three-way core
--------------------

``merge_databases(base: Option<&Connection>, ours, theirs, out, opts) -> MergeReport``
in ``src/merge.rs`` is the single engine both entry points call.

- ``base_ids`` = the set of task identities present in ``base`` (empty set
  if ``base`` is ``None`` — no common ancestor, e.g. the manual 2-way form,
  or git's ``%O`` for a file added independently on both sides).
- **Common tasks** (identity present in ``base``, and in ``ours`` and/or
  ``theirs``): reconciled per-field against the base row — this is the
  real 3-way merge.

  - Present in base, missing from one side, unchanged in the other → the
    deletion wins (dropped; tags/deps/related cascade; dangling edges from
    the other side onto it are dropped too).
  - Present in base, missing from one side, *changed* in the other →
    modify/delete conflict: keep the modified (undeleted) version, flag it.
  - Present in base, present in both → per-field merge (see below).
  - Present in both, but **not** in base (no common ancestor at all — the
    2-way form) → ``merge_common_no_base``: a field that agrees on both
    sides carries through unchanged; a field that disagrees can't be
    attributed to either side, so it's a conflict — keep ``ours``, tag it.

- **New tasks** (identity not in ``base_ids``): a task new to exactly one
  side is kept as-is. A task new to *both* sides sharing the same display
  id is a pure id collision (two unrelated tasks that happened to get the
  same value) — never field-merged. ``ours``'s new tasks keep their ids;
  any of ``theirs``'s new tasks that collide (with an ``ours`` id or with
  each other after remapping) get renumbered above the current max id, in
  ``created_at`` order for determinism. The renumbering map is applied to
  ``theirs``'s tags, dep edges, and related edges (all endpoints) before
  anything is unioned in, including edges from common tasks that reference
  a renumbered id.
- This means the 2-way form (``base = None``) isn't a separate code
  path — it's the 3-way engine with an empty base, which naturally makes
  *every* overlapping identity a collision to renumber. One engine, two
  entry points.

Per-field rules for a common task
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

Let ``changed(x) = x_side != x_base``. Fields not listed below (e.g.
``created_at``) are immutable and always take the base value.

.. list-table::
   :header-rows: 1
   :widths: 22 78

   * - Field
     - Rule
   * - ``title``
     - Only one side changed → take it. Both changed to the *same* value →
       fine. Both changed to *different* values → **hard conflict**: keep
       ``ours``, tag ``merge-conflict``, note the clash in ``details``.
   * - ``details``
     - Only one side changed → take it. Both changed → diff each side's
       delta against the base text and concatenate both deltas (mirrors
       ``edit --append-details``'s own append semantics) instead of
       duplicating the shared prefix. Not a hard conflict.
   * - ``status``
     - Only one side changed → take it. Both changed to different values →
       rank order ``pending(0) < {partial,in-progress}(1) < {done,rejected}(2)``,
       higher rank wins; equal-rank tie-break ``in-progress`` over
       ``partial``, ``done`` over ``rejected``. Auto-resolved, not flagged.
       ``started_at``/``completed_at`` follow whichever row's status was
       selected (earlier ``completed_at`` if both sides independently
       reached ``done``).
   * - ``priority``
     - Only one side changed → take it. Both changed differently → take
       the more urgent (lower number). Auto-resolved.
   * - ``is_gate``
     - Only one side changed → take it. Both changed to different booleans
       → **hard conflict**, keep ``ours``, flag.
   * - ``location``
     - Only one side changed → take it. Both changed to different values →
       **hard conflict**, keep ``ours``, flag.
   * - ``implementation_client``
     - Same rule as ``location``.
   * - ``project_name``
     - Same rule as ``location``.
   * - ``tags``
     - Plain union of ``ours`` ∪ ``theirs``. Never a conflict.
   * - ``deps``
     - Union of both sides' edges (after id remapping), skipping
       self-loops and any edge that would introduce a cycle in the merged
       graph (dropped silently — cycles can only arise from the union of
       two acyclic graphs in pathological cross-referencing new-task
       cases).
   * - ``related``
     - Union of both sides' edges (after id remapping), then symmetrized —
       every link is mirrored on both endpoints, dangling references (a
       related task that no longer exists post-merge) are dropped, and a
       self-link is dropped. Never a hard conflict.

A **hard conflict** means: the affected task gets tagged ``merge-conflict``
and a line appended to ``details`` recording both values, so
``list --tag merge-conflict`` / ``show <id>`` surface it for a human to
resolve with ``edit``, same as any other task. ``--strict`` changes this:
on the first hard conflict, abort entirely (exit 1, ``--into`` file
untouched) instead of writing a best-effort result.

---------------------------------------------------------------------------

Entry points
------------

1. ``merge --ours PATH --theirs PATH [--base PATH] [--into PATH] [--strict]``
   — manual/ad-hoc use. ``--into`` defaults to overwriting ``--ours`` in
   place. Prints a summary (counts + conflict list); ``--json`` for
   machine-readable output.
2. ``git-merge-driver <base> <ours> <theirs>`` — implements git's
   ``merge.<driver>.driver = ... %O %A %B`` contract directly: reads the
   temp files git hands it (an empty/missing ``%O`` becomes
   ``base = None``), writes the merged result back into the ``ours`` path
   (git's requirement), and exits non-zero when any hard conflict was
   recorded so git still reports the merge as needing attention even
   though a usable file was written. It also refuses outright — before
   opening anything for real — if the three files' pre-migration schema
   versions disagree; see :doc:`incidents` for why.
3. ``install-merge-driver`` — one-time setup helper: adds
   ``<db-relative-path> merge=todo-sqlite-cli`` to ``.gitattributes`` and
   runs ``git config merge.todo-sqlite-cli.name``/``.driver`` in the
   current repo. Only run when the user explicitly asks — it edits a
   tracked file that affects every collaborator's merge behavior.

Both ``merge`` and ``git-merge-driver`` take explicit paths and bypass the
normal ``--db``/marker resolution (like ``init`` does), since they're
operating on specific files handed to them, not "the" project database.

---------------------------------------------------------------------------

Output DB construction
------------------------

Read ``base``/``ours``/``theirs`` fully into memory as plain Rust structs
(each opened read-only via ``db::open``, which auto-migrates it to
``SCHEMA_VERSION`` first — safe here only because the schema-version guard
above has already confirmed all three sides agree before any of them are
opened), compute the merged task/tag/dep/related sets purely in memory,
then write the result into a fresh file at a temp path via
``db::create_schema`` + explicit-id inserts, restore ``sqlite_sequence`` to
the merged max id, and ``fs::rename`` into place. Never mutate
``ours``/``theirs``/``base`` in place; never write partial output on error.

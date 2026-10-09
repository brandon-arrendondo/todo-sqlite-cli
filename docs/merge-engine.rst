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

A task's ``uuid`` (schema v5+) is its identity. The numeric ``id`` is a
display alias and may be duplicated, reused after deletion, or changed by
``renumber``. Merge matches UUIDs, including without a common ancestor.
Different UUIDs are distinct tasks even when their display IDs match.

The v4-to-v5 migration generates fresh UUIDs for existing rows. Copies of an
old backlog must share one UUID-bearing migration snapshot before diverging.
Independently migrating copies of the same pre-UUID database does not create
matching identities. Schema-version equality is necessary for the merge guard
but does not establish shared UUID history. See :doc:`incidents`.

---------------------------------------------------------------------------

The three-way core
--------------------

``merge_databases(base: Option<&Connection>, ours, theirs, out, opts) -> MergeReport``
in ``src/merge.rs`` is the single engine both entry points call.

- ``base_uuids`` = the set of task identities present in ``base`` (empty set
  if ``base`` is ``None`` — no common ancestor, e.g. the manual 2-way form,
  or git's ``%O`` for a file added independently on both sides).
- **Common tasks** (UUID present in ``base``, and in ``ours`` and/or
  ``theirs``): reconciled per-field against the base row — this is the
  real 3-way merge.

  - Present in base, missing from one side, unchanged in the other → the
    deletion wins (dropped; tags/deps/related cascade; dangling edges from
    the other side onto it are dropped too).
  - Present in base, missing from one side, *changed* in the other →
    modify/delete conflict: keep the modified (undeleted) version, flag it.
  - Present in base, present in both → per-field merge (see below).

- **UUIDs unknown to base but present on both sides** are still the same
  task and use ``merge_common_no_base``. It compares ``title``, ``details``,
  ``status``, ``priority``, ``is_gate``, ``location``,
  ``implementation_client`` and ``project_name``. Equal values carry through;
  differing values keep ours and are hard conflicts. Display ID and
  ``created_at``/``started_at``/``completed_at`` take ours without a conflict,
  even when they differ. Tags, dependency edges and related edges union.
- **UUIDs present on only one side and absent from base** carry through with
  their original display IDs. Matching display IDs do not trigger automatic
  renumbering. Use ``doctor`` to find aliases shared by different UUIDs and
  ``renumber <full-uuid> <new-id>`` to resolve them explicitly.

Without a base, the known-base reconciliation pass is empty. Shared UUIDs
still reconcile once, and different UUIDs still remain separate. A common
ancestor enables attribution of which side changed each scalar field.

Per-field rules for a common task
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

Let ``changed(x) = x_side != x_base``. For common tasks, ``created_at`` and the display ``id`` take the base value.
A side's display-ID renumbering is not merged into a common task.

.. list-table::
   :header-rows: 1
   :widths: 22 78

   * - Field
     - Rule
   * - ``title``
     - Only one side changed → take it. Both changed to the *same* value →
       fine. Both changed to *different* values → **hard conflict**: keep
       ``ours``, tag ``merge-conflict``, report the clash.
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
     - Union of both sides' UUID edges, skipping
       self-loops and any edge that would introduce a cycle in the merged
       graph (dropped silently — cycles can only arise from the union of
       two acyclic graphs in pathological cross-referencing new-task
       cases).
   * - ``related``
     - Union of both sides' UUID edges, then symmetrized —
       every link is mirrored on both endpoints, dangling references (a
       related task that no longer exists post-merge) are dropped, and a
       self-link is dropped. Never a hard conflict.

A **hard conflict** means: the affected task gets tagged ``merge-conflict``
and the clash is recorded in the merge report, so
``list --tag merge-conflict`` / ``show <id>`` surface it for a human to
resolve with ``edit``, same as any other task. ``--strict`` changes this:
if any hard conflict is found, do not replace the output (exit 1) instead
of writing a best-effort result.

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

Read ``base``/``ours``/``theirs`` fully into memory as plain Rust structs.
Both entry points check present schema versions before the normal read/write
``db::open`` calls can migrate inputs to ``SCHEMA_VERSION``. Compute merged
task/tag/dependency/related sets in memory, then write a fresh file at a temp
path through ``db::create_schema`` and explicit-ID inserts. Finally use
``fs::rename`` to replace the output. The current schema has no display-ID
AUTOINCREMENT sequence; ``add`` allocates from the current maximum.

Task reconciliation does not edit the input rows directly, but opening inputs
can set WAL mode and migrate equally old schemas before output construction.
Thus ``--strict`` protects output replacement, not all filesystem changes to
inputs. Back up databases before migration or merge. Both entry points refuse
mismatched schema versions before using the normal opener.

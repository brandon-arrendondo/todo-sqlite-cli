# ADR-0003: A task's UUID is its identity; the display ID is only an alias

**Status:** Proposed, 2026-10-10. Records the implemented identity model;
awaiting review and acceptance.

## Context

A short numeric ID is convenient for typing a command, but independently
created tasks can receive the same number. Treating that number as identity
would combine unrelated tasks during a merge. Conversely, a renamed or
renumbered task must remain the same task.

Schema v5 introduced UUID task identity. The v4-to-v5 migration assigns a
fresh UUID to each existing task. The current schema uses `uuid` as the
primary key and retains a non-unique integer `id` as a display alias.
[The merge guide](../merge-engine.rst) describes the current reconciliation
rules and the limits of independently migrated copies.

## Decision

Match tasks and their relationships by UUID. A matching UUID identifies the
same task across database copies, including a merge without a common
ancestor. Different UUIDs identify different tasks even if their display
IDs, titles and other contents match.

The display ID is a local convenience, not a durable cross-copy reference.
New tasks receive a numeric alias from the current database; independent
copies can allocate the same alias. Merge preserves distinct UUIDs rather
than deleting one task or guessing identity from its title.

A numeric lookup that matches multiple tasks must refuse to choose one.
Use a UUID to disambiguate; `doctor` reports duplicate display IDs and
`renumber <full-uuid> <new-id>` resolves an alias explicitly. Renumbering
changes the alias, not the task's UUID. Relationship identity remains intact.

Copies of an old pre-UUID backlog must share one UUID-bearing migration
snapshot before they diverge. Independently migrating the same old rows
mints different identities; schema equality alone cannot reconstruct that
shared history. [ADR-0004](0004-never-merge-different-schema-versions.md)
records the schema guard and this limitation.

## Consequences

- A merged database may legitimately contain duplicate numeric aliases.
  Automation needing durable references should use UUIDs.
- Identity is not inferred from spelling or equal content. Any deliberate
  recovery of duplicated historical tasks needs separate review.
- A common ancestor improves reconciliation of changes but is not a
  prerequisite for recognizing shared UUIDs.
- This decision records existing behavior; it does not add automatic
  renumbering or identity repair.

# ADR-0004: Databases with different schema versions are never merged

**Status:** Proposed, 2026-10-10. Records the implemented pre-migration guard;
awaiting review and acceptance.

## Context

A production merge duplicated 618 of 620 tasks when one database already
had UUID identities and another still used the older numeric-identity
schema. Migrating the older input during the merge minted fresh UUIDs for
its existing rows. Union by UUID then treated those rows as different tasks.
The duplication followed the identity rules but violated the assumption
that the copies shared identity history.

[The incident record](../incidents.rst) retains the evidence. Its historical
schema labels differ from the current source: UUID identity is introduced
by the current v4-to-v5 migration. The safeguard depends on the versions
actually stored in each input, not on retelling those historical labels.

## Decision

Before opening merge inputs through the normal migrating database opener,
inspect the on-disk schema version of every present input. Require the
versions to match. Both the manual merge command and the Git merge driver
must refuse a mismatch before normal opening or output construction.
A missing common ancestor is allowed; it is not a schema version.

Normal database opening migrates supported older schemas forward and refuses
a schema newer than the binary supports. Merge does not bypass those rules.
Equal supported older versions may pass the guard and then be migrated by
the normal opener; passing the guard does not promise read-only inputs.

Version equality is necessary, not sufficient, for shared identity history.
In particular, independently migrating copies of the same pre-UUID backlog
can produce equal schema versions with different UUIDs. Coordinate that
migration by sharing a single migrated snapshot before copies diverge, as
[ADR-0003](0003-uuid-is-identity-display-id-is-an-alias.md) requires.

## Consequences

- A mismatch fails visibly instead of turning automatic migration into
  silent mass duplication. Upgrade and coordinate the database copies
  before retrying; never disable the guard to get a merge through.
- An older binary cannot reinterpret a newer database as its own schema.
- The guard cannot repair identities already minted independently. Such a
  recovery is a separate data-review operation, not a permissive merge.
- This decision records the current safeguards; it promises neither
  downgrade support nor transactionally read-only merge inputs.

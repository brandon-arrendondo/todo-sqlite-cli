# ADR-0002: The changelog tells users what changed; it is not a git log or a task list

**Status:** Proposed, 2026-10-10. Awaiting review and acceptance.

## Context

A todo-sqlite-cli user needs to know what a release adds, removes or changes,
especially when task identity, database compatibility or CLI behavior changes.
A list of commits or completed work mixes those changes with implementation
history.
The merge and incident records already hold design and failure evidence;
they are not release notes.

todo-sqlite-cli has no `CHANGELOG.md` today. Version-bump commits and the
release workflow's generated GitHub notes do not establish a curated
record of user-visible effects.

## Decision

Create `CHANGELOG.md` starting with the next release as a curated record for
users of todo-sqlite-cli. Each release has one dated section, newest first, with
Unreleased on top. Draw the GitHub release body from that release's section;
a compare link may supplement the notes but does not replace them. Write the
entry in the same change that ships the user-visible effect, under Unreleased.
Use these headings only when they have entries:

1. **Added:** a new CLI option, output format, task-management command or
   integration capability. Describe what the user can now do.
2. **Fixed:** a crash, incorrect output, nondeterminism or other tool defect.
   Describe the failing construct and resulting behavior in user terms.
3. **Removed:** an option, output field, platform or supported behavior
   a user could have relied on. Even small removals must be recorded.
4. **Changed:** a change to existing output's meaning or an option's behavior,
   including changed defaults. It is not a category for internal work.

**A change to task identity, database compatibility or merge semantics is
listed under Changed, even when it corrects a bug.** Explain which data and
commands are affected, so users know to review compatibility and migration
requirements. Describe the correction in that entry rather than duplicating
it under Fixed.

Entries are short publication-ready explanations, not copied commit subjects
or work-item titles. Exclude investigation work, paper and docs-only
edits, CI and packaging chores, refactors, tests or fixtures added for their
own sake, and dependency changes with no user-visible effect. If any of those
ships a user-visible capability or fix, describe that effect instead.

Do not publish internal tracking references or locate defects in another
project that have not been fixed upstream. The changelog is part of the public
record, and the intention is to ship it in release packages; today's packages
do not include it.

## Consequences

- Release notes require editorial review; automation can collect candidates
  but cannot decide that every completed change deserves an entry.
- Group related effects so a release remains readable. Absence of internal
  work from the changelog is correct, not missing attribution.
- Backfilling older releases is a separate maintainer decision. Git history
  remains the implementation record; this ADR authorizes no history rewrite
  or replacement of already-published archives.
- Deprecated and Security headings are not used. A security fix is a Fixed
  entry, published only once fixed; if it involves another project, wait
  until the fix has landed upstream. Nothing is deprecated yet; if something
  is, add the Deprecated heading then.
- Merge and incident evidence stays in its design and incident records.
  Entries can point to that evidence without becoming incident reports.

Origin: adaptation of knots ADR-0004, itself ported from aurora-lint
ADR-0009, restated for todo-sqlite-cli's user-visible behavior.

Source record: [knots ADR](https://github.com/brandon-arrendondo/knots/blob/7dc71958c024b08726de5a095fa4d94abddbce95/docs/adr/0004-changelog-is-for-users.md).

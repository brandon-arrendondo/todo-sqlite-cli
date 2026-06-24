"""
Invoke tasks for todo-sqlite-cli development.

Usage:
    invoke check          # Run pre-commit hooks on all files
    invoke build          # Build in release mode
    invoke test           # Run all tests
    invoke bump-version   # Bump version across all files (reads Cargo.toml)

Install invoke: pip install invoke
"""

import datetime
import re
from pathlib import Path

from invoke import task

# A semver core like 1.8.0 (no pre-release / build metadata).
SEMVER = r"\d+\.\d+\.\d+"


def _read_cargo_version():
    cargo = Path("Cargo.toml").read_text()
    # Match the package version line (anchored at start of line) — not the
    # `version = "..."` fields inside dependency tables.
    match = re.search(r'^version = "([^"]+)"', cargo, re.MULTILINE)
    if not match:
        raise RuntimeError("Could not find version in Cargo.toml")
    return match.group(1)


# Files that embed THIS crate's own version, with the pattern that locates it
# and a replacement template ({new} = new version, {date} = today YYYY-MM-DD).
#
# Patterns match any semver (not just the current one) so a bump heals any
# existing drift instead of silently skipping an out-of-sync file.
#
# NOT touched (intentionally):
#   - .pre-commit-config.yaml `rev: v1.8.2`  -> that pins knots, not us
#   - mcp_server/pyproject.toml version       -> independently-versioned package
VERSION_FILES = [
    # (path, pattern, replacement-template)
    ("Cargo.toml", r'^(version = ")' + SEMVER + r'(")', r"\g<1>{new}\g<2>"),
    (
        "man/todo-sqlite-cli.1",
        r"(todo-sqlite-cli )" + SEMVER + r'(")',
        r"\g<1>{new}\g<2>",
    ),
    # Refresh the man page date stamp on every bump.
    (
        "man/todo-sqlite-cli.1",
        r'(\.TH TODO-SQLITE-CLI 1 ")\d{4}-\d{2}-\d{2}(")',
        r"\g<1>{date}\g<2>",
    ),
]


@task
def bump_version(c, new_version=None):
    """Bump this crate's version across every file that embeds it.

    Reads the current version from Cargo.toml. With no --new-version, prints
    the current version and the files that would change (dry run). Otherwise
    rewrites Cargo.toml, the man page version, and the man page date.

    Args:
        new_version: Target version string, e.g. 1.8.1 (no leading 'v').
    """
    current = _read_cargo_version()

    if not new_version:
        print(f"Current version (Cargo.toml): {current}")
        print("\nFiles that would be updated:")
        for path, *_ in VERSION_FILES:
            print(f"  {path}")
        print("\nRun: invoke bump-version --new-version X.Y.Z")
        return

    if not re.fullmatch(SEMVER, new_version):
        raise SystemExit(f"--new-version must look like X.Y.Z, got '{new_version}'")

    today = datetime.date.today().isoformat()
    changed = []

    for path, pattern, tmpl in VERSION_FILES:
        p = Path(path)
        if not p.exists():
            continue
        text = p.read_text()
        replacement = tmpl.format(new=new_version, date=today)
        updated = re.sub(pattern, replacement, text, flags=re.MULTILINE)
        if updated != text:
            p.write_text(updated)
            changed.append(path)

    if changed:
        print(f"Bumped -> {new_version} in:")
        for f in sorted(set(changed)):
            print(f"  {f}")
        print(
            "\nNext: review `git diff`, commit, then "
            f"`git tag v{new_version} && git push && git push origin v{new_version}`"
        )
    else:
        print("No version strings matched — nothing changed.")


@task
def check(c):
    """Run pre-commit hooks on all files."""
    c.run("pre-commit run --all-files", pty=True)


@task
def build(c, release=False):
    """Build the project.

    Args:
        release: Build in release mode (default: debug).
    """
    cmd = "cargo build"
    if release:
        cmd += " --release"
    c.run(cmd, pty=True)


@task
def test(c):
    """Run all tests."""
    c.run("cargo test", pty=True)

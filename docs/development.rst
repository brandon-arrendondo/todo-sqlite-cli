Development and verification
============================

Read the repository's ``AGENTS.md`` and ``CLAUDE.md`` before contributing.
Policy is kept in AGENTS.md; this guide describes setup and verification.
Run commands from the directory containing Cargo.toml.

The repository toolchain tracks stable and requests rustfmt, Clippy and
rust-analyzer. A C compiler is needed for bundled SQLite. The optional MCP
wrapper requires Python 3.11 or later.

.. code-block:: sh

   cargo build --locked
   cargo test --locked
   pre-commit install
   pre-commit run --all-files

Install pre-commit before those commands, for example in a Python virtual
environment. The first hook run needs network access to install pinned hook
environments. All-files checks include file hygiene, rustfmt, Clippy,
Knots complexity and agent/commit guard regressions. There is no coverage
threshold configured in this repository.

Tests use temporary databases. Never use a maintainer task database for
verification, including seemingly read-only commands that can migrate it.
For manual examples, pass ``--db`` pointing inside a fresh temporary directory.

Documentation build
-------------------

.. code-block:: sh

   python3 -m venv /tmp/todo-docs-venv
   /tmp/todo-docs-venv/bin/python -m pip install sphinx sphinx-rtd-theme
   /tmp/todo-docs-venv/bin/sphinx-build -n -W --keep-going -b html docs /tmp/todo-docs-html
   groff -man -Tutf8 man/todo-sqlite-cli.1 > /tmp/todo-sqlite-cli-man.txt

The Sphinx build treats warnings and unresolved references as failures. Check
CLI examples against ``src/cli.rs`` and the relevant command implementation;
help alone can contain stale prose. Verify MCP tool counts/parameters against
``mcp_server/server.py`` and ``cli_ops.py``.

MQTT scenario checks
--------------------

The optional scenario runner needs mosquitto, mosquitto_passwd, ``mcp<2`` and
paho-mqtt, and uses a throwaway local broker:

.. code-block:: sh

   python3 mcp_server/tests/mqtt_fleet_scenario.py

It skips with exit 0 when prerequisites are absent; a zero exit alone does not
prove the scenario ran. ``--compat-ref <git-ref>`` adds mixed-version checks.
Run this when changing MQTT behavior. Documentation-only changes need no live
broker or production database access.

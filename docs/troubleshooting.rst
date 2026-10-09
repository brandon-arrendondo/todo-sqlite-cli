Troubleshooting
===============

No database found
-----------------

Check the :doc:`usage` precedence and your cwd. A marker's relative path is
relative to the marker directory. Explicit ``--db`` is useful for scripts.
``init`` does not follow an environment-selected or ancestor-marker database.

No next task, but work remains
------------------------------

``next`` omits gates and blocked partial/pending tasks. Use ``list --kind gate``
and ``list --status active --json`` to inspect gates and blocked flags. A
dependency closed as rejected does not unblock dependents.

An ID selects multiple tasks or a different task
------------------------------------------------

Display IDs are aliases. Use UUIDs for persistent references, ``doctor`` to
inspect duplicates, and ``renumber <full-uuid> <new-id>`` to repair an alias.
Do not rely on a deleted numeric ID never being reused.

Schema mismatch during merge
----------------------------

Back up the inputs and inspect their versions without independently migrating
copies of a pre-UUID backlog. Establish a shared migrated snapshot; matching
version numbers alone cannot repair incompatible UUID histories. See
:doc:`merge-engine` and :doc:`incidents`. Normal read commands can migrate
schemas, so they are not a side-effect-free inspection method.

MCP startup or tool availability
--------------------------------

Install the Python package with ``mcp>=1,<2`` and verify that the CLI is on PATH,
or set ``TODO_SQLITE_CLI_BIN``. Standalone mode exposes 13 task tools.
MQTT mode adds only the tools valid for that role. MQTT write requests from a
worker are pending requests, not confirmations that a task mutation happened;
poll check_request for the result. Snapshot updates replace the worker replica.

Standalone database resolution follows the environment and markers; MQTT
mode passes an explicit role database override. Server config must refer to
local files and available environment variables. See the README's MQTT section
for configuration, ACLs, presence, directives and connection diagnostics.

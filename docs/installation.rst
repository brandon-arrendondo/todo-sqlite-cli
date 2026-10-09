Installation
============

The published Rust package installs the CLI executable:

.. code-block:: sh

   cargo install todo-sqlite-cli --locked

SQLite is bundled through rusqlite; a system SQLite installation is not
required. Bundling SQLite does not imply the whole executable is statically
linked. A compiler toolchain is needed to build the bundled C code.

For a source checkout, run these commands in the repository root:

.. code-block:: sh

   cargo build --release --locked
   ./target/release/todo-sqlite-cli --help
   cargo install --path . --locked

``rust-toolchain.toml`` selects the stable Rust channel, rather than a fixed
version. Cargo.toml declares ``rust-version = "1.75"``; CI builds on stable
and does not separately test that declared minimum against locked dependencies.
See :doc:`development` for repository checks.

Cargo installation builds the binary; it does not install a man page or the
optional Python MCP server. Release CI stages the manual, MCP directory,
licence records and SBOM alongside Linux/Windows binaries, and prepares Linux
deb, rpm and AppImage packages. Check repository releases for available assets.
The standalone manual source is ``man/todo-sqlite-cli.1``.

The optional MCP wrapper needs Python 3.11 or later and ``mcp>=1,<2``. Install
from a source checkout or an extracted release archive:

.. code-block:: sh

   python3 -m venv .venv
   .venv/bin/python -m pip install ./mcp_server
   .venv/bin/todo-mcp-server

For MQTT support, install ``./mcp_server[mqtt]`` instead. The wrapper invokes
``todo-sqlite-cli`` from PATH, or the path in ``TODO_SQLITE_CLI_BIN``.
It is a separate optional service, not part of CLI execution.

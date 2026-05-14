# Alternatives

This document compares `todo-sqlite-cli` to other TODO/task tools — both
Rust crates and tools coding agents already encounter. It exists so you
can decide whether this tool fits your workflow, and to be honest about
where existing options would serve you better.

## TL;DR — where this tool sits

| Property                       | `todo-sqlite-cli`                  |
|--------------------------------|------------------------------------|
| Scope                          | **Per-project** (walk-up marker, like `.git`) |
| Storage                        | Single SQLite file                 |
| Transport                      | Plain CLI (no MCP, no daemon, no TTY) |
| Primary audience               | Coding agents (Claude Code & friends) |
| Secondary audience             | Humans, shell scripts, CI jobs     |
| Runtime                        | Single static binary               |

No other tool surveyed combines all five rows. The closest functional
overlap is **`todolist-mcp`** (SQLite + agent-aimed, but MCP transport
and a single global DB). The closest spiritual overlap is **Claude
Code's built-in TodoWrite**, which is ephemeral and per-session.

## Direct comparisons

### Claude Code built-in tasks (`TodoWrite` / `~/.claude/tasks`)
[Docs](https://code.claude.com/docs/en/agent-sdk/todo-tracking) · built
into the Claude Agent SDK.

The closest *spiritual* competitor: agents already have a TODO list.
The key gap is durability — the built-in list lives inside the session
message stream and does not persist across `/clear`, between branches,
or across repos. `todo-sqlite-cli` is the **durable, queryable,
project-scoped** complement; you can use both together (built-in for
intra-turn scratchpad, this for cross-session state).

### todolist-mcp ([wdm0006](https://github.com/wdm0006/todolist-mcp))
Python · SQLite · MCP server · also ships a FastAPI/HTMX kanban UI.

Closest *functional* competitor: SQLite-backed, explicitly agent-aimed.
Differences:
- MCP transport (requires an MCP-aware client and a long-lived server
  process) vs a plain CLI you can call from any shell, script, CI job,
  or non-MCP agent.
- Single global DB vs per-project walk-up marker.
- Python runtime + web UI vs single static binary.

Pick `todolist-mcp` if you want a web kanban view and your agents are
all MCP-aware. Pick this if you want zero runtime deps and per-project
isolation.

### mcp-shrimp-task-manager ([cjo4m06](https://github.com/cjo4m06/mcp-shrimp-task-manager))
TypeScript/Node · JSON + React web UI · MCP server.

Opinionated agent-workflow engine: chain-of-thought workflows, task
decomposition, "task memory." Much heavier than this tool, with a
methodology baked in. Pick it if you want the methodology; pick this
if you just want a durable typed backing store and your own workflow
in your `CLAUDE.md`.

## Rust crates on crates.io (adjacent, not direct)

No crate on crates.io combines SQLite storage, project-local DB
discovery, and agent-friendly framing. Closest neighbors:

- **[taskchampion](https://crates.io/crates/taskchampion)** — the
  SQLite-backed library that powers Taskwarrior 3.x. A *library*, not a
  CLI. If you're building a Rust task tool from scratch and want an
  embeddable storage layer with sync support, look here. Different
  category than this binary.
- **[tod](https://crates.io/crates/tod)** /
  **[doist](https://crates.io/crates/doist)** — unofficial Todoist
  clients. Cloud-backed (Todoist API), so they're great if your tasks
  are personal and you want them on your phone too, but offline-only
  and agent-context-bound is the opposite of their model.
- **[terminalist](https://crates.io/crates/terminalist)** — TUI Todoist
  client. Requires a TTY, which makes it a non-starter for agents.

## Established CLI task managers (for context)

These are well-loved, well-maintained, and not aimed at coding agents.
They're listed so you can pick one if your use case is personal task
tracking rather than agent state.

- **[Taskwarrior](https://taskwarrior.org/)** (C++; v3 uses
  taskchampion → SQLite). The 800-pound gorilla. Personal global task
  DB with a rich UDA/filter DSL. Pick Taskwarrior if your problem is
  personal task tracking and you want power.
- **[dstask](https://github.com/naggie/dstask)** (Go). Tasks as
  markdown/YAML files in a git repo. Lovely for solo developers who
  want git as the sync mechanism. Personal-global, not agent-aimed.
- **[todo.txt-cli](https://github.com/todotxt/todo.txt-cli)** (Bash).
  Canonical minimal format. One plain text file, no schema, no deps.
  Pick it for maximum portability; this tool for typed status,
  dependencies, and timestamped completion.

## Out of scope (different category)

- **Session/conversation memory tools** (e.g.
  [claude-mem](https://github.com/thedotmack/claude-mem)) compress and
  re-inject past Claude sessions. Complementary, not competitive — they
  store conversation, not task state.
- **Todoist MCP bridges** (e.g.
  [todoist-mcp-server](https://github.com/abhiz123/todoist-mcp-server))
  expose hosted Todoist to MCP-aware agents. Pick those if you already
  live in Todoist and want your agent to read/write it.

## When *not* to use this tool

- Your tasks are personal, not project-scoped → Taskwarrior or Todoist.
- You want a TUI / kanban / mobile sync → terminalist, dstask, Todoist.
- Your agents are all MCP-native and you want a web UI →
  `todolist-mcp`.
- You only need within-session task tracking → Claude Code's built-in
  `TodoWrite` is already there.

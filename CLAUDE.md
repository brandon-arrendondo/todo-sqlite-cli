# Developer guide

@AGENTS.md

Read [README.md](README.md) for repository structure, usage, and contribution instructions.

Read [docs/development.rst](docs/development.rst) for setup and verification.
Build with `cargo build --locked`, test with `cargo test --locked`, and run
`pre-commit run --all-files`. Use temporary databases in tests; never operate on a maintainer task database as part of verification.

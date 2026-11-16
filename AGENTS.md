# AGENTS.md

Working conventions for this repository. This file is the source of truth for every coding agent; [CLAUDE.md](CLAUDE.md) imports it.

If `prd.md` and `plan.md` exist locally, read them for requirements, architecture and phases before making changes. They are private: git-ignored, never committed, and never linked from tracked files.

## Overview

Parameters is a graph-based, English-like workflow automation language written in Rust. The JSON workflow document is the single source of truth; the text language and the graph editor are two views of it. The UI is Dioxus 0.7 fullstack (web, desktop, mobile, server). PostgreSQL is the only database, for both local and managed modes.

## Commands

- `cargo test --workspace --features parameters-app/server` — run all tests, including server-only code
- `cargo clippy --workspace --all-targets --features parameters-app/server -- -D warnings` — lint
- `cargo clippy -p parameters-app --target wasm32-unknown-unknown --features web -- -D warnings` — lint the web build
- `cargo fmt --all` — format
- `UPDATE_GOLDEN=1 cargo test -p parameters-core -p parameters-lang` — regenerate golden files (test fixtures, `schema/workflow.v1.json`, `docs/language.md`); review the diff before committing
- `cargo bench -p parameters-lang` — parser benchmark; fails if parsing and validating 1,000 steps takes over 5 ms
- `cargo bench -p parameters-engine` — scheduler benchmark; fails above 50 µs of overhead per step
- `cargo run -p parameters-cli -- check|fmt|convert|run <file>` — the `parameters` command line tool
- `cargo deny check` — advisories, licenses, sources
- `dx serve --package parameters-app --platform web` — run the app
- `docker compose up -d` — start PostgreSQL (needs `.env`, see `.env.example`)

## Crates

Dependencies only point toward `core`:

- `lang`, `engine` and `store` depend on `core`.
- `service` depends on `core`, `lang`, `engine` and `store`.
- `mcp` depends on `core`, `lang` and `service`.
- `app` depends on `core` and `lang`, plus `service` and `mcp` behind its `server` feature.
- `cli` depends on `core`, `lang` and `engine`.

- `core` and `lang` are pure (no I/O) and must compile to `wasm32-unknown-unknown`.
- `engine`, `store`, `service` and `mcp` are server-only. `app` may only depend on them behind its `server` feature, and server-only code is gated with `#[cfg(feature = "server")]`. Anything else ships in the public WASM bundle.

## Rules

- Follow SOLID, DRY, YAGNI and KISS. Prefer functionality and security over performance when they conflict.
- No code comments unless something cannot be made self-explanatory through naming and structure.
- All I/O is async; never block the runtime. Pick the best algorithm and data structure for each job, and avoid work that grows quadratically with user-sized input.
- Keep dependencies minimal. Every new crate needs a reason over writing the code by hand; add it to `[workspace.dependencies]`.
- Never commit secrets, `.env` files or personal data. Configuration is server-only and never returned to clients.
- Language keywords are reserved (`KEYWORDS` in `crates/core/src/name.rs`); names may not use them.
- `docs/language.md` and `schema/workflow.v1.json` are generated from code; change the code, then regenerate them.
- Built-in node field and output names must be valid names (not keywords); the registry test enforces this.
- Nothing secret may leave the engine unmasked: reports, events, logs and errors all go through the run's redactor.
- Conventional Commits with a scope, such as `feat(core): ...`. Types: `feat`, `fix`, `docs`, `chore`, `style`, `refactor`, `ci`, `test`, `revert`, `perf`. No AI attribution lines in commits or pull requests.
- UI: square edges (no border radius), theme tokens in CSS custom properties, light and dark modes.

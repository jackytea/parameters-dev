# AGENTS.md

Working conventions for this repository. This file is the source of truth for every coding agent; [CLAUDE.md](CLAUDE.md) imports it.

If `prd.md` and `plan.md` exist locally, read them for requirements, architecture and phases before making changes. They are private: git-ignored, never committed, and never linked from tracked files.

## Overview

Parameters is a graph-based, English-like workflow automation language written in Rust. The JSON workflow document is the single source of truth; the text language and the graph editor are two views of it. The UI is Dioxus 0.7 fullstack (web, desktop, mobile, server). PostgreSQL is the only database, for both local and managed modes.

## Commands

- `cargo test --workspace --features parameters-app/server` — run all tests, including server-only code; the database tests need `DATABASE_URL` pointing at a PostgreSQL server where the user can create databases (each test gets its own)
- `cargo clippy --workspace --all-targets --features parameters-app/server -- -D warnings` — lint
- `cargo clippy -p parameters-app --target wasm32-unknown-unknown --features web -- -D warnings` — lint the web build
- `cargo clippy -p parameters-app --features desktop -- -D warnings` (and `--features mobile`) — lint the native builds; on Linux this needs the WebKitGTK headers listed in `.github/workflows/ci.yaml`
- `cargo fmt --all` — format
- `UPDATE_GOLDEN=1 cargo test -p parameters-core -p parameters-lang` — regenerate golden files (test fixtures, `schema/workflow.v1.json`, `docs/language.md`); review the diff before committing
- `cargo bench -p parameters-lang` — parser benchmark; fails if parsing and validating 1,000 steps takes over 5 ms
- `cargo bench -p parameters-engine` — scheduler benchmark; fails above 50 µs of overhead per step
- `cargo run -p parameters-cli -- check|fmt|convert|run <file>` — the `parameters` command line tool
- `cargo deny check` — advisories, licenses, sources
- `cargo sqlx prepare --workspace -- --all-targets --features parameters-app/server` — regenerate `.sqlx` after changing a query or migration (needs `sqlx-cli` 0.9 and a migrated `DATABASE_URL`); commit the result, CI checks it
- `dx serve --package parameters-app --platform web` — run the app with hot reload (needs `DATABASE_URL` and `PARAMETERS_ENCRYPTION_KEY`)
- `dx bundle --package parameters-app --platform web --release` — build the server and the web client, as the Dockerfile does
- `docker compose up -d` — start PostgreSQL and the server on `127.0.0.1:8080` (needs `.env`, see `.env.example`); `docker compose up -d database` starts only PostgreSQL

## Crates

Dependencies only point toward `core`:

- `lang` and `engine` depend on `core`.
- `api` (wire types shared by the server, MCP and clients) depends on `core` and `lang`.
- `store` depends on `api`.
- `service` depends on `api`, `core`, `lang`, `engine` and `store`.
- `mcp` depends on `api`, `core`, `lang` and `service`.
- `app` depends on `api`, `core` and `lang`, plus `service` and `mcp` behind its `server` feature.
- `cli` depends on `core`, `lang` and `engine`.

- `core`, `lang` and `api` are pure (no I/O) and must compile to `wasm32-unknown-unknown`.
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
- All SQL lives in `store`, written with the compile-time checked `sqlx::query!` macros. Migrations in `crates/store/migrations` are append-only: never edit one that has been committed.
- Authorization and validation live in `service`. Server functions and MCP tools are thin adapters that pass an `Actor` and map `ApiError`; they never query the store directly.
- MCP tools (`crates/mcp/src/tools.rs`) take their input schema from the struct that parses their arguments, report failures as tool errors (`isError`) so agents can correct themselves, and return run data through `output::run`, which clips it and labels it as untrusted.
- Only a person in the app (`Via::Ui`) may approve a workflow that has privileged steps (`Node::privileged`); every changed version needs a new approval.
- Local mode listens on loopback only and rejects requests whose `Host` or `Origin` is not localhost.
- The mode decides the `Capabilities` (`crates/service/src/policy.rs`): sandbox, limits and rate. Check capabilities there, never `is_managed()` scattered through features.
- Every service method that takes an `Actor` starts with `actor.require(Scope::…)`; every query is scoped by `actor.user`. The app's auth middleware (`crates/app/src/server/auth.rs`) is the only place that turns cookies and tokens into an `Actor`.
- Tokens, sessions and sign-in links are random 32-byte values; only their SHA-256 hashes are stored.
- Conventional Commits with a scope, such as `feat(core): ...`. Types: `feat`, `fix`, `docs`, `chore`, `style`, `refactor`, `ci`, `test`, `revert`, `perf`. No AI attribution lines in commits or pull requests.
- UI: square edges (no border radius), theme tokens in CSS custom properties, light and dark modes.
- Native-only code (`crates/app/src/native.rs`, `ui::device::native`) is gated by the `desktop` and `mobile` features; native builds talk to the server with a bearer session from the device sign-in (`/auth/device`), never with cookies.
- Releases come only from the tag-triggered release workflow; pin every action to a commit SHA, and keep signing optional behind repository secrets.
- UI data loads on the client only (`on_client()` in `crates/app/src/ui/common.rs`), so the server-rendered page and the first client render match. Live views use the server-sent event endpoints and reconnect after a pause.
- The editor keeps one `EditorState` (`crates/app/src/ui/editor/mod.rs`). Graph and inspector edits go through `parameters_core::edit` and reprint the text; text edits replace the workflow when they parse. Keep editing rules in `core::edit`, where they are tested.

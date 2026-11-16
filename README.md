# Parameters

An open-source, graph-based, English-like programming language for workflow automation. Describe steps and the dependencies between them in code or in a graph editor; both are views of the same shareable JSON document.

Parameters can be self-hosted or used through the managed service at [parameters.dev](https://parameters.dev).

> Status: early development.

## Requirements

- Rust (the exact version is pinned in [rust-toolchain.toml](rust-toolchain.toml); on Windows use the MSVC toolchain)
- [Dioxus CLI](https://dioxuslabs.com/) 0.7: `cargo install --locked dioxus-cli@0.7.10`
- Docker, for PostgreSQL

## Getting started

```sh
cp .env.example .env
docker compose up -d
cargo test --workspace
dx serve --package parameters-app --platform web
```

Fill in `.env` before starting the database: `POSTGRES_DB`, `POSTGRES_USER`, a long random `POSTGRES_PASSWORD`, and a `DATABASE_URL` built from them (`postgres://USER:PASSWORD@127.0.0.1:PORT/DB`). `POSTGRES_PORT` defaults to `5432`. `.env` is ignored by git and must never be committed.

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/core` | Workflow model, JSON format, validation |
| `crates/lang` | Language lexer, parser and printer |
| `crates/engine` | Workflow execution |
| `crates/store` | PostgreSQL persistence |
| `crates/service` | Use cases shared by the UI and MCP |
| `crates/mcp` | MCP server for AI agents |
| `crates/app` | Dioxus fullstack app (web, desktop, mobile, server) |
| `crates/cli` | `parameters` command-line tool |

## License

[AGPL-3.0](LICENSE)

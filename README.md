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
```

Fill in `.env` first:

- `POSTGRES_DB`, `POSTGRES_USER` and a long random `POSTGRES_PASSWORD`.
- `PARAMETERS_ENCRYPTION_KEY`: 64 hexadecimal characters, from `openssl rand -hex 32`. Secrets are encrypted with it, so keep a copy somewhere safe; without it they cannot be read.
- `DATABASE_URL` (`postgres://USER:PASSWORD@127.0.0.1:PORT/DB`), only needed to run the server or tests outside Docker.
- `POSTGRES_PORT` (default `5432`) and `PARAMETERS_PORT` (default `8080`) are optional.

`.env` is ignored by git and must never be committed. `docker compose up -d` starts PostgreSQL and the Parameters server at <http://localhost:8080>; both are published on `127.0.0.1` only. `docker compose up -d database` starts only PostgreSQL, for development:

```sh
cargo test --workspace --features parameters-app/server
dx serve --package parameters-app --platform web
```

## The language

Workflows are written in `.parameters` files or as JSON, and either one converts to the other. See the [language reference](docs/language.md) for the syntax and a full example. The JSON format is described by [schema/workflow.v1.json](schema/workflow.v1.json).

```sh
cargo run -p parameters-cli -- check deploy.parameters
cargo run -p parameters-cli -- fmt deploy.parameters
cargo run -p parameters-cli -- convert deploy.parameters --output deploy.json
cargo run -p parameters-cli -- run examples/hello.parameters --input name=Ada
```

`run` prints each step as it starts and finishes, then the workflow's outputs as JSON. Steps run in parallel unless one depends on another. Secrets are read from environment variables named `PARAMETERS_SECRET_<NAME>` and are masked in everything the run prints. See [examples/](examples) for workflows you can run locally.

## The server

Self-hosted Parameters has no accounts, and its workflows can run commands on the machine, so the server only listens on a loopback address and rejects requests whose `Host` or `Origin` is not `localhost`. A workflow that runs commands or uses files must be reviewed and approved in the app before it runs, and again whenever it changes.

Runs are queued in PostgreSQL. If the server stops during a run, the run is queued again when a server comes back, and it is marked failed after three interrupted attempts. A run starts again from the beginning, so steps can run more than once.

The server is configured with environment variables:

| Variable | Default | Purpose |
| --- | --- | --- |
| `DATABASE_URL` | the `PGHOST`, `PGUSER`, `PGPASSWORD` and `PGDATABASE` variables | PostgreSQL connection |
| `PARAMETERS_ENCRYPTION_KEY` | required | 32-byte key, as hex, that encrypts stored secrets |
| `PARAMETERS_ADDRESS` | `127.0.0.1:8080` | Address to listen on; must be loopback |
| `PARAMETERS_ALLOW_NON_LOOPBACK` | `false` | Allows a non-loopback address. Only for containers whose port is published on `127.0.0.1`, as in `docker-compose.yaml` |
| `PARAMETERS_WORKSPACE` | the current directory | Directory that shell and file steps work in |
| `PARAMETERS_MAX_RUNS` | `4` | Runs executed at the same time |
| `PARAMETERS_DATABASE_CONNECTIONS` | `10` | Size of the database connection pool |

The JSON API lives under `/api`:

| Endpoint | Purpose |
| --- | --- |
| `GET`, `POST /api/workflows` | List workflows, or create one from `{"source": {"format": "text" or "json", "content": "..."}}` |
| `GET`, `PUT`, `DELETE /api/workflows/{id}` | Read, update (with the expected `version`) or delete a workflow |
| `POST /api/check` | Validate a source without saving it |
| `GET /api/workflows/{id}/review`, `POST .../approve` | Review the commands, files, hosts and secrets a workflow uses, and approve it |
| `POST /api/workflows/{id}/runs` | Start a run with `{"inputs": {...}}` |
| `GET /api/runs`, `GET /api/runs/{id}`, `POST /api/runs/{id}/cancel` | List, read and cancel runs |
| `GET /api/runs/{id}/events` | Live run updates as server-sent events |
| `GET /api/secrets`, `PUT`, `DELETE /api/secrets/{name}` | List secret names, set a secret with `{"value": "..."}`, delete one. Values are never returned |
| `GET /api/nodes`, `GET`, `PUT`, `DELETE /api/nodes/{type}`, `POST /api/nodes/{type}/rename`, `GET /api/nodes/{type}/users` | The custom node library |
| `POST /api/node-imports/plan`, `POST /api/node-imports` | Preview and import custom nodes, renaming any that conflict |

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/core` | Workflow model, JSON format, validation |
| `crates/api` | Types shared by the server, MCP and clients |
| `crates/lang` | Language lexer, parser and printer |
| `crates/engine` | Workflow execution |
| `crates/store` | PostgreSQL persistence |
| `crates/service` | Use cases shared by the UI and MCP |
| `crates/mcp` | MCP server for AI agents |
| `crates/app` | Dioxus fullstack app (web, desktop, mobile, server) |
| `crates/cli` | `parameters` command-line tool |

## License

[AGPL-3.0](LICENSE)

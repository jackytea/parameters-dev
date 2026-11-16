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

`dx serve` needs the same environment as the server (see below), for example `DATABASE_URL` and `PARAMETERS_ENCRYPTION_KEY` exported from `.env`.

## The language

Workflows are written in `.parameters` files or as JSON, and either one converts to the other. See the [language reference](docs/language.md) for the syntax and a full example. The JSON format is described by [schema/workflow.v1.json](schema/workflow.v1.json).

```sh
cargo run -p parameters-cli -- check deploy.parameters
cargo run -p parameters-cli -- fmt deploy.parameters
cargo run -p parameters-cli -- convert deploy.parameters --output deploy.json
cargo run -p parameters-cli -- run examples/hello.parameters --input name=Ada
```

`run` prints each step as it starts and finishes, then the workflow's outputs as JSON. Steps run in parallel unless one depends on another. Secrets are read from environment variables named `PARAMETERS_SECRET_<NAME>` and are masked in everything the run prints. See [examples/](examples) for workflows you can run locally.

## The app

Open <http://localhost:8080> for the home page, which introduces the language, the built-in nodes and the editions. The app has these sections:

- **Workflows** (`/workflows`): create, import (text or JSON, pasted or from a file), duplicate and delete workflows.
- **Editor**: the code and the graph side by side, kept in sync.
  - Build in the graph: add steps, drag a step's right-hand port onto another step to make it wait, click a connection to remove it, and edit the selected step in the inspector.
  - Or write the workflow as text. While the text has a syntax error, the graph is read-only.
  - Changes saved elsewhere, including by an AI agent, appear in open editors as soon as they are saved. If you have unsaved changes, you choose which version to keep.
  - Before a workflow that runs commands or uses files can run, the editor shows what it can do and asks you to approve that version.
  - Run a workflow and watch each step's status on the graph, with its outputs and log below.
- **Runs**: every run, with live status, outputs and logs, and a way to cancel.
- **Nodes** and **Secrets**: the custom node library, including imports that resolve name clashes, and encrypted secrets.

Light and dark themes follow the system unless you pick one. On a narrow screen, switch between the code and the graph.

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
| `POST /api/check`, `POST /api/convert` | Validate a source without saving it, or convert it between text and JSON (`{"source": ..., "to": "text" or "json"}`) |
| `GET /api/node-catalog`, `GET /api/node-catalog/{type}` | Every node a workflow can use, built-in and custom, with its inputs, outputs and an example |
| `GET /api/workflows/{id}/review`, `POST .../approve` | Review the commands, files, hosts and secrets a workflow uses, and approve it |
| `POST /api/workflows/{id}/runs` | Start a run with `{"inputs": {...}}` |
| `GET /api/runs`, `GET /api/runs/{id}`, `POST /api/runs/{id}/cancel` | List, read and cancel runs |
| `GET /api/runs/{id}/events` | Live run updates as server-sent events |
| `GET /api/secrets`, `PUT`, `DELETE /api/secrets/{name}` | List secret names, set a secret with `{"value": "..."}`, delete one. Values are never returned |
| `GET /api/nodes`, `GET`, `PUT`, `DELETE /api/nodes/{type}`, `POST /api/nodes/{type}/rename`, `GET /api/nodes/{type}/users` | The custom node library |
| `POST /api/node-imports/plan`, `POST /api/node-imports` | Preview and import custom nodes, renaming any that conflict |
| `GET`, `POST /api/triggers`, `POST /api/triggers/{id}/enabled`, `DELETE /api/triggers/{id}` | Webhooks and schedules |
| `POST /hooks/{token}` | Start a run through a webhook |

## Triggers

A workflow can start by itself. Add triggers in the editor's Triggers panel, through the API, or with the MCP `create_trigger` tool.

- **Webhooks** get a secret address, `/hooks/whk_…`, shown once. A `POST` to it queues a run, and the request body (up to 256 KiB, JSON or text) becomes the workflow's input named `payload`, if it declares one. A signed webhook also needs `X-Parameters-Signature: sha256=<hex>`, the HMAC-SHA256 of the body with the trigger's signing secret. GitHub's `X-Hub-Signature-256` header is accepted too, so a GitHub webhook can use it directly. Each webhook accepts up to 60 calls a minute.
- **Schedules** use five cron fields in UTC (minute, hour, day of month, month, day of week), with lists, ranges and steps such as `0 9 * * 1-5` or `*/15 * * * *`, or `@hourly`, `@daily`, `@weekly`, `@monthly` and `@yearly`. A schedule that was missed while the server was down runs once when it comes back, not once for every missed time.

Runs started by triggers count as the workflow owner's runs: they need the same approval for commands and files, count against the same limits, and are labelled `webhook` or `schedule` in the Runs list. If a trigger cannot start a run, its last error is shown next to it. In managed mode, a schedule can run at most every 5 minutes.

## Managed mode

The same server runs the hosted service at parameters.dev. Set `PARAMETERS_MODE=managed` and it changes in these ways:

- **Accounts:** people sign in with a link sent by email, or with Google, GitHub or Discord. Sessions last 30 days from the last visit and live in an `HttpOnly`, `SameSite=Lax` cookie, which is also `Secure` over https.
- **Access tokens:** agents and scripts use access tokens (`prm_…`), created on the Account page. Each token is limited to `read`, `write` and/or `run`, can expire, can be revoked, and is shown only once. Only a hash is stored.
- **Isolation:** every workflow, run, secret and custom node belongs to one account, and nothing is visible to another.
- **Sandbox:**
  - Steps that run commands or use files are not run.
  - `http.request` refuses private, loopback, link-local and cloud metadata addresses, including through redirects and DNS names that resolve to them.
- **Limits:** 500 steps per workflow, 15 minutes per run, 5 queued or running runs, and 600 requests a minute per account.
- **Requests:** writes that use the session cookie must come from the app's own origin, and responses carry a content security policy and other security headers.

| Variable | Purpose |
| --- | --- |
| `PARAMETERS_MODE` | `local` (default) or `managed` |
| `PARAMETERS_PUBLIC_URL` | The address people use, such as `https://parameters.dev`; used in sign-in links and redirects |
| `PARAMETERS_SMTP_URL`, `PARAMETERS_MAIL_FROM` | SMTP server for sign-in emails, such as `smtps://user:password@smtp.example.com:465`, and the sender |
| `PARAMETERS_GOOGLE_CLIENT_ID`, `PARAMETERS_GOOGLE_CLIENT_SECRET` | Google sign-in; GitHub and Discord use `PARAMETERS_GITHUB_…` and `PARAMETERS_DISCORD_…`. Register `{PARAMETERS_PUBLIC_URL}/auth/{google,github,discord}/callback` as the redirect address |
| `PARAMETERS_TRUST_PROXY` | `true` when a reverse proxy in front of the server sets `X-Forwarded-For` |

Managed mode needs email or at least one sign-in provider. It may listen on any address, but it expects https in front of it.

## AI agents (MCP)

The server speaks the [Model Context Protocol](https://modelcontextprotocol.io) at `http://localhost:8080/mcp` (Streamable HTTP), so an AI agent can read the language reference, write workflows, run them and read the results. To add it to Claude Code, for example:

```sh
claude mcp add --transport http parameters http://localhost:8080/mcp
```

| Kind | Name | Purpose |
| --- | --- | --- |
| Resource | `parameters://reference/language` | The language reference |
| Resource | `parameters://schema/workflow` | The JSON Schema of the workflow format |
| Resource template | `parameters://nodes/{type}` | One node's description, inputs, outputs and example |
| Tools (read) | `validate`, `convert`, `list_nodes`, `describe_node`, `list_workflows`, `get_workflow`, `get_run` | Check, convert and inspect |
| Tools (change) | `create_workflow`, `update_workflow`, `run_workflow`, `cancel_run` | Save, run and stop workflows |

In managed mode, connect with an access token from the Account page: `claude mcp add --transport http parameters https://parameters.dev/mcp --header "Authorization: Bearer prm_…"`.

Agents cannot delete workflows or approve them. A workflow that runs commands or uses files only runs after a person approves that exact version in the app, and `run_workflow` tells the agent what is waiting for approval. Changes an agent makes are recorded as made through MCP. Run results are returned as data, with long values shortened, and labelled so the agent does not treat them as instructions. Custom nodes from the library that a workflow uses are copied into it when it is saved.

## Desktop and mobile apps

The desktop apps (Windows, macOS, Linux) and the mobile apps (iOS, Android) are the same interface, running natively. They connect to a Parameters server: parameters.dev by default, or any server you choose on desktop (for example `http://localhost:8080` for the server on your computer). Builds can change the default with the `PARAMETERS_SERVER_URL` environment variable at build time.

On a server with accounts, the app asks you to sign in with your browser:

1. The app shows a short code and opens `/connect?code=…` in your browser.
2. Sign in there if you are not already, check that the code matches, and choose Allow.
3. The app picks up its session by itself.

The session is stored in a file in the app's own data folder that only your user account can read, and signing out in the app ends it.

```sh
dx serve --package parameters-app --platform desktop
dx bundle --package parameters-app --platform desktop --release
```

## Releases

Pushing a tag such as `v0.1.0` runs [the release workflow](.github/workflows/release.yaml). It builds and publishes:

- the `parameters` command line tool for Linux (x86-64 and ARM), macOS (Apple silicon) and Windows;
- the desktop app for Linux (`.deb`, `.rpm`, `.AppImage`), macOS and Windows;
- the Android app (`.apk`) and an iOS build;
- the server image, `ghcr.io/<owner>/<repository>`, with an SBOM.

Every file in a release is listed in `SHA256SUMS`. Each file also has a signed build-provenance attestation, made with GitHub's Sigstore-based attestations and needing no stored keys. Check a download with `gh attestation verify <file> --repo <owner>/<repository>`, and the image with `gh attestation verify oci://ghcr.io/<owner>/<repository>:<version> --repo <owner>/<repository>`.

Platform code signing runs when its repository secrets are set:

| Platform | Secrets |
| --- | --- |
| macOS (signing and notarization) | `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` |
| Windows | `WINDOWS_CERTIFICATE` (base64 `.pfx`), `WINDOWS_CERTIFICATE_PASSWORD` |
| Android | `ANDROID_KEYSTORE` (base64), `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` |

Without them, those builds are published unsigned. The iOS build is always unsigned, because installing it needs an Apple provisioning profile.

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

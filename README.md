# MCP Hub

A self-hosted, multi-user gateway that turns MCP servers into remote endpoints your AI agents can reach from anywhere.

![Release](https://github.com/duckysmacky/mcp-hub/actions/workflows/release.yml/badge.svg)
![Version](https://img.shields.io/badge/version-1.0.0-blue)
![Docker](https://img.shields.io/badge/docker-ghcr.io-2496ED?logo=docker&logoColor=white)
![OAuth](https://img.shields.io/badge/auth-OAuth_2.1-green)
![Rust](https://img.shields.io/badge/backend-Rust-orange?logo=rust)
![SvelteKit](https://img.shields.io/badge/frontend-SvelteKit-FF3E00?logo=svelte&logoColor=white)

## Table of contents

- [MCP Hub](#mcp-hub)
  - [Table of contents](#table-of-contents)
  - [About](#about)
  - [Features](#features)
    - [Instances](#instances)
    - [Catalog](#catalog)
    - [OAuth and connected apps](#oauth-and-connected-apps)
    - [Credential storage](#credential-storage)
    - [Web interface](#web-interface)
  - [Installation](#installation)
    - [Requirements](#requirements)
    - [Quick start](#quick-start)
    - [Configuration](#configuration)
    - [Self-hosting on a VPS](#self-hosting-on-a-vps)
    - [Updating](#updating)
    - [Using an external Postgres](#using-an-external-postgres)
    - [Backups](#backups)
    - [Operator commands](#operator-commands)
  - [Setting up the catalog](#setting-up-the-catalog)
    - [Adding a server](#adding-a-server)
    - [Runtimes](#runtimes)
    - [Environment variables](#environment-variables)
    - [The lockfile](#the-lockfile)
    - [HTTP mode](#http-mode)
    - [Native binaries](#native-binaries)
    - [Custom servers](#custom-servers)
  - [Using the hub](#using-the-hub)
    - [Creating an instance](#creating-an-instance)
    - [Connecting Claude](#connecting-claude)
    - [Connecting other agents](#connecting-other-agents)
  - [Security](#security)
  - [Development](#development)

## About

Most MCP servers are local programs that talk over stdio. That works on the one machine they are installed on, and nowhere else: not on your phone, not in claude.ai, and not on a teammate's laptop. Each device needs its own install and its own copy of your credentials.

MCP Hub runs those servers for you on one host and gives each user a remote endpoint:

```
https://mcp-hub.example.com/mcp/<slug>
```

An operator decides which servers are available. A user signs in, picks one, fills in their own credentials, and connects any MCP client to that URL. When a request arrives, the hub works out which user it belongs to, and starts or reuses a process for them with their credentials.

It's built around a few ideas:

- **The operator chooses what runs.** Servers come from a catalog file with pinned versions and frozen dependencies. Users can't supply commands, arguments, or packages unless the operator opts in.
- **Credentials belong to each user.** Every user's values are encrypted under a key that only they can unlock, and secrets can be set but never read back.
- **Clients sign in properly.** Agents authenticate with OAuth 2.1 and a consent page, so you never paste a long-lived token into a config file.
- **No code runs at install time, and nothing is fetched at request time.** Packages are installed ahead of time with their install scripts off. There is no `npx` or `bunx` when a process starts.
- **It's easy to host.** One container with everything inside, an optional bundled Postgres, and a first run that needs only an `.env` file.

## Features

### Instances

An **instance** is a user's configured copy of a server: a slug, their values, and a list of disabled tools. Each one gets its own URL at `/mcp/<slug>`. Slugs are unique per user, so several users can each have `/mcp/taiga`, and one user can have two instances of the same server under different slugs, for example for work and personal accounts.

From the instance page you can start, stop, and restart it, see its status, and turn individual tools off, so an agent never sees them.

### Catalog

The catalog is a TOML file listing the servers your users may run, and which variables each one accepts. It supports:

- **Four runtimes:** Node, Bun, uv for Python, and native binaries
- **Two modes:** `stdio` runs a process per session, and `http` runs one process per instance that holds no credentials
- **Typed variables:** strings, numbers, booleans and enums, each optionally required, secret, or in conflict with another
- **A lockfile** recording the whole dependency tree of each server, with hashes

See [Setting up the catalog](#setting-up-the-catalog).

### OAuth and connected apps

The hub is its own OAuth 2.1 authorization server, with discovery, dynamic client registration, and PKCE. MCP clients register themselves, and a user approves each client for one instance on a consent page.

Every approval is listed on the **Clients** page and can be revoked at any time. Revoking one stops that app's access to that instance, and leaves the others alone.

### Credential storage

Each user has a random data encryption key, and all of their values are encrypted with it. That key is itself only stored wrapped under keys derived from the password, a web session, or an OAuth token. A database dump alone doesn't reveal any values.

There's a cost to this: an operator resetting a forgotten password also wipes that user's stored values, because nothing else can unlock them.

### Web interface

A single-page app served by the hub itself, with no separate frontend container:

- Instances list, new-instance form, and an instance page with setup steps for each client
- Environment variables grouped into required and optional, with write-only secrets
- Tool filter and connected apps
- Settings for password, theme (dark or light), and language (English or Russian)

## Installation

### Requirements

- Docker and Docker Compose (v2.20 or newer)
- A domain and reverse proxy (nginx, Caddy, etc.) for anything beyond trying it locally. See [Self-hosting on a VPS](#self-hosting-on-a-vps)

### Quick start

```sh
git clone https://github.com/duckysmacky/mcp-hub.git
cd mcp-hub
cp .env.example .env
```

Edit `.env`:

- Set `DB_PASSWORD` to a real password
- Set `PUBLIC_URL` to the address you'll open the hub at. To try it locally, use `http://localhost:8080`

`PUBLIC_URL` has to match exactly, since signing in is refused from any other address.

```sh
docker compose up -d
```

The hub is closed by default, so create an invite code for your own account:

```sh
docker compose exec app mcp-hub invite create
```

Open `PUBLIC_URL` and sign up with that code.

The hub has no servers to offer yet. To start with the included examples (Taiga and Firefly III):

```sh
cp config/catalog.example.toml config/catalog.toml
cp config/catalog.example.lock config/catalog.lock
docker compose restart app
```

To add your own, see [Setting up the catalog](#setting-up-the-catalog).

### Configuration

Configuration lives in three places:

| Where | What |
|---|---|
| `.env` | The public URL, database connection and password, ports, image tag, and log level |
| `config/settings.toml` | Optional tuning: sign-up mode, limits, timeouts, network rules, and rate limits |
| `config/catalog.toml` and `config/catalog.lock` | Allowed servers and their frozen dependencies |

Your files in `config/` are ignored by git, so `git pull` won't touch them.

**`.env` variables** (see `.env.example`):

| Variable | Default | Description |
|---|---|---|
| `PUBLIC_URL` | none | Public origin the hub is served at, with no path or trailing slash. Must be `https`, except on a loopback host. Used as the OAuth issuer and the base of every endpoint URL |
| `COMPOSE_PROFILES` | `database` | Starts the bundled Postgres. Leave empty for an [external one](#using-an-external-postgres) |
| `DB_HOST` | `database` | Postgres host: `database` for the bundled container, or an external host |
| `DB_PORT` | `5432` | Postgres port. The bundled container listens on and publishes this same port |
| `DB_NAME` | `mcp_hub` | Postgres database name |
| `DB_USER` | `mcp_hub` | Postgres user |
| `DB_PASSWORD` | none | Postgres password. Change this |
| `DB_SSLMODE` | `disable` | `disable` for the bundled container, `require` or `verify-full` for a server over the internet |
| `DATABASE_URL` | none | Only for building and testing with cargo. Keep it in sync with the `DB_*` variables |
| `HUB_PORT` | `8080` | Host port the hub is published on, bound to `127.0.0.1` |
| `MCP_HUB_VERSION` | `latest` | Image tag to pull. Pin to a release (for example `1.0.0`) for a stable deploy |
| `RUST_LOG` | `info` | Log level |

**`settings.toml`** is only needed to change a default. Copy the example, which lists every key:

```sh
cp config/settings.example.toml config/settings.toml
```

Unknown keys are rejected, and changes apply on restart. The ones you're most likely to want:

| Key | Default | Description |
|---|---|---|
| `server.signup` | `invite-only` | `invite-only`, `enabled`, or `disabled` |
| `catalog.require_lock` | `true` | Refuse to start without a matching lockfile |
| `catalog.allow_custom_servers` | `false` | Let users define their own servers. Read [Custom servers](#custom-servers) first |
| `limits.max_processes_per_user` | `6` | Running processes per user |
| `limits.max_processes_total` | `50` | Running processes across the hub |
| `limits.max_instances_per_user` | `20` | Instances per user |
| `limits.idle_timeout_secs` | `600` | Stop a session's process after this long without requests |
| `network.trusted_proxies` | `[]` | Addresses `X-Forwarded-For` is believed from. Needed behind a proxy |
| `network.self_addresses` | `[]` | The host's public IPs. URLs resolving to them are allowed on ports 443 and 80 only |
| `network.allow_http_urls` | `false` | Allow plain `http` in URL values users enter |
| `audit.retention_days` | `90` | How long audit events are kept |

### Self-hosting on a VPS

The hub's port is bound to `127.0.0.1`, so it's meant to sit behind a reverse proxy. An nginx example:

```nginx
server {
    server_name mcp-hub.example.com;
    listen 443 ssl;

    http2 on;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;

        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto https;

        # MCP responses stream, and sessions stay open
        proxy_buffering off;
        proxy_cache off;
        proxy_read_timeout 1h;
        proxy_send_timeout 1h;
    }
}
```

Then tell the hub to trust that proxy, in `config/settings.toml`. A proxy on the host reaches the container through Docker's network:

```toml
[network]
trusted_proxies = ["172.16.0.0/12"]
```

Without this, every client shares one rate limit.

A few more things to get right:

- **HTTPS is required.** `PUBLIC_URL` must be `https` outside loopback, and session cookies are marked `Secure`. Get a certificate with [certbot](https://certbot.eff.org/) or your proxy's built-in ACME support.
- **`PUBLIC_URL` must be exactly the URL clients use**, or OAuth audience checks fail.
- **Claude Desktop and claude.ai connect from Anthropic's cloud**, so the hub must be reachable over public HTTPS for them.
- **Services on the same host** should be entered by their public HTTPS URL. List the host's public IP in `network.self_addresses`, so they're allowed. Loopback, private, and Docker-internal addresses are always rejected in URL values, so a server can't be pointed at a service only reachable on your LAN.

### Updating

```sh
docker compose pull
docker compose up -d
```

Database migrations run automatically at startup and can't be reversed, so pin `MCP_HUB_VERSION` in `.env` to a release if you'd rather update deliberately than track `latest`.

### Using an external Postgres

Create the role and database on your server, then change these in `.env`:

```sh
COMPOSE_PROFILES=
DB_HOST=db.example.com
DB_PORT=5432
DB_NAME=mcp_hub
DB_USER=mcp_hub
DB_PASSWORD=your-password
DB_SSLMODE=require
```

```sh
docker compose up -d
```

The hub creates its tables itself, and retries for up to a minute if the database isn't reachable yet.

`localhost` means the container, not the host, so it won't reach a Postgres running on the host. Give the role rights on its own database only (see [Security](#security)).

### Backups

Postgres is the only thing to back up, along with your `config/` directory. Installed packages can be rebuilt from the lockfile.

```sh
docker compose exec database pg_dump -U mcp_hub mcp_hub > backup.sql
```

The dump doesn't reveal stored values, but it does contain usernames, slugs, and client metadata.

### Operator commands

There's no admin web interface. Operator tasks run through the binary:

```sh
mcp-hub user list
mcp-hub user disable <username>
mcp-hub user enable <username>
mcp-hub user reset-password <username>   # wipes that user's stored values
mcp-hub invite create --days 7 --note "for Sam"
mcp-hub invite list
mcp-hub invite revoke <id>
mcp-hub catalog reinstall <server-id>
```

## Setting up the catalog

The catalog is `config/catalog.toml`. `config/catalog.example.toml` is a full, working example.

### Adding a server

```toml
[[server]]
id       = "taiga-stdio"
enabled  = true
mode     = "stdio"
runtime  = "bun"
package  = "@madebyclowd/taiga-mcp-server@1.0.2"
bin      = "taiga-mcp-server"
bin_args = []

[[server.env]]
name        = "TAIGA_BASE_URL"
description = "The base URL of the Taiga instance"
type        = "string"
secret      = false
required    = true
```

| Field | Required | Description |
|---|---|---|
| `id` | yes | Stable identifier and display name. Lowercase letters, digits and hyphens. Renaming it orphans existing instances |
| `enabled` | no | Set to `false` to stop offering a server, without removing it |
| `mode` | yes | `stdio` or `http`. See [HTTP mode](#http-mode) |
| `runtime` | yes | `node`, `bun`, `uv`, or `binary` |
| `package` | yes | `name@exact-version`. Ranges and tags are refused. For `binary`, a path or a name on `PATH` |
| `bin` | no | Which entry point to run. Required for `uv`, and for npm packages with several. Not allowed for `binary` |
| `bin_args` | no | Fixed arguments |

After any change, regenerate the lockfile and restart:

```sh
docker compose run --rm --no-deps -T app catalog lock --output - > config/catalog.lock.new && mv config/catalog.lock.new config/catalog.lock
docker compose restart app
```

The hub refuses to start if a catalog rule fails, and names the server and variable at fault. If you remove a server, its instances are disabled and flagged for their owners.

When choosing what to add, read the server's README for every variable it reads, then test it with a real call. The catalog is your responsibility: only add packages you trust.

### Runtimes

The image includes all of them, at pinned versions:

| Runtime | For | Installed with | Notes |
|---|---|---|---|
| `node` | npm packages | Bun, with install scripts off | Most npm servers declare Node in `engines`, so this is the safe choice |
| `bun` | npm packages | Bun, with install scripts off | Starts faster, but test each package under it |
| `uv` | PyPI packages | uv, wheels only, with hashes required | Packages without a wheel are refused |
| `binary` | Anything compiled | Not installed | See [Native binaries](#native-binaries) |

### Environment variables

Each `[[server.env]]` declares one variable a user may set. A process only ever gets the variables its entry declares.

| Field | Default | Description |
|---|---|---|
| `name` | required | The variable's name. Names like `PATH` or `LD_PRELOAD` are refused |
| `description` | empty | Shown in the interface |
| `type` | `string` | `string`, `number`, `boolean`, or `enum`. Checked on save |
| `enum_values` | empty | The choices, only with `type = "enum"` |
| `secret` | `true` | Secrets can be set but never read back. All values are encrypted either way |
| `required` | `false` | Must be set before the instance can start |
| `conflicts` | empty | Variables that can't be set together with this one. Must be listed on both sides |

Any value that parses as a URL is checked when it's saved, and again when a process starts. See `network` in [Configuration](#configuration).

### The lockfile

`config/catalog.lock` records the whole dependency tree of every enabled server, so what runs can't change without you noticing. Commit it next to your catalog, if you keep them in a repository.

The hub won't start if the lock is missing or doesn't match an enabled server. Upgrading means editing the version, regenerating the lock, and restarting.

To skip this, set `catalog.require_lock = false`. Anything the lock doesn't cover is then resolved at startup, through the same frozen install. Top-level versions stay pinned and install scripts stay off, but dependencies can change between restarts.

### HTTP mode

By default, `stdio` entries run one process per session, with the user's credentials in its environment. If a package's HTTP transport reads credentials from each request, prefer `http` mode:

```toml
[[server]]
id      = "taiga-http"
mode    = "http"
runtime = "bun"
package = "@madebyclowd/taiga-mcp-server@1.0.2"
bin     = "taiga-mcp-server-http"

[server.http]
port_env = "HTTP_PORT"
path     = "/mcp"
headers  = { Authorization = "Bearer {TAIGA_TOKEN}" }

[[server.env]]
name = "TAIGA_TOKEN"
```

The hub runs one process per instance, and adds the user's credentials as headers on each proxied request:

- Credentials never sit in a process environment, which closes the gap described in [Security](#security)
- One process serves all of an instance's sessions, instead of one each

`{VAR}` in `headers` is that user's value. Those variables are never put in the process's environment. `port_env` is the variable the hub uses to tell the server which loopback port to listen on.

### Native binaries

`runtime = "binary"` runs a ready-made executable, such as a server written in Rust or Go. Nothing is installed, and the lock pins its SHA-256.

Mount it into the container, by adding a line to the `app` service in `docker-compose.yml`:

```yaml
volumes:
  - ./bin:/opt/mcp:ro
```

```toml
[[server]]
id      = "my-server"
mode    = "stdio"
runtime = "binary"
package = "/opt/mcp/my-server"
```

The file must be executable, and must be mounted when you generate the lock. Replacing it means locking and restarting again.

The image is based on Debian with glibc 2.41. A static binary works as it is. If yours needs other shared libraries, build your own image with `FROM ghcr.io/duckysmacky/mcp-hub`.

### Custom servers

With `catalog.allow_custom_servers = true`, users can define their own servers on their instances: any runtime, package, and variables. Versions must still be exact and install scripts stay off, but there's no lock.

**Every account on such a hub can run any code as the hub's user, including reaching the database credentials and every user's stored values.** Every account is effectively equal to the operator. It's meant for single-user hubs and groups who would give each other shell access anyway. Keep `signup` on `invite-only` or `disabled`, and never turn this on for a hub with users you don't fully trust.

## Using the hub

### Creating an instance

1. Sign in and choose **New instance**
2. Pick a server and a slug
3. Fill in the variables it asks for. Secrets can't be read back later, only replaced
4. Start it, and copy the connection URL from the instance page

The instance page has a **Connect an app** section with these steps ready to copy, already filled in with your slug and URL.

### Connecting Claude

**Claude Code:**

```sh
claude mcp add --transport http <slug> https://mcp-hub.example.com/mcp/<slug>
```

Then run `/mcp` inside Claude Code, pick the server, and sign in.

**Claude Desktop and claude.ai:**

1. Open Customize, then Connectors (Settings, then Connectors in some versions)
2. Choose **Add custom connector** and paste the connection URL. Leave the OAuth fields empty, since Claude registers itself
3. Choose **Connect** and approve access on the hub's consent page

The same connector then works in the mobile apps. The hub must be reachable over public HTTPS for all of these.

### Connecting other agents

Any MCP client that speaks Streamable HTTP and OAuth works. Point it at your connection URL, and it handles discovery, registration, and sign-in by itself.

**Codex:**

```sh
codex mcp add <slug> --url https://mcp-hub.example.com/mcp/<slug>
codex mcp login <slug>
```

**OpenCode**, in `opencode.json`, then run `opencode mcp auth <slug>`:

```json
{
  "mcp": {
    "<slug>": { "type": "remote", "url": "https://mcp-hub.example.com/mcp/<slug>" }
  }
}
```

**Cursor**, in `~/.cursor/mcp.json`. Cursor opens a browser to sign in the first time it connects:

```json
{
  "mcpServers": {
    "<slug>": { "url": "https://mcp-hub.example.com/mcp/<slug>" }
  }
}
```

**VS Code**, in `.vscode/mcp.json`. Start the server from that file, and VS Code asks you to sign in:

```json
{
  "servers": {
    "<slug>": { "type": "http", "url": "https://mcp-hub.example.com/mcp/<slug>" }
  }
}
```

## Security

**Users trust the operator with their credentials.** Whoever runs the hub can read values from running processes. That can't be fixed by design, so only use a hub whose operator you trust.

**Don't offer `stdio` servers to people you don't trust.** Every process runs under the same user, and processes under the same user can read each other's environment. A malicious or exploited server could read another user's credentials, and the hub's database password. Until per-process sandboxing arrives:

- Keep `signup` on `invite-only` or `disabled` on the internet
- Prefer `http` mode wherever a package supports it
- Only add packages you've vetted
- Give an external database's role rights on its own database only

What the hub does do:

- Starts each process with a cleared environment, holding only the variables its catalog entry declares, and never builds a shell string
- Checks ownership against the authenticated user on every MCP request, and answers the same way for a slug that doesn't exist and one that belongs to someone else
- Never returns a secret through any API, and never logs one
- Never forwards its own access tokens to an upstream server or third-party API
- Stores tokens only as SHA-256 hashes
- Runs in a container with a read-only root, no capabilities, and `no-new-privileges`
- Limits processes, sessions, spawn rate, request sizes, and login attempts

Two known limits:

- URL checks are validation, not enforcement. A process makes its own connections, so DNS rebinding or a redirect can get past a check made ahead of time
- There are no memory or CPU limits per process, only caps on how many run

## Development

Requires Rust (stable), Bun, and Docker. Node and uv are only needed for catalog servers using them.

```sh
cp .env.example .env                  # one .env serves both Docker and cargo
docker compose up -d --wait database  # just the database

cd backend && cargo dev               # serves on :8080
cd frontend && bun install && bun run dev   # proxies the API to :8080
```

`cargo dev` uses `config/settings.dev.toml`. `backend/.cargo/config.toml` overrides what differs from `.env`, such as `DB_HOST`, so you never need to edit `.env` to switch between them. To serve the built interface from the backend instead, run `bun run build` and open `http://localhost:8080`.

```sh
cd backend
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test                            # needs the database and DATABASE_URL

cd frontend
bun run check
bun run lint
bun run test
```

After changing `config/catalog.example.toml`, regenerate its lock:

```sh
cd backend && cargo run -- --settings ../config/settings.dev.toml catalog lock
```

To build the image from source, run `docker compose up -d --build`.

Pull requests run all of these checks. A push to `master` publishes `ghcr.io/duckysmacky/mcp-hub:latest`, and a `v*` tag publishes that version and drafts a GitHub release.

See `AGENTS.md` for conventions, invariants, and repo layout.

# MCP Hub - Agent Guide

MCP Hub is a self-hosted, multi-user gateway that turns **custom (external, not native) MCP servers into per-user remote MCP endpoints**.

A user signs in, picks an MCP server from the operator's **catalog** (for example Taiga or Firefly III), fills in **their own** credentials, and gets an endpoint at:

```
https://<public_url>/mcp/<slug>
```

Agents (Claude Desktop, Claude Code, claude.ai, the mobile apps) connect to that URL over **Streamable HTTP** and authenticate with **OAuth 2.1**. When a request arrives, the hub resolves `(user from token, slug from path)` to that user's instance and spawns (or reuses) a child MCP process with that user's decrypted env. Users never see each other's processes or credentials.

The reference deployment is `mcp.nikdor.xyz`. The project is built to be self-hostable by others.

## Repository layout

```
.
├── AGENTS.md            # this file (CLAUDE.md is a symlink to it)
├── .context/            # design docs - the source of truth for how things should work
├── backend/             # Rust (Axum + Tokio + sqlx/SQLite) - the whole server
│   └── AGENTS.md
├── frontend/            # SvelteKit SPA (TypeScript, Svelte 5), built statically, served by the backend
│   └── AGENTS.md
├── catalog.example.toml # example operator catalog
├── Dockerfile
└── docker-compose.yml
```

## Non-negotiable invariants

Never break these. Details and reasoning are in `.context/SECURITY.md`.

1. **Users never supply commands, arguments, or packages.** Only catalog entries run, and only from packages pre-installed at pinned versions. No `npx`/`bunx` when a process starts.
2. **A child process gets only the env keys its catalog entry allows**, after `env_clear()`. The gateway sets `PATH`/`HOME` itself. Never build a shell string; always use argv.
3. **Every MCP request is resolved through the token's user.** A session id, slug, or instance id from the request is never enough on its own: ownership is checked against the authenticated user on every request.
4. **Secrets are write-only.** No API returns a secret value. Secrets are never logged, never put in error messages, and never sent to the frontend.
5. **No token passthrough.** The hub's access tokens never reach an upstream MCP server or third-party API.
6. **Tokens are opaque and stored only as SHA-256 hashes.** A token is also key material (it unwraps the user's DEK), so never log it or keep it in plaintext.
7. **URL-type env values go through `net_guard`** (resolve the host, reject loopback/private/link-local addresses). Never just compare against the string "localhost".
8. **The gateway never trusts a request because it comes from localhost** (child processes run on the same host).

## Commands

```bash
# backend
cd backend
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo run -- --config ../config.dev.toml

# frontend
cd frontend
bun install
bun run check        # svelte-check + tsc
bun run lint
bun run dev          # Vite dev server proxies /api, /oauth, /mcp, /.well-known to the backend
bun run build        # static output consumed by the backend / Docker image

# full stack
docker compose up --build
```

Before calling a task done, run fmt, clippy, and tests for every domain you touched.

## Working conventions

- **Small, focused commits** with clear messages. Prefer a sequence of incremental commits over one large one. Backend features usually land before their frontend.
- **Descriptive names.** No single-letter or cryptic identifiers except genuine loop counters. `snake_case` in Rust; idiomatic `camelCase` in TypeScript.
- **Don't add dependencies casually.** If you add a crate or npm package, say why in the commit message. Security-relevant crates (crypto, auth, process) need an explicit reason.
- **Tests come with behaviour.** Bridge, OAuth, crypto, and net_guard changes are never merged without tests.
- **Ask before:** changing the DB schema in a way that loses data, changing the token or encryption format, loosening any invariant above, or adding a new runtime to the catalog.

## Glossary

| Term | Meaning |
|---|---|
| **Catalog** | Operator-defined TOML listing allowed MCP servers, their pinned package, runtime, mode, and allowed env keys |
| **Template / catalog entry** | One `[[server]]` in the catalog |
| **Auth mode** | A named group of env keys inside a template (for example "token" or "username + password"); the user picks one |
| **Instance** | A user's configured copy of a template: slug, auth mode, encrypted env values, enabled flag |
| **Slug** | URL segment of an instance. Unique **per user**, not globally; several users may each have `/mcp/taiga` |
| **Session** | One MCP session (`Mcp-Session-Id`); in v1 each session owns one child process |
| **Grant** | A user's OAuth approval for one client and one instance; shown in the UI and revocable |
| **DEK** | A user's random data encryption key; encrypts all of that user's env values |
| **Wrap** | The DEK encrypted under a key derived from the password, a web-session token, an auth code, or an OAuth token |
| **Mode `stdio`** | The hub spawns a child per session and bridges stdin/stdout to Streamable HTTP |
| **Mode `shared_http`** | One shared upstream HTTP MCP process; the hub reverse-proxies and injects per-user headers |

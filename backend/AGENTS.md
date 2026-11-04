# Backend - Agent Guide

A single Rust binary (`mcp-hub`) that is everything server-side: web UI API, OAuth authorization server, MCP resource server, process supervisor, and static file host for the built frontend.

Read the root `AGENTS.md` first. This file only adds backend-specific rules.

## Stack

| Concern | Crate |
|---|---|
| HTTP | `axum`, `tower`, `tower-http` (trace, compression, static files, limits) |
| Async | `tokio` (full), `tokio-util` (`LinesCodec`, `CancellationToken`), `futures` |
| Database | `sqlx` with SQLite, migrations in `migrations/`, compile-time checked queries (`sqlx::query!`) with offline data in `.sqlx/` |
| Serialization | `serde`, `serde_json`, `toml` |
| Errors | `thiserror` inside modules; `anyhow` only in `main.rs` and startup code |
| Logging | `tracing`, `tracing-subscriber` (JSON in production, pretty in dev) |
| Passwords / KDF | `argon2` (Argon2id) |
| Encryption | `chacha20poly1305` (`XChaCha20Poly1305`), `hkdf`, `sha2`, `rand`, `zeroize` |
| Misc | `base64` (URL-safe, no padding), `time`, `url`, `hickory-resolver` (for `net_guard`) |

Edition 2024, stable toolchain. No `unsafe` outside the small `pre_exec` block in `process/` (it must carry a `// SAFETY:` comment).

## Module layout

```
backend/src/
├── main.rs            # CLI args, config load, startup, graceful shutdown
├── config.rs          # Config struct (TOML + env overrides), validation
├── app_state.rs       # AppState: db pool, catalog, session manager, config
├── error.rs           # ApiError / GatewayError -> HTTP responses (no secret leakage)
├── db/                # pool setup, typed repository functions per table
├── catalog/           # catalog TOML parsing, validation, package installation
├── crypto/            # DEK generation, wrap/unwrap, env value encrypt/decrypt, token hashing
├── auth/              # users, password login, web sessions (cookie), invites, extractors
├── oauth/             # authorization server: metadata, register (DCR), authorize, token, revoke
├── mcp/               # /mcp/{slug} handlers (POST/GET/DELETE), protected resource metadata
├── bridge/            # stdio <-> JSON-RPC session (BridgeSession), shared_http proxy
├── supervisor/        # session registry, limits, idle reaper, restart/stop, stderr ring buffers
├── process/           # building the child Command: env_clear, allowed env, cwd, process group, pdeathsig
├── net_guard/         # URL/host validation against loopback/private/link-local ranges
├── api/               # JSON API for the frontend (/api/*), DTOs
└── web/               # serving the built SPA with index.html fallback
```

Keep handlers thin: parse, authorize, call a module function, map errors. Business logic lives in the modules, not in the handlers.

## HTTP surface (summary)

The full design is in `.context/ARCHITECTURE.md`, `.context/OAUTH.md`, and `.context/BRIDGE.md`.

```
GET    /healthz
GET    /.well-known/oauth-authorization-server
GET    /.well-known/oauth-protected-resource/mcp/{slug}
GET    /oauth/authorize            POST /oauth/token   POST /oauth/register   POST /oauth/revoke
POST   /mcp/{slug}                 GET  /mcp/{slug}    DELETE /mcp/{slug}
/api/auth/*  /api/catalog  /api/instances/*  /api/grants/*  /api/oauth/pending/*  /api/admin/*
/*                                  # SPA static files, index.html fallback
```

## Rules

### Errors and responses
- No `unwrap()` or `expect()` in non-test code, except for true invariants, where `expect("why this cannot fail")` is fine.
- Error types must never carry secret values, tokens, or decrypted env in their `Display`/`Debug`. Use `#[error("decryption failed for env key {env_key}")]`, never one that includes the value.
- `/mcp/{slug}` returns **404 both for "slug doesn't exist" and "slug belongs to someone else"**. They must look identical.
- JSON-RPC level errors (inside a 200 response) are for MCP semantics; HTTP status codes are for transport and auth (401/403/404/400/202). See `.context/BRIDGE.md`.

### Async
- Never block the runtime. Argon2 hashing and key derivation go through `tokio::task::spawn_blocking`.
- Every spawned task needs a clear owner and an end condition (EOF, a `CancellationToken`, or a dropped channel). No orphan tasks.
- Writes to a child's stdin go only through that session's single writer task.

### Database
- All schema changes are new numbered files in `migrations/`. Never edit a migration that has shipped.
- After changing queries, run `cargo sqlx prepare` so `.sqlx/` stays in sync (CI builds offline).
- Store timestamps as UTC unix seconds (`INTEGER`).
- Secret material (ciphertexts, wraps, token hashes) is `BLOB`, never base64 text.

### Crypto (`crypto/`)
- Only `crypto/` touches AEAD, KDF, and raw key bytes. Other modules call its functions.
- Keys live in `Zeroizing<[u8; 32]>` (or a newtype that zeroizes on drop) and are never `Clone`d casually.
- Every AEAD call has associated data that binds the ciphertext to its row (see `.context/SECRETS.md`). Never encrypt without AAD.
- Versioned info strings for HKDF (`"mcp-hub/dek-wrap/v1/<kind>"`). Changing a format means a new version string plus a migration path, and it needs approval.

### Processes (`process/`, `bridge/`, `supervisor/`)
- Build every child `Command` through `process::build_child_command`. Nothing else may call `Command::new` for MCP children.
- Required on every child: `env_clear()`, only the declared env keys, a gateway-set `PATH` and `HOME`, `current_dir` set to an empty per-session scratch directory, `kill_on_drop(true)`, its own process group, `PR_SET_PDEATHSIG(SIGKILL)`, stdin/stdout/stderr piped.
- Always drain stderr into a bounded ring buffer, otherwise the child blocks when the pipe fills. Before storing, redact exact occurrences of that instance's secret values.
- Always read stdout with a max line length (`LinesCodec::new_with_max_length`).

### Logging
- Never log: tokens, cookies, `Authorization` headers, env values, request/response bodies of `/mcp/*`, or tool arguments. Log method names, durations, user/instance ids, status codes.
- Use `tracing` spans with `user_id`, `instance_id`, and `session_id` fields (the session id is fine to log; tokens are not).

## Testing

- **Unit tests** next to the code (`#[cfg(test)]`). Required for `crypto`, `net_guard`, `catalog` validation, and env key validation.
- **Integration tests** in `tests/` start the real router against a temporary SQLite database.
- **Fake MCP server.** `tests/fixtures/fake_mcp/` is a small Rust binary speaking stdio MCP (initialize, tools/list, tools/call echo, a deliberate crash, a slow call, an over-long line, stderr spam). Bridge tests use it, so the suite never needs Bun, Node, or the network.
- OAuth tests cover the full flow: register, authorize, consent, token, refresh rotation, refresh reuse detection, audience mismatch, and PKCE failure.

## Security checklist for every backend change

- [ ] Does any new code path accept an id (session, instance, grant, client) from a request? Is ownership checked against the authenticated user?
- [ ] Could a secret, token, or decrypted value end up in a log, error, panic message, or API response?
- [ ] Does any new child process go through `process::build_child_command`?
- [ ] Does any new URL or host coming from a user go through `net_guard`?
- [ ] Are new limits (body size, line length, timeouts, counts) bounded?

# Frontend - Agent Guide

A deliberately simple SvelteKit SPA for managing accounts, MCP instances, secrets, and OAuth grants. The backend serves the static build; there is no Node/Bun server in production.

Read the root `AGENTS.md` first. This file only adds frontend-specific rules.

## Stack

- **SvelteKit** with `@sveltejs/adapter-static` (`fallback: 'index.html'`), SSR off (`export const ssr = false` in the root layout).
- **Svelte 5 runes** (`$state`, `$derived`, `$effect`, `$props`). No legacy `export let` or stores for new code.
- **TypeScript** in strict mode. No `any` without a comment explaining why.
- **Bun** as package manager and script runner.
- Styling: plain CSS with custom properties (design tokens in `src/app.css`), component-scoped `<style>`. No heavy UI framework. Light and dark themes via `prefers-color-scheme`.

## Layout

```
frontend/src/
├── app.css                  # tokens: colors, spacing, radius, typography
├── lib/
│   ├── api/
│   │   ├── client.ts        # fetch wrapper: credentials 'same-origin', JSON, error mapping
│   │   ├── types.ts         # DTO types mirroring backend api/ DTOs
│   │   └── *.ts             # one file per resource: auth, catalog, instances, grants, oauth
│   ├── components/          # reusable UI pieces (SecretField, EnvForm, StatusBadge, ...)
│   └── session.svelte.ts    # current user state (runes-based)
└── routes/
    ├── +layout.svelte / +layout.ts   # ssr = false, auth guard
    ├── login/  register/              # register needs an invite code unless open registration
    ├── instances/                     # list
    ├── instances/new/                 # pick template -> slug -> auth mode -> env
    ├── instances/[id]/                # env, start/stop/restart, status, tool filter, connection URL
    ├── grants/                        # connected clients per instance, revoke
    ├── consent/                       # OAuth consent page (?request=<pending id>)
    ├── settings/                      # change password
    └── admin/                         # invites, users (admins only)
```

## Rules

### Auth and data handling
- Auth uses an **HttpOnly, Secure, SameSite=Lax cookie** set by the backend. The frontend never sees or stores a token. No tokens in `localStorage`, `sessionStorage`, or JS-accessible cookies.
- State-changing requests send `Content-Type: application/json`; the backend also checks `Origin`. Don't add endpoints that change state on `GET`.
- Treat the backend as the only authority for permissions. Hiding a button is UX, not security.

### Secrets UI (write-only, like GitHub secrets)
- An env value with `kind = "secret"` is **never** displayed, prefilled, or sent back by the API. The UI shows "Set" plus the date it was set, with a **Replace** action and a **Remove** action.
- Secret inputs use `type="password"` and `autocomplete="new-password"`, and are cleared from component state immediately after a successful save.
- `plain` and `url` values may be shown and edited normally.
- Show which keys are required by the selected auth mode, and keep **Start** disabled until all of them are set (mirror the backend check; the backend still enforces it).

### Consent page (`/consent`)
It's the anti-phishing barrier for OAuth, so it must show clearly:
- the **client name** (as registered, marked as self-reported),
- the **redirect URI host**, prominently. When the redirect is `localhost`/`127.0.0.1`, show a warning that a local app on this machine will receive access,
- **which instance** (slug and template name) access is being granted to,
- Approve and Deny buttons of equal visual weight. Never auto-approve.

If the user isn't logged in, send them to `/login?next=/consent?request=...` and come back.

### Instance page
- Show the connection URL `<public_url>/mcp/<slug>` with a copy button, plus short "how to add this to Claude" instructions.
- Status comes from `/api/instances/{id}/status`: enabled/disabled, live session count, last error, recent stderr (already redacted by the backend). Poll gently (every 5 s while the page is visible), and don't poll in background tabs.
- Start / Stop / Restart map to enable / disable (kills sessions) / kill all sessions. Ask for confirmation before Stop and Restart, because they drop live agent connections.

### API types
- `lib/api/types.ts` mirrors the backend DTOs by hand. When a backend DTO changes, update this file in the same change.
- Map API errors to readable messages in one place (`client.ts`). Never show raw stack traces or JSON.

### Accessibility and layout
- Works at phone width (users will add connectors from their phones). No horizontal scroll, touch targets at least 44 px.
- Every form input has a label; errors are linked with `aria-describedby`.

## Commands

```bash
bun install
bun run dev        # Vite dev server; proxies /api, /oauth, /mcp, /.well-known to http://127.0.0.1:8080
bun run check      # svelte-check
bun run lint       # eslint + prettier --check
bun run format
bun run build      # outputs to build/, which the Docker image copies into the backend's static dir
```

# Frontend - Agent Guide

A deliberately simple SvelteKit SPA for managing accounts, MCP instances, secrets, and connected clients, and for OAuth consent. The backend serves the static build; there is no Node/Bun server in production.

Read the root `AGENTS.md` first. This file only adds frontend-specific rules.

## Stack

- **SvelteKit** with `@sveltejs/adapter-static` (`fallback: 'index.html'`), SSR off (`export const ssr = false` in the root layout).
- **Svelte 5 runes** (`$state`, `$derived`, `$effect`, `$props`). No legacy `export let` or stores for new code.
- **TypeScript** in strict mode. No `any` without a comment explaining why.
- **Bun** as package manager and script runner.
- **Styling is the Cobalt design system** (https://claude.ai/artifact/5mPPgiXkzBzGZLBbcAtCcu; its `project/README.md` and component READMEs are the reference). No heavy UI framework.

## Cobalt

- Compose pages from Cobalt's `cb-*` classes and tokens. Don't invent colors, sizes, or shadows; a component's scoped `<style>` only arranges things.
- `lib/styles/tokens.css` (generated from Cobalt's `tokens.json`) and `lib/styles/cobalt.css` (its `bundle.css`, verbatim) are vendored: re-sync them, don't edit them. Fixes and additions go in `lib/styles/app.css`.
- JetBrains Mono is self-hosted through Fontsource, so no page calls a third party.
- Dark and light themes: the OS decides by default, Settings overrides it, and `app.html` applies the choice before first paint. Theme and language are the only things in `localStorage`.
- Text first: bracketed status words, verb-first labels, sentence case. Colour only reinforces the word. Exactly one primary button per view.
- Motion: 120ms color changes and the spinner. Nothing else moves.
- Where Cobalt's examples and the backend disagree (slug length, password length), the backend's rules win.
- Cobalt also draws READ/DESTRUCTIVE tool tags. They arrive when the API has that data.

## Languages

- Every visible string lives in `lib/i18n/en.ts` and `ru.ts`, read as `i18n.t.section.key`. `ru: Messages` makes a missing key a type error. Messages with parameters are functions; Russian plurals use `plural()`.
- Operator catalog descriptions and server output stay as written.
- Backend errors have no codes: `client.ts` passes them through `apiErrors` in `ru.ts`, matched by pattern. When a backend message changes or is added, update that table.

## Layout

```
frontend/src/
├── app.html                 # cb-root on body, theme script
├── lib/
│   ├── api/
│   │   ├── client.ts        # fetch wrapper: credentials 'same-origin', JSON, error mapping
│   │   └── types.ts         # DTO types mirroring backend api/ DTOs
│   ├── styles/              # tokens.css and cobalt.css (vendored), app.css (ours)
│   ├── components/          # Button, SecretInput, EnvVars, EnvVarRow, ClientGuides, ConfirmDialog, ...
│   ├── clients.ts           # how to connect each MCP client to an instance
│   ├── navigation.ts        # safeNextPath: the open-redirect guard for ?next=
│   ├── dates.ts             # relative times
│   ├── i18n/                # en.ts, ru.ts, api-errors.ts, the reactive locale
│   ├── status.ts            # maps an instance's status to a word and a tone
│   ├── theme.ts             # stored theme choice
│   └── toasts.svelte.ts     # the toast stack
└── routes/
    ├── +layout.svelte / +layout.ts   # styles, ssr = false, auth guard
    ├── (app)/                        # signed-in pages, inside the nav
    │   ├── instances/                # list
    │   ├── instances/new/            # pick server or define a custom one -> slug
    │   ├── instances/[id]/           # env, start/stop/restart, status, tool filter, settings, connection URL
    │   ├── clients/                  # connected clients (grants), revoke
    │   └── settings/                 # change password, theme, language
    └── (card)/                       # centred card pages, no nav
        ├── login/  signup/           # sign-up follows GET /api/auth/config: enabled, disabled, invite-only
        └── consent/                  # OAuth consent page (?request=<pending id>)
```

Pages call `api` from `client.ts` directly, and the current user comes from the root layout's `load`, so there are no per-resource wrappers and no session store. `resolve()` takes route IDs, so they include the group: `resolve('/(app)/instances')`.

## Rules

### Auth and data handling

- Auth uses an **HttpOnly, Secure, SameSite=Lax cookie** set by the backend. The frontend never sees or stores a token. No tokens in `localStorage`, `sessionStorage`, or JS-accessible cookies.
- State-changing requests send `Content-Type: application/json`; the backend also checks `Origin`. Don't add endpoints that change state on `GET`.
- Treat the backend as the only authority for permissions. Hiding a button is UX, not security.

### Secrets UI (write-only, like GitHub secrets)

- A **secret** env value is write-only: never displayed, prefilled, or sent back by the API. The UI shows `SET` plus the date it was set, with a **Replace** action and a **Remove** action. Non-secret values come back as `value` and can be read and edited; they are encrypted at rest all the same.
- Values are set in place, in the variable's own row, with an input by type. Required variables come first; optional ones follow in a collapsible section. Secrets carry a lock tag. Secret inputs use `type="password"` and `autocomplete="new-password"` and are cleared from component state immediately after a successful save.
- A required variable is tinted only when it's in `missing_required`, which already treats conflicting variables as alternatives.
- Vars the catalog no longer lists (`in_catalog = false`) are shown as "no longer used", with Remove only.

### Consent page (`/consent`)

It's the anti-phishing barrier for OAuth, so it must show clearly:

- the **client name** (as registered, marked as self-reported),
- the **redirect URI host**, prominently. When the redirect is loopback, show a warning that a local app on this machine will receive access,
- **which instance** (slug and catalog server id) access is being granted to,
- Approve and Deny buttons of equal visual weight, neither primary, neither pre-focused. Never auto-approve.

If the user isn't logged in, send them to `/login?next=/consent?request=...` and come back.

### Instance page

- Always show the connection URL `<public_url>/mcp/<slug>` with a copy button. How to add it to each client (`lib/clients.ts`) sits in a collapsed section, with one expandable entry per client, so it doesn't clutter the page. Check a client's own docs before adding or changing its guide.
- Status comes from `/api/instances/{id}/status`: enabled/disabled, live process count, last exit, recent stderr (already redacted by the backend). Poll gently (every 5 s while the page is visible), and don't poll in background tabs. Unsaved form edits must survive a poll.
- Start / Stop / Restart map to enable / disable (kills sessions) / kill all sessions. Ask for confirmation before Stop and Restart, because they drop live agent connections. The same goes for renaming, which stops running processes too.
- A custom instance also has an editable definition panel (`CustomServerPanel`). Saving asks for confirmation, because it stops running processes.
- The tool filter saves on click, needs no restart, and is empty until a connected client has listed tools.

### Connected clients

- Names and URLs are self-reported, so they're tagged and never linked. Never load `logo_uri`: it would call a third party and lend the client an unverified logo.
- Revoking stops the instance's running processes. Say so in the confirmation.

### API types

- `lib/api/types.ts` mirrors the backend DTOs by hand. When a backend DTO changes, update this file in the same change.
- Map API errors to readable messages in one place (`client.ts`). Never show raw stack traces or JSON.

### Accessibility and layout

- Works at phone width (users will add connectors from their phones). No horizontal scroll, touch targets at least 44 px.
- Every form input has a label; errors are linked with `aria-describedby`.

## Commands

```bash
bun install
bun run dev        # Vite dev server; proxies /api, /oauth, /mcp, /.well-known to http://127.0.0.1:8080,
                   # rewriting Origin to http://localhost:8080 so the backend's CSRF check accepts it
bun run check      # svelte-check
bun run lint       # eslint + prettier --check
bun run format
bun run test       # vitest
bun run build      # outputs to build/, which the backend serves from `paths.static`
```

To test the real thing, build, run the backend, and open `PUBLIC_URL` exactly (`http://localhost:8080` in dev, not `127.0.0.1`), or the `Origin` check rejects every change. Passwords take several seconds in a debug backend build, because Argon2 is slow without optimizations. Without a migrated dev database, build the backend with `SQLX_OFFLINE=true`.

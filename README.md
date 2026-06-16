# Dronte

Fair-source, self-hostable **in-app notification inbox infrastructure**.
One Rust binary + Postgres + Redis, a deliberately small HTTP API, and a
drop-in `<Inbox />` React component. No workflow engine — the inbox is the
primitive.

> The plan lives in `docs/dronte-project-plan.md`. The frozen v1 contracts and
> the per-phase specs are archived in `project/archive-v1/` (the generated
> OpenAPI spec is the published truth since the v1 flip).

## Repository layout

```
server/            Rust binary: API, SSE, workers          (FSL-1.1-MIT)
packages/client/   @dronte/client — headless TS core       (MIT)
packages/react/    @dronte/react  — hooks + <Inbox />      (MIT)
examples/          quickstarts and integration examples    (MIT)
docs/              Fumadocs site + project plan            (MIT)
project/           archived v1 contracts + OpenAPI baseline
```

## Admin dashboard

The server embeds an operator dashboard at `/admin` (status/timeline browser,
broadcast composer, subscriber lookup, DLQ replay, environment + API key
management, HMAC rotation, and admin-user management). It ships inside the
binary — `docker run dronte` serves it with no extra artifact.

**Built-in users with roles.** The dashboard has its own login (email +
password, Argon2id-hashed) backed by a server-side session cookie. Four fixed
roles gate what each operator can do (still single-org — no organizations, no
per-environment user scoping):

- **viewer** — read-only (inbox, timelines, subscriber lookup, DLQ list).
- **operator** — viewer, plus DLQ replay and composing broadcasts.
- **developer** — viewer, plus create/revoke API keys and read an
  environment's subscriber HMAC secret.
- **admin** — everything, including creating environments, rotating HMAC
  secrets, and managing users.

**Bootstrap (root) admin.** On boot, if `DRONTE_ADMIN_EMAIL` and
`DRONTE_ADMIN_PASSWORD` are set, Dronte ensures an `admin` account with that
email — creating it, or resetting its password to the env value if it drifted.
This is the lockout-recovery path: restart with the env vars to restore admin
access. Everyone else gets their own account from the Users page.

```bash
DRONTE_ADMIN_EMAIL=ops@example.com \
DRONTE_ADMIN_PASSWORD="$(openssl rand -hex 24)" \
DRONTE_ADMIN_TLS_TERMINATED=true \
  dronte serve
```

**TLS is required.** The session cookie is `HttpOnly; SameSite=Strict;
Path=/admin`, marked `Secure` only when `DRONTE_ADMIN_TLS_TERMINATED=true`.
The binary serves plain HTTP, so terminate TLS at a proxy and set that flag;
without it Dronte logs a boot-time warning and the cookie omits `Secure`.
Passwords and session ids are never logged.

## License FAQ

**What is licensed how?** The server (`server/`) is
[FSL-1.1-MIT](./LICENSE) — the Functional Source License. The SDKs
(`packages/client`, `packages/react`) and everything in `examples/` are
MIT — they embed in your frontend, and they carry their own `LICENSE`
files.

**What does FSL mean for me?** You can use, self-host, modify, and
redistribute Dronte freely — internally, in production, commercially, at
any scale, for free, forever. The single thing the license prohibits is
offering Dronte itself to others as a competing commercial product or
hosted service. Each release additionally converts to plain MIT two years
after it ships.

**Does the server license affect my application?** No. You run the Dronte
binary as a standalone network service and integrate over HTTP through the
MIT-licensed SDKs. Nothing about the server's license reaches your
codebase, and calling the HTTP API creates no obligations.

**Is this open source?** Not by the OSI definition — FSL is
[fair source](https://fair.io). The source is public, self-hosting is
unrestricted, and every release becomes MIT (true open source) on its
second anniversary.

**What about the API spec and docs?** The generated OpenAPI document and
the documentation content are MIT, so third-party clients, bindings, and
integrations are unambiguous.

**The name and logo?** Not licensed. "Dronte" and the logo remain with the
project regardless of code license — that, not the code license, is the
protection against confusing forks.

**Contributing:** external code contributions require a CLA (so the
licensing model above stays enforceable).

# Chimely

Open-source, self-hostable **in-app notification inbox infrastructure**.
One Rust binary + Postgres + Redis, a deliberately small HTTP API, and a
drop-in `<Inbox />` React component. No workflow engine — the inbox is the
primitive.

## Repository layout

```
server/            Rust binary: API, SSE, workers          (AGPL-3.0)
packages/client/   @chimely/client — headless TS core       (MIT)
packages/react/    @chimely/react  — hooks + <Inbox />      (MIT)
examples/          quickstarts and integration examples    (MIT)
docs/              Fumadocs site                           (MIT)
```

## Admin dashboard

The server embeds an operator dashboard at `/admin` (status/timeline browser,
broadcast composer, subscriber lookup, DLQ replay, environment + API key
management, HMAC rotation, and admin-user management). It ships inside the
binary — `docker run chimely` serves it with no extra artifact.

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

**Bootstrap (root) admin.** On boot, if `CHIMELY_ADMIN_EMAIL` and
`CHIMELY_ADMIN_PASSWORD` are set, Chimely ensures an `admin` account with that
email — creating it, or resetting its password to the env value if it drifted.
This is the lockout-recovery path: restart with the env vars to restore admin
access. Everyone else gets their own account from the Users page.

```bash
CHIMELY_ADMIN_EMAIL=ops@example.com \
CHIMELY_ADMIN_PASSWORD="$(openssl rand -hex 24)" \
CHIMELY_ADMIN_TLS_TERMINATED=true \
  chimely serve
```

**TLS is required.** The session cookie is `HttpOnly; SameSite=Strict;
Path=/admin`, marked `Secure` only when `CHIMELY_ADMIN_TLS_TERMINATED=true`.
The binary serves plain HTTP, so terminate TLS at a proxy and set that flag;
without it Chimely logs a boot-time warning and the cookie omits `Secure`.
Passwords and session ids are never logged.

## License FAQ

**What is licensed how?** The server (`server/`) is
[AGPL-3.0](./LICENSE) — the GNU Affero General Public License v3. The SDKs
(`packages/client`, `packages/react`) and everything in `examples/` are
MIT — they embed in your frontend, and they carry their own `LICENSE`
files.

**What does AGPL mean for me?** You can use, self-host, modify, and
redistribute Chimely freely — internally, in production, commercially, at
any scale, for free, forever. AGPL's one obligation is reciprocity: if you
modify Chimely and offer it to others over a network, you must give those
users that modified server's source (AGPL §13). Running it
unmodified carries no such obligation.

**Does the server license affect my application?** No. You run the Chimely
binary as a standalone network service and integrate over HTTP through the
MIT-licensed SDKs. Your application is a separate program that talks to
Chimely over the API — AGPL covers Chimely's own source, not the client
code that calls it, and calling the HTTP API creates no obligations.

**Is this open source?** Yes. AGPL-3.0 is an OSI-approved open source
license. It is copyleft: the freedom to use, study, modify, and share is
preserved for everyone you distribute or network-serve the server to.

**What about the API spec and docs?** The generated OpenAPI document and
the documentation content are MIT, so third-party clients, bindings, and
integrations are unambiguous.

**The name and logo?** Not licensed. "Chimely" and the logo remain with the
project regardless of code license — that, not the code license, is the
protection against confusing forks.

**Contributing:** external code contributions require a CLA (so the project
can continue to relicense or offer commercial licenses if needed).

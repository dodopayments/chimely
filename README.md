# Chimely

[![CI](https://github.com/dodopayments/chimely/actions/workflows/ci.yml/badge.svg)](https://github.com/dodopayments/chimely/actions/workflows/ci.yml)
[![@chimely/react](https://img.shields.io/npm/v/%40chimely%2Freact?label=%40chimely%2Freact)](https://www.npmjs.com/package/@chimely/react)
[![@chimely/client](https://img.shields.io/npm/v/%40chimely%2Fclient?label=%40chimely%2Fclient)](https://www.npmjs.com/package/@chimely/client)
[![License](https://img.shields.io/badge/license-AGPL--3.0%20server%20%7C%20MIT%20SDKs-blue)](https://github.com/dodopayments/chimely#license-faq)

Open-source, self-hostable **in-app notification inbox infrastructure**.
One Rust binary + Postgres + Redis, a deliberately small HTTP API, and a
drop-in `<Inbox />` React component. No workflow engine: the inbox is the
primitive.

Your backend sends one POST:

```bash
curl -X POST https://chimely.example.com/v1/notifications \
  -H 'Authorization: Bearer <api-key>' \
  -H 'Content-Type: application/json' \
  -d '{
    "subscriber_id": "usr_123",
    "category": "billing.invoice",
    "payload": { "title": "Invoice ready", "body": "March invoice is ready." }
  }'
```

Your frontend renders one component:

```tsx
import { Inbox } from '@chimely/react';

<Inbox
  serverUrl="https://chimely.example.com"
  environment="production"
  subscriberId="usr_123"
  subscriberHash={subscriberHash}
/>
```

That is a live bell with unread counts, read state, tabs, archive, infinite
scroll, per-category preferences, and SSE live updates. Nothing else to build.

## Quickstart

Nothing to clone: the server runs from the published image and the component
installs from npm.

```bash
docker network create chimely

docker run -d --name chimely-pg --network chimely \
  -e POSTGRES_PASSWORD=chimely postgres:16-alpine

docker run -d --name chimely --network chimely -p 8080:8080 \
  --restart unless-stopped \
  -e DATABASE_URL=postgres://postgres:chimely@chimely-pg:5432/postgres \
  -e CHIMELY_DEV_ENVIRONMENT=demo \
  -e CHIMELY_DEV_API_KEY=dev-secret-key \
  ghcr.io/dodopayments/chimely:0.2.1
```

The restart policy covers the first seconds while Postgres is still
initializing (the server fails fast when its database is unreachable). Then
follow the [quickstart](https://chimely.dev/docs/quickstart): send a
notification with curl and mount `<Inbox />` in your app, about five minutes
end to end.

## How it works

```
 your backend ── POST /v1/notifications ──▶ ┌─────────────┐ ◀──▶ Postgres  (source of truth)
                                            │   chimely   │
 your frontend ◀── SSE hints + REST/ETag ──▶│  one binary │ ◀──▶ Redis  (hints; optional)
    <Inbox />                               └─────────────┘
```

- **Postgres is authoritative.** Redis only carries real-time hints and
  recomputable caches; losing it delays updates but never loses data.
- **SSE is a hint, not a transport.** Every hint triggers a conditional
  refetch (ETag, mostly `304`s), so missed hints are harmless by
  construction.
- **Single-org, multi-environment.** Environments are the isolation unit;
  multi-tenancy is "run another instance".
- The server embeds its migrations (run on boot under an advisory lock), an
  operator dashboard, Prometheus metrics, and OpenAPI docs at `/docs`.

## What Chimely is not

These are deliberate. Chimely does the inbox and nothing else:

- No workflows, steps, or conditional routing.
- No email, SMS, or push channels. In-app inbox only.
- No server-side templating. Your payload is stored and shown as-is.
- No digests or batching.

## Repository layout

```
server/            Rust binary: API, SSE, workers           (AGPL-3.0)
packages/client/   @chimely/client, headless TS core        (MIT)
packages/react/    @chimely/react, hooks + <Inbox />        (MIT)
examples/          quickstarts and integration examples     (MIT)
docs/              Fumadocs site                            (MIT)
```

## Admin dashboard

The server embeds an operator dashboard at `/admin` (status/timeline browser,
broadcast composer, subscriber lookup, DLQ replay, environment + API key
management, HMAC rotation, and admin-user management). It ships inside the
binary: `docker run chimely` serves it with no extra artifact.

**Built-in users with roles.** The dashboard has its own login (email +
password, Argon2id-hashed) backed by a server-side session cookie. Four fixed
roles gate what each operator can do (still single-org, no organizations, no
per-environment user scoping):

- **viewer** reads everything (inbox, timelines, subscriber lookup, DLQ list).
- **operator** adds DLQ replay and composing broadcasts.
- **developer** adds create/revoke API keys and reading an environment's
  subscriber HMAC secret.
- **admin** adds creating environments, rotating HMAC secrets, and managing
  users.

**Bootstrap (root) admin.** On boot, if `CHIMELY_ADMIN_EMAIL` and
`CHIMELY_ADMIN_PASSWORD` are set, Chimely ensures an `admin` account with that
email exists, creating it or resetting its password to the env value if it
drifted. This is the lockout-recovery path: restart with the env vars to
restore admin access. Everyone else gets their own account from the Users
page.

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

## Versioning

Chimely is pre-1.0 (currently 0.2.x). The HTTP API and the SDK surface may
change on minor version bumps until 1.0.0. Pin your versions: the Docker tag
and the npm versions.

## Contributing

See [CONTRIBUTING.md](https://github.com/dodopayments/chimely/blob/main/CONTRIBUTING.md)
for the dev setup and workflow. External code contributions require a CLA so
the project keeps relicensing and commercial-licensing flexibility.

## License FAQ

**What is licensed how?** The server (`server/`) is
[AGPL-3.0](https://github.com/dodopayments/chimely/blob/main/LICENSE), the GNU
Affero General Public License v3. The SDKs (`packages/client`,
`packages/react`) and everything in `examples/` are MIT; they embed in your
frontend, and they carry their own `LICENSE` files.

**What does AGPL mean for me?** You can use, self-host, modify, and
redistribute Chimely freely: internally, in production, commercially, at
any scale, for free, forever. AGPL's one obligation is reciprocity: if you
modify Chimely and offer it to others over a network, you must give those
users that modified server's source (AGPL §13). Running it
unmodified carries no such obligation.

**Does the server license affect my application?** No. You run the Chimely
binary as a standalone network service and integrate over HTTP through the
MIT-licensed SDKs. Your application is a separate program that talks to
Chimely over the API. AGPL covers Chimely's own source, not the client
code that calls it, and calling the HTTP API creates no obligations.

**Is this open source?** Yes. AGPL-3.0 is an OSI-approved open source
license. It is copyleft: the freedom to use, study, modify, and share is
preserved for everyone you distribute or network-serve the server to.

**What about the API spec and docs?** The generated OpenAPI document and
the documentation content are MIT, so third-party clients, bindings, and
integrations are unambiguous.

**The name and logo?** Not licensed. "Chimely" and the logo remain with the
project regardless of code license; that, not the code license, is the
protection against confusing forks.

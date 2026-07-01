# Chimely

> Fair-source, self-hostable in-app notification inbox infrastructure. Drop in
> one React component, send with one API call from your backend. Open-source
> infrastructure you run yourself.

Chimely is a self-hostable **in-app notification inbox** you run yourself. It is
the inbox primitive, not a workflow engine: you send notifications, and a
drop-in `<Inbox />` renders them.

- **One POST to notify** — your backend calls `POST /v1/notifications`.
- **One `<Inbox />` to render** — drop the component into your frontend.

A live bell, unread counts, read state, and preferences, with nothing to build.

## How it works

You run one Rust binary backed by Postgres (the source of truth), plus Redis
for the real-time plane. Your backend sends notifications over a small HTTP API,
and the `<Inbox />` widget streams updates live over SSE and stays in sync.
Notifications update instantly without a page refresh.

It self-hosts on a single node and scales to many.

## What Chimely is not

These are deliberate. Chimely does the inbox and nothing else:

- No workflows, steps, or conditional routing.
- No email, SMS, or push channels. In-app inbox only.
- No server-side templating. Your payload is stored and shown as-is.
- No digests or batching.

## Architecture

- One Rust binary: HTTP API, SSE hints, and background workers.
- Postgres (>= 15) is the authoritative source of truth.
- Redis is the real-time hint/cache plane. Redis loss may delay hints but never
  loses data, since counters are recomputable from Postgres.
- SSE is a hint, not a transport. Clients refetch via conditional REST (ETag)
  on every hint, so missed hints are harmless by construction.

## Get started

- [Quickstart](/docs/quickstart): Run Chimely and render an inbox in about ten minutes.
- [Self-hosting](/docs/self-hosting): Compose, configuration, health, and backups.
- [Auth & HMAC](/docs/auth): API keys and the subscriber hash.
- [SDK reference](/docs/sdk-reference): Inbox props, hooks, and the client.
- [API reference](/docs/api): The full HTTP API.

## License

The server is AGPL-3.0 (OSI open source, copyleft). The SDKs (`@chimely/client`,
`@chimely/react`) and examples are MIT so they can embed in customer frontends.

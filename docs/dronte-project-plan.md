# Dronte — Project Plan

Fair-source, self-hostable **in-app notification inbox infrastructure**. A Rust server with Postgres as source of truth and Redis as the real-time plane, plus a drop-in `<Inbox />` React component and a deliberately small HTTP API.

The thesis: Novu models a workflow engine that happens to have an inbox channel. Dronte models an inbox that may later gain push transports. Same DX on the outside — `<Inbox />` component, one API call to notify — radically simpler on the inside: no workflow engine, no step model, no per-channel template system. Web-push and mobile-push come later as additional transports for the *same* notification object, not as workflow steps.

Positioning: "the inbox primitive." Novu self-host requires Mongo + multiple Node services and a workflow mental model; Dronte is one binary + Postgres + Redis and one POST request.

Deployment model: **single-org, multi-consumer, Plausible-style.** One Dronte instance serves one company — all of its consumer apps and all of their end users — with no organization layer in the schema or the product. Multi-tenancy is solved the way Plausible solves it: run another instance.

---

## Architecture overview

```
                      ┌──────────────────────────────────────────┐
customer backend ─────►  dronte (single binary)                  │
  POST /v1/notifications                                         │
  POST /v1/broadcasts │  ┌──────────┐   ┌────────────────────┐   │
                      │  │ API      │   │ Workers            │   │
<Inbox /> widget ─────►  │ plane    │   │ (counters, fanout, │   │
  REST + SSE          │  │          │   │  push transports†) │   │
                      │  └────┬─────┘   └─────────┬──────────┘   │
                      │       │                   │              │
                      └───────┼───────────────────┼──────────────┘
                              │                   │
                   ┌──────────▼─────────┐  ┌──────▼──────┐
                   │ Postgres           │  │ Redis       │
                   │ source of truth:   │  │ real-time:  │
                   │ notifications,     │  │ SSE pub/sub │
                   │ broadcasts, outbox │  │ counters,   │
                   │ jobs, read state   │  │ rate limits │
                   └────────────────────┘  └─────────────┘
                                              † post-launch
```

Division of labor: Postgres owns durability and correctness (notifications, broadcasts, read state, transactional outbox, job queue via `FOR UPDATE SKIP LOCKED`). Redis owns the hot path (cross-replica SSE fan-out via pub/sub, unread-counter cache, per-key rate limiting). Redis is part of the standard deployment; a Redis-less mode (LISTEN/NOTIFY fan-out, counters from Postgres) exists for dev and tiny single-node self-hosts, clearly documented as the degraded path.

## Core domain model

Deliberately small — this is the internal difference from Novu:

- **Subscriber** — end user of the customer's product, identified by customer-provided `subscriber_id`. Modeled one-subscriber-many-endpoints from day 1: a future `push_subscriptions` table hangs off it, and nothing in the API treats subscriber ↔ device as 1:1.
- **Notification** — the only first-class delivery object. `POST /v1/notifications` with `{subscriber_id(s), category, payload, idempotency_key, deliver_at?}`. Typed payload rendered client-side; `deliver_at` gives scheduled delivery without a workflow engine.
- **Broadcast** — one row per announcement targeting an environment or topic, fanned out on read, never materialized per subscriber. `POST /v1/broadcasts`.
- **Category** — customer-defined notification type (`payment.failed`). Drives client-side rendering and per-subscriber preferences. No templates server-side.
- **Preferences** — per-subscriber, per-category, per-channel mute (`channel = 'in_app'` is the only value in v1). Evaluated at read time for in-app; at send time for push transports later. The channel column exists from day 1 so adding push never requires a preferences migration.
- **Environment** — instance-level named space with its own API keys and full data isolation (`environment_id` in every key). Not hardcoded to dev/prod: a company with multiple consumer apps runs `dashboard-prod`, `mobile-prod`, `dashboard-dev`, etc. on one instance. Environments are how "multi-consumer" works without an org layer.

No workflows, no steps, no triggers-as-indirection. The customer's backend decides *what* to send and *when*; Dronte makes it durable, real-time, and renderable.

## DX contract (the part that must match Novu's bar)

**Server side — one call:**

```bash
curl -X POST https://dronte.example.com/v1/notifications \
  -H "Authorization: Bearer $DRONTE_API_KEY" \
  -d '{"subscriber_id":"usr_42","category":"payment.failed","payload":{"amount":4200,"currency":"USD"},"idempotency_key":"evt_9f8a"}'
```

**Client side — one component:**

```tsx
<Inbox subscriberId={user.id} subscriberHash={hash} appearance={{...}} />
```

Plus `useNotifications` / `useUnreadCount` hooks for headless use. SDK layering: all logic in `@dronte/client` (framework-agnostic TS core — auth, SSE reconnect, store, optimistic updates), `@dronte/react` is bindings + the styled-but-overrideable `<Inbox />` (render props, CSS variables, zero styling dependencies). Community Vue/Svelte bindings build on the core.

**Widget auth:** HMAC subscriber hash — `HMAC-SHA256(api_secret, subscriber_id)` computed by the customer backend, passed to `<Inbox />`. Mandatory in production environments, optional in development so the quickstart works in 30 seconds. JWT mode as a v2 option.

## Reliability design

Every guarantee below is a documented, tested invariant.

**Ingestion idempotency.** `idempotency_key` on every create (client-supplied or server-generated, echoed in the response). Unique on `(environment_id, idempotency_key)`. Retries are acknowledged-and-dropped.

**Transactional outbox.** Notification insert + outbox/job row in one Postgres transaction. No dual-write between DB and Redis: Redis pub/sub hints are published by a worker draining the outbox, so a Redis outage delays hints but never loses notifications.

**At-least-once workers, idempotent effects.** Workers claim via `FOR UPDATE SKIP LOCKED`; every side effect (counter bump, hint publish, future push send) is keyed and replay-safe. Completed jobs are DELETEd immediately (optional `jobs_archive`), never status-flagged in place. Jobs carry a `progress_cursor` so large fan-outs (a future broadcast-to-push over millions of endpoints) run as resumable chunked jobs — never one giant transaction, never N tiny rows.

**Real-time push as hint, not transport.** SSE with `Last-Event-ID` resume; the client refetches via REST on every hint and on reconnect. Redis pub/sub for cross-replica fan-out (LISTEN/NOTIFY in Redis-less mode). Missed hints are harmless by construction.

**"Mark all read" watermark.** Per-subscriber `last_read_at` watermark instead of bulk UPDATE; read = `read_at IS NOT NULL OR created_at <= watermark`. Avoids MVCC bloat on the hottest write path.

**Status timeline.** Append-only status log per notification (`created → delivered_hint → seen → read`). Exposed in API and admin UI — the "did it send?" answer, and the foundation for push delivery receipts later.

**HA posture.** Binary is stateless; N replicas behind any LB. Leaderless workers (SKIP LOCKED distributes work). Graceful shutdown: stop claiming, finish in-flight, close SSE streams with retry hints. `/healthz`, `/readyz` gating on Postgres + migrations (Redis degraded-OK, not readiness-fatal). Migrations embedded, run on boot under an advisory lock.

**Operational hygiene.** `tracing` + OTLP, Prometheus `/metrics` (queue depth, hint latency, SSE connection count, counter drift), structured JSON logs, per-API-key rate limiting (Redis-backed token bucket), monthly partitioning on `notifications` with retention config.

## Scalability design

Made up front because most are schema-level and retrofit-hostile.

**Broadcast fan-out-on-read.** Direct notifications fan out on write; broadcasts never do. List query and unread count merge both sources; broadcast read state = watermark + an exceptions table for individually-read broadcasts. "Announce to all subscribers" is one row regardless of tenant size.

**Maintained unread counters.** `count(*)` is O(unread) and unread count is the hottest read. A `subscriber_counters` table is updated transactionally with inserts and watermark moves; Redis caches it with Postgres authoritative. Broadcast contribution computed from per-environment broadcast counters vs. the subscriber watermark.

**Per-environment queue fairness.** Single-org removes hostile-tenant starvation, but a burst from one consumer app (a broadcast from `dashboard-prod`) must not starve another's real-time notifications. The claim query round-robins across environments with pending work; ingestion rate-limited per API key.

**Scale-up over shard-out.** Single-org eliminates the multi-tenant sharding problem; the scaling story is one Postgres scaled vertically, partitioning, and Redis offloading the hot reads. `environment_id` stays in every PK, unique constraint, and FK regardless — it's nearly free, it's what enforces hard isolation between consumer apps, and it preserves optionality if a hosted offering ever exists (which would be instance-per-customer, Plausible-style, not shared tenancy).

**Jobs-table MVCC hygiene.** Delete-on-complete, aggressive per-table autovacuum, `fillfactor` for HOT updates. Documented ceiling: low thousands of jobs/sec — plenty for an inbox, not Kafka.

**Redis pub/sub for fan-out, debounced.** At most one hint per subscriber per interval, batched — never one publish per row. In Redis-less mode the same debouncing applies to NOTIFY (which serializes at commit), and the LISTEN connection is dedicated and direct (transaction-mode PgBouncer breaks LISTEN; docs say so loudly).

**Deploy-time thundering herd.** Jittered exponential reconnect in `@dronte/client`, retry hints on graceful SSE close, ETag/`If-None-Match` on the list endpoint so reconnect refetches are mostly 304s. Part of the SDK contract.

**Read-replica caveat (documented, not built).** Replica reads introduce read-your-writes anomalies (mark read → lagged refetch → notification reappears). If ever added: sticky-to-primary after writes.

## Deliverables and repo layout

```
dronte/
  server/              Rust binary: API, SSE, workers, embedded admin UI
  packages/
    client/            @dronte/client — headless TS core
    react/             @dronte/react  — hooks + <Inbox />
  examples/            next.js quickstart, vite, axum trigger example
  docs/                fumadocs site (Next.js)
```

## Stack decisions (settled — sessions do not relitigate)

**Server:** Rust stable (2024 edition, pinned via rust-toolchain.toml), axum 0.8 on tokio, sqlx (compile-time-checked raw SQL; built-in migrator, run on boot under advisory lock), Postgres ≥15, `fred` Redis client (resilient pub/sub), Redis Lua token bucket for cross-replica rate limiting, RustCrypto hmac+sha2, thiserror/anyhow, tracing + OTLP, metrics + Prometheus exporter. Single crate until compile times force a split.

**Contract tooling:** code-first via utoipa, rendered docs served from the binary via utoipa-scalar at /docs. Since the v1 flip the generated spec (`cargo run -- openapi`) is the published artifact; the hand-written convergence target retired to project/archive-v1/openapi.yaml. The `contract` CI job runs oasdiff breaking-change detection of the live spec against project/openapi-baseline.yaml (the export frozen at the last release). openapi-typescript consumes the generated spec for @dronte/client types in the same CI step. Annotation-vs-handler drift (utoipa response codes are hand-annotated) is guarded by the Rust contract-drift integration tests (server/tests/redteam_contract_drift*.rs), which assert the status a handler returns is the status its annotation declares. The light schemathesis run named earlier in this plan was never wired into CI; these tests are the guard in its place.

**Testing:** testcontainers-rs (Postgres + Redis), cargo-nextest, proptest for two-source merge and watermark invariants.

**TypeScript:** pnpm workspaces, tsup, vitest, Biome, changesets. `<Inbox />`: plain CSS with custom properties, @floating-ui/dom as the only runtime UI dep, no Tailwind in published packages.

**Admin SPA:** Vite + React + TanStack Query/Router, embedded via rust-embed.

**Build/ship:** GitHub Actions (Swatinem/rust-cache), cargo-chef multi-stage Docker, debian-slim image. Docs: Fumadocs (Next.js), with fumadocs-openapi rendering the exported spec so the docs site stays generated-from-code too. `npx dronte dev`: postgresql_embedded, Redis-less mode (exercises the LISTEN/NOTIFY fallback).

## Licensing

**SDKs and examples — MIT.** `packages/client`, `packages/react`, and `examples/` ship MIT unconditionally. These embed in customers' frontends; any copyleft here would kill adoption, and the SDKs are the distribution channel.

**Server — FSL-1.1-MIT** (decided 2026-06; previously AGPL-3.0-only). The plan's original decision rule — "switch the server to FSL *before* the first external contribution is merged" — was exercised while every commit was still the owner's, so the relicense required no third-party consent. FSL keeps everything self-hosters care about (free use, modification, redistribution, production at any scale) and prohibits exactly one thing: offering Dronte as a competing commercial product or service. Each release converts to MIT on its second anniversary, so nothing stays locked up forever. The server is "fair source", never "open source", in all docs and marketing; the README's license FAQ spells out the boundary because the practical answer for users is unchanged from the AGPL era: nothing reaches their codebase.

**Contribution mechanics:** under FSL with exclusive commercialization, external code contributions require a CLA before merging — it is the only mechanism that grants the rights needed to sell commercial licenses over contributed code. The CLA must be in place before the first external PR is accepted. (DCO sign-off, used in the AGPL era, was dropped: it certifies origin only and is redundant once a CLA is mandatory.)

**Adjacent rights:** the generated OpenAPI spec and docs content are MIT/CC-BY so third-party clients and integrations are unambiguous. The Dronte name and logo are not licensed — trademark stays with the project regardless of code license, which is the actual protection against confusing forks.

## Phases

**Phase 0 — Claims and scaffolding (days).** GitHub org, npm scope `@dronte`, crates.io name, dronte.dev. Monorepo, CI (fmt/clippy/test with Postgres+Redis service containers; typecheck/vitest for packages; changesets; Docker publish). Licensing per the Licensing section: LICENSE files in place (server license per the Licensing section, MIT packages/examples).

**Phase 1 — Core inbox engine (2–3 weeks).** Schema (subscribers, notifications, broadcasts, subscriber_counters, broadcast read exceptions, outbox/jobs with `progress_cursor`, environments, api_keys, preferences with `channel` column — `environment_id` in every key), notifications + broadcasts endpoints with idempotency and `deliver_at`, SKIP LOCKED worker loop with per-environment fair claiming and delete-on-complete, subscriber REST API (keyset-paginated merged list, maintained unread count, watermark read marks, per-category preferences, ETag), SSE with debounced Redis pub/sub hints (LISTEN/NOTIFY fallback), HMAC auth, migrations-on-boot, health endpoints.

**Phase 2 — Client SDKs and `<Inbox />` (2–3 weeks, parallelizable with late Phase 1).** `@dronte/client` with jittered-backoff reconnect/resume, ETag-conditional refetch, optimistic read-state, pagination. `@dronte/react` hooks + `<Inbox />`: bell, badge, popover list, infinite scroll, category preference panel, render-prop overrides, CSS variables, zero styling deps. Next.js example doubles as the live demo.

**Phase 3 — Reliability hardening (1–2 weeks).** Retry/backoff/DLQ for workers, status timeline, Prometheus + OTLP, rate limiting, graceful shutdown, partitioning + retention. Chaos tests: kill workers mid-job, kill SSE mid-stream, duplicate creates, replica migration races, Redis outage (hints delayed, nothing lost), tenant flood (fairness holds), sustained jobs-table load (vacuum keeps up).

**Phase 4 — Admin dashboard (1–2 weeks).** Embedded SPA: notification/status browser, broadcast composer, subscriber lookup, DLQ replay, API key + environment management. Instance-level admin auth (static credential via env var at launch; OIDC later) — no org or user management to build, which is most of why this phase is short.

**Phase 5 — Launch.** Live demo without signup, docker-compose (dronte + Postgres + Redis) and Redis-less one-liner both front and center, honest "Dronte vs Novu" page (no workflows, no email — by design), 10-minute Next.js quickstart, `npx dronte dev`. HN / r/selfhosted / r/rust. Dronte-is-Dutch-for-dodo footnote in the launch post.

**Phase 6+ — Push transports (post-launch, demand-driven).** Web-push first (VAPID, `push_subscriptions` per subscriber-endpoint, send via isolated worker pool with strict timeouts, prune on 404/410), then mobile push (FCM, APNs) behind a `Transport` trait. Same notification object, per-category + per-transport preferences, delivery receipts feeding the status timeline. Broadcast-to-push runs as chunked resumable fan-out jobs. The v1 pre-wires (preference `channel` column, job `progress_cursor`, many-endpoint subscriber model) make this phase purely additive — no migrations of live semantics. The SDK service-worker story (registration, permission UX, push handler, bundler packaging) is the bulk of the effort. Digest/batching, if ever, is an ingestion-side aggregation option — still no workflow engine.

## Explicit non-goals

No multi-org tenancy (run another instance), no workflow engine, no step model, no digests/delays-as-steps (`deliver_at` covers scheduling), no email/SMS/chat channels, no server-side templates or translation UI, no visual builders, no outbound webhooks, no Mongo/Kafka backends, no read replicas, no multi-region. The comparison page celebrates this list.

## Risks

- **Scope creep toward Novu parity** — the non-goals list is the product strategy; revisit only on user demand, not completeness anxiety.
- **Two-source inbox complexity** — the direct+broadcast merge touches list, count, watermark, and SSE. It is the hardest correctness surface; Session 0 schema review and Phase 3 chaos tests concentrate there.
- **Redis as semi-required dependency** — softens the "one dependency" pitch; mitigated by a real, documented Redis-less mode and honest guidance on when it suffices.
- **SDK API churn** — `<Inbox />` props and `@dronte/client` are public contracts; design review before v0.1 publish, semver discipline after.
- **Solo-maintainer bus factor** — small API surface and heavy integration tests keep contributions approachable; good-first-issues from day one.

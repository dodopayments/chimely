# Phase 1 — Core inbox engine

> Derived from `docs/dronte-project-plan.md` (Phases, "Phase 1") and
> `docs/risk-register.md` (W1, W2, W4, M2, M3, M6). Self-contained: restates
> every invariant it touches. The contracts it implements are frozen:
> `specs/schema.sql`, `specs/openapi.yaml` (convergence target),
> `specs/sdk-api.d.ts`. **Never edit the contracts to match the code.**
>
> Estimated 2–3 weeks. Known risk (M5): this phase is front-loaded by the
> contract — deliver_at, seen watermarks, ETag, and fairness are all
> published. If it slips, re-plan openly; nothing can be cut silently.

## Goal

A single `dronte` binary that implements the entire v1 HTTP contract —
management plane, subscriber plane, SSE — on Postgres (authoritative) +
Redis (hints/cache), with the schema from `specs/schema.sql` migrated on
boot and every reliability invariant below enforced and tested.

## Deliverables

1. **Migrations** (sqlx embedded migrator; run on boot under an advisory
   lock so N replicas race safely):
   - The full `specs/schema.sql` table set: environments, api_keys,
     subscribers, notifications (monthly RANGE partitions on `visible_at`),
     broadcasts, broadcast_reads, subscriber_counters, jobs,
     idempotency_keys, preferences (with `channel`, default `'in_app'`).
   - `typeid_format` / `typeid_parse` SQL helpers (risk W1 — promised by
     schema.sql comments; ship in the first migration).
   - Partition maintenance job (boot + daily, same advisory lock):
     pre-create partitions covering `[now - retention, now + 13 months]`,
     retention = DETACH + DROP, and a `dronte_partitions_remaining` metric
     with documented alert threshold (risk W4 — a stalled maintenance job
     plus exhausted headroom is a total write outage; there is deliberately
     NO DEFAULT partition, so a missing partition fails loudly).
2. **Auth.** Management: Bearer key, sha256 hash lookup over non-revoked
   keys. Subscriber: `HMAC-SHA256(secret, subscriber_id)` verified against
   current-then-previous secret slots; enforced when
   `environments.require_subscriber_hash = true`, optional otherwise.
   Headers with query-parameter fallbacks exactly as specs/openapi.yaml
   defines them.
3. **Management endpoints.** `POST /v1/notifications` (1–100 recipients,
   lazy subscriber upsert, `deliver_at` ≤ 13 months out),
   `POST /v1/broadcasts`, `PUT /v1/subscribers/{id}` (created_at backdate
   on first create only), `GET/PUT /v1/subscribers/{id}/preferences`.
4. **Subscriber endpoints.** `GET /v1/inbox/items` (merged keyset list,
   ETag/If-None-Match, `Cache-Control: private, max-age=0`),
   `GET /v1/inbox/counts`, `POST /v1/inbox/notifications/{id}/read`,
   `POST /v1/inbox/broadcasts/{id}/read`, `POST /v1/inbox/read-all`,
   `POST /v1/inbox/seen-all`, `GET/PUT /v1/inbox/preferences`,
   `GET /v1/inbox/stream` (SSE).
5. **Worker loop.** `FOR UPDATE SKIP LOCKED` claims, round-robin across
   environments with pending work, DELETE on completion. Job types:
   `hint` (debounced pub/sub publish), `deliver` (scheduled notification
   coming due), `counter_rebuild` (recount one subscriber after a
   preference flip). Redis pub/sub for cross-replica hint fan-out;
   LISTEN/NOTIFY fallback in Redis-less mode (dedicated direct connection —
   transaction-mode PgBouncer breaks LISTEN; document loudly).
6. **SSE.** `hint` events with opaque resume tokens, comment-frame
   keep-alive (`: ping`, every 30s), `Last-Event-ID` answered with an
   immediate hint if anything changed, graceful-shutdown `retry:` directive
   with jitter. Per-subscriber connection caps (risk M3 — dev environments
   without subscriber hashes are otherwise an open connection-exhaustion
   relay). `subscriber_hash` scrubbed from access/proxy log lines for this
   endpoint (tested invariant — query-string credentials leak into logs).
7. **Health.** `/healthz` liveness; `/readyz` gates on Postgres
   connectivity + migrations applied. Redis is degraded-OK and NOT
   readiness-fatal.
8. **utoipa annotations** for everything above, converging the exported
   spec (`dronte openapi`) toward specs/openapi.yaml, including the
   info description, security schemes, and component schemas.

## Invariants in play (restated; violating any is a bug)

- **Two-source inbox.** Direct notifications fan out on WRITE (a row per
  recipient); broadcasts fan out on READ (one row per announcement, never
  materialized per subscriber). List, unread count, and read state must
  agree across both sources at all times.
- **Ordering spine.** `visible_at = COALESCE(deliver_at, created_at)` is
  the timestamp for pagination keysets, watermark comparisons, and
  partitioning — never `created_at` (born-read trap; retention alignment).
  Ordering timestamps are DB-clock-sourced (`now()` inside the INSERT),
  never computed by app replicas.
- **Scheduled invisibility.** Rows with `visible_at > now()` are excluded
  from all subscriber queries; counters are NOT bumped at create for
  scheduled rows — the deliver job bumps them in the SAME transaction that
  deletes the job row (job deletion is the exactly-once key).
- **Watermark reads.** Direct item read ⟺ `read_at IS NOT NULL OR
  visible_at <= read_watermark`; broadcast read ⟺ `broadcast_reads` row
  exists OR `created_at <= read_watermark`. Mark-all-read moves the
  watermark (one-row update) and GCs exception rows at or below it —
  NEVER a bulk UPDATE over notifications. Seen state is watermark-only.
- **Counter maintenance.** Immediate-visible insert bumps counters in the
  same txn as the insert, as a CONDITIONAL increment
  (`+= (visible_at > read_watermark)::int` — the guard against the
  mark-all-read race). Individual mark-read decrements only if the row was
  unread above the watermark. EVERY read-state mutation bumps
  `subscriber_counters.updated_at` in the same txn (it is an ETag input;
  skipping it serves stale 304s). Counters ignore category mutes;
  a preference flip enqueues `counter_rebuild` for that one subscriber.
- **Transactional outbox.** Notification rows + counter bumps +
  idempotency snapshot + outbox job commit in ONE transaction. No
  Postgres↔Redis dual writes anywhere.
- **Idempotency.** Unique on (environment_id, scope, idempotency_key) in
  `idempotency_keys`; a retried key returns the original response
  byte-identically (200; first acceptance 201) and never partially re-runs
  a batch.
- **Jobs.** Deleted on completion, never status-flagged. `progress_cursor`
  exists from day 1; large fan-outs run as resumable chunked jobs. Claims
  round-robin across environments — one environment's burst must not
  starve another's real-time jobs.
- **Redis is the hint/cache plane.** Its loss may delay hints but must
  never lose data; Postgres is always authoritative. SSE is a hint, not a
  transport: clients refetch via REST; missed hints are harmless by
  construction. Hints are debounced (at most one per subscriber per
  interval) — never one publish per row.
- **Broadcast visibility.** A subscriber sees a broadcast iff
  `broadcasts.created_at >= subscribers.created_at` (backdatable on
  import).
- **Isolation.** `environment_id` is in every PK, UNIQUE constraint, and
  FK. Single-org: no organization concept anywhere. Preferences carry
  `channel` (only `'in_app'` for now; no CHECK constraint — the API layer
  owns the allowed-values list). Subscribers are one-to-many endpoints.
- **ETag validator inputs** (list endpoint): request cursor,
  `subscriber_counters.updated_at`, latest direct item `(visible_at, id)`,
  latest broadcast `(created_at, id)`, `max(preferences.updated_at)`.
  Each one index-only; anything that can change a page moves at least one.
- **Redis count-cache epoch (risk M2).** Before unread counts are cached
  in Redis, the cache key must include an environment-level epoch so a
  broadcast create can invalidate every subscriber's cached count without
  a scan. Design note required in this phase even if caching itself slips.

## CI gates added in this phase

- **Migration lint (risk W2):** any new table whose PK/UNIQUE lacks
  `environment_id`, or any column with a serial/sequence default, fails CI
  (environments itself is the allowlisted root). Run against the migrated
  testcontainers database via catalog queries.
- **Schemathesis light run (risk M6):** generated-spec-vs-handler drift
  guard (utoipa response codes are hand-annotated).
- The `contract` job keeps running oasdiff (still allowed to fail until
  Phase 2 completes).

## Out of scope (deliberately)

Rate limiting + 429/Retry-After enforcement, retry/backoff/DLQ, status
timeline, partition retention tuning, chaos tests (all Phase 3). Admin UI
(Phase 4). Cancel/retract endpoints (designed in Phase 2, risk M1). Topic
targeting, broadcast deliver_at, digests, push (post-launch).

## Acceptance criteria

- [ ] **oasdiff delta has shrunk to only not-yet-implemented surface.**
      `oasdiff diff specs/openapi.yaml <(cargo run -- openapi)` lists
      nothing for implemented endpoints/schemas/headers; the remaining
      delta is enumerated in the phase-exit notes and consists only of
      items scheduled for later phases (expected: the 429/`Retry-After`
      declarations, which land with Phase 3 rate limiting). Anything else
      in the diff is unfinished Phase 1 work.
- [ ] All endpoints serve the contract on a fresh deploy (single binary +
      Postgres + Redis); schemathesis run green; the same suite passes in
      Redis-less mode (LISTEN/NOTIFY fallback).
- [ ] The canonical merged-list and unread-count queries from
      specs/schema.sql are implemented in their specified shape; EXPLAIN
      confirms `notifications_inbox_idx` and `broadcasts_window_idx` serve
      the two arms.
- [ ] proptest suites: random interleavings of create / schedule / read /
      read-all / seen-all / preference-flip / broadcast keep list, counts,
      and read state in agreement across both sources (the two-source merge
      and watermark invariants).
- [ ] testcontainers integration tests (real Postgres + Redis; no storage
      or pub/sub mocks) cover at minimum: byte-identical idempotent replay;
      deliver_at flow (invisible → visible, counters bumped in the
      job-deletion txn; kill the worker mid-deliver and verify replay
      safety); conditional-increment race (mark-all-read concurrent with
      insert → no drift); broadcast_reads GC on watermark move; ETag
      changes on EVERY read-state mutation (incl. mark-broadcast-read);
      hint debounce; per-environment claim fairness under a one-env flood;
      month-boundary insert lands in a pre-created partition.
- [ ] Migration lint green; `typeid_format`/`typeid_parse` exist and
      round-trip API-shaped ids.
- [ ] `/readyz` is ready iff Postgres reachable + migrations applied;
      Redis down ⇒ still ready, hints delayed, NOTHING lost (test kills
      Redis, creates notifications, restores Redis, asserts delivery of
      hints and zero data loss).
- [ ] `subscriber_hash` never appears in logs for the SSE endpoint (tested).
- [ ] Rust CI (fmt, clippy -D warnings, nextest) and generated-artifacts
      job stay green throughout.

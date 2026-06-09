# Risk register

Known risks with no contract home yet. Spec-encodable gaps do NOT belong
here — they get fixed in `specs/` directly (see "Resolved into the
contracts" below for the audit trail). Review this file at every phase
boundary; an item leaves by becoming a tested invariant, a shipped
endpoint, or an explicitly accepted trade-off noted in the plan.

## Resolved into the contracts (2026-06-10)

Encoded in `specs/` in the same commit that added this file:

- **DB-clock timestamp sourcing** — ordering timestamps computed by Postgres
  inside the INSERT, never by app replicas (schema.sql header).
- **Conditional counter increment** — `+= (visible_at > read_watermark)::int`
  guard against the mark-all-read race (schema.sql counter invariants).
- **ETag validator inputs defined** — counters.updated_at + latest direct
  item + latest broadcast + prefs max(updated_at); every read-state mutation
  must bump counters.updated_at (openapi.yaml list endpoint + schema.sql).
- **`Cache-Control: private`** on inbox responses (openapi.yaml).
- **Log scrubbing of `subscriber_hash`** on the SSE endpoint named a tested
  invariant (openapi.yaml stream description).
- **`EventSourceLike` error events** — reconnect/backoff depends on them
  (sdk-api.d.ts).
- **`InboxLocalization` index signature removed** — typos no longer
  type-check (sdk-api.d.ts).

## Next week — Phase 0 / Phase 1 entry criteria

| # | Risk | Bites when | Planned home |
|---|------|------------|--------------|
| W1 | `typeid_format`/`typeid_parse` SQL helpers are promised by schema.sql comments but don't exist | First debugging session with an API-shaped id | First migration |
| W2 | Shard-invariant CI lint (environment_id in every PK/unique, no sequences) is promised but unwritten | First migration that forgets — silently unshardable | Phase 0 CI |
| W3 | License decision (MIT vs FSL) deferred to "before first external contributor" — in practice that's launch day on HN | First external PR arrives before the decision | Phase 0, hard deadline |
| W4 | No-DEFAULT-partition choice means a stalled partition-maintenance job = total write outage at a month boundary | Maintenance job silently broken for >13 months of pre-created headroom — or headroom never created | Phase 1: maintenance job + alert on partitions-remaining < N |

## Next month — Phase 1–2

| # | Risk | Bites when | Planned home |
|---|------|------------|--------------|
| M1 | **No undo anywhere**: no cancel for scheduled notifications, no broadcast retraction, no notification delete. A typo'd broadcast is permanent; a scheduled trial-ending notice fires after the user upgrades | First real customer "oops", week one of usage | Design in Phase 2 (cancel-vs-deliver-job race needs thought), ship as additive endpoints |
| M2 | Redis count-cache invalidation on broadcast create is env-wide — without an env-level epoch in the cache key, every subscriber's cached count goes stale | First broadcast after Redis caching lands | Phase 1 design note, before counters hit Redis |
| M3 | No per-subscriber SSE connection caps; dev environments (`require_subscriber_hash=false`) are an open connection-exhaustion relay | A staging URL leaks | Phase 1 |
| M4 | SDK publish freezes the contract — remaining pre-publish review items: provider-vs-standalone precedence rules, slot list completeness, appearance variable names | `npm publish` of v0.1 | Phase 2 gate: design review checklist |
| M5 | Phase 1 scope is front-loaded by the contract (deliver_at, seen watermarks, ETag, fairness all published) — "2–3 weeks" is the plan's most optimistic number for a solo maintainer | Week 3 of Phase 1 | Re-plan openly if slipping; nothing can be cut silently anymore |
| M6 | **Meta-risk**: nothing binds spec to implementation — the three spec files drift into aspirational documentation | Quietly, by month two | Phase 1 CI: schemathesis against openapi.yaml; schema.sql canonical queries as integration tests |

## Parked — year horizon (revisit at Phase 3 and at launch)

- **GDPR erasure**: no `DELETE /v1/subscribers/{id}`; notification FKs don't
  cascade; deletion is a cross-partition scatter — wants a chunked job design
  before the first EU customer.
- **Jobs lease column**: chunked fan-outs release row locks per chunk commit
  → double-claim; Phase 6 push fan-out needs `lease_until` + long-txn vacuum
  discipline.
- **Intra-environment starvation**: fairness is per-env only; one env's
  broadcast fan-out vs its own real-time hints needs job priority classes.
- **Redis-less mode** is a permanent second test matrix; kill it or fund it
  when evidence arrives.
- **Broadcast immortality**: no expiry/archival; `expires_at` is additive but
  its watermark semantics are not trivial.
- **Webhook pressure**: "tell my backend when the user reads X" will be the
  top ask; the planned escape hatch is status-timeline export, not inline
  webhooks — design it before improvising under pressure.
- **HMAC secrets at rest**: stored plaintext by necessity; document KMS
  wrapping for self-hosters whose threat model includes DB dumps.

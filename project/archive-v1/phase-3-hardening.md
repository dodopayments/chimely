# Phase 3 — Reliability hardening

> Derived from `docs/dronte-project-plan.md` (Phases, "Phase 3";
> "Reliability design"; "Scalability design"; "Operational hygiene") and
> `docs/risk-register.md` (W4 follow-through, M2 if still open, parked
> items reviewed at this boundary). Self-contained: restates every
> invariant it touches. 1–2 weeks.

## Goal

Turn every documented guarantee into a chaos-tested one. After this phase,
killing any process at any moment, losing Redis entirely, flooding one
environment, or replaying any job must not lose data, drift a counter, or
starve a neighbor — and the metrics must prove it.

## Deliverables

1. **Worker retry/backoff/DLQ.** Failed jobs reschedule with exponential
   backoff in `run_at`, `attempts += 1`, `last_error` recorded. Jobs
   exhausting `max_attempts` are parked for DLQ replay (separate parked
   state/table per the delete-on-complete rule — a parked job is not a
   completed job; completed work still leaves NO row). Optional
   `jobs_archive` for completed-job forensics.
2. **Status timeline.** Append-only status log per notification
   (`created → delivered_hint → seen → read`), exposed in the API — the
   "did it send?" answer and the foundation for push delivery receipts.
   Additive endpoint(s) in the generated spec.
3. **Observability.** Prometheus `/metrics` complete: queue depth (per
   environment and job type), hint publish latency, SSE connection count,
   counter drift (sampled recount vs maintained value),
   `dronte_partitions_remaining`, claim fairness. OTLP traces across
   ingest → outbox → worker → hint. Structured JSON logs everywhere;
   `subscriber_hash` scrubbing re-verified.
4. **Rate limiting.** Per-API-key (management) and per-subscriber
   (widget) limits via the Redis Lua token bucket (cross-replica
   correctness); 429 + `Retry-After` exactly as specs/openapi.yaml
   declares. In Redis-less mode: in-process limiter, documented as
   single-node semantics. This closes the last expected oasdiff delta —
   the contract gate (now required) must stay green.
5. **Graceful shutdown, complete.** Stop claiming; finish in-flight jobs;
   close SSE streams with jittered `retry:` directives; drain within a
   deadline; readiness flips before the listener closes.
6. **Partitioning + retention.** Monthly partition retention
   (DETACH + DROP) on the configured horizon; maintenance-job alerting
   (W4) proven by a test that simulates a stalled job and asserts the
   metric/alert fires well before headroom exhausts.
7. **Chaos test suite** (testcontainers; real Postgres + Redis; no mocks):
   - kill a worker mid-job (before/after partial side effects) →
     at-least-once replay, no double-applied effects, no lost jobs;
   - kill the server mid-SSE-stream → clients refetch on reconnect, no
     missed state;
   - duplicate creates under concurrency → exactly one notification set,
     byte-identical replays;
   - migration race: N replicas booting concurrently → advisory lock
     serializes, one migrator wins, others proceed;
   - Redis outage (full loss, then recovery) → hints delayed, NOTHING
     lost, counters recoverable from Postgres, /readyz stays ready;
   - tenant flood: one environment saturates the queue → other
     environments' job latency stays bounded (fairness holds);
   - sustained jobs-table churn at target rate → autovacuum keeps up
     (table size and dead tuples bounded over the run).

## Invariants in play (restated; violating any is a bug)

- **At-least-once workers, idempotent effects.** Every side effect
  (counter bump, hint publish, future push send) is keyed and replay-safe.
  For scheduled delivery, the job-row DELETE is the exactly-once key: the
  counter bump and hint enqueue commit in the SAME transaction that
  deletes the job.
- **Jobs are deleted on completion — never status-flagged in place.**
  Parked (DLQ) jobs are the explicit exception and live outside the hot
  claim path. MVCC hygiene is part of the contract: fillfactor 50,
  threshold-based aggressive autovacuum, delete-on-complete keeps the
  table near-empty at steady state. Documented ceiling: low thousands of
  jobs/sec — this is an inbox, not Kafka.
- **Resumable chunked fan-outs.** Large jobs process a chunk, advance
  `progress_cursor`, COMMIT; a crashed worker's successor resumes from the
  cursor; chunk effects are keyed so replaying the last uncommitted chunk
  is safe. Never one giant transaction, never N tiny rows.
- **Per-environment fairness.** The claim query round-robins environments
  with pending work (one SKIP LOCKED claim per env per sweep); a broadcast
  burst from one consumer app must not starve another's real-time hints.
- **Redis is the hint/cache plane.** Its loss may DELAY hints; it must
  never LOSE data. Postgres is always authoritative; cached counters are
  recomputable at any moment. The count cache key includes an environment
  epoch (M2) so broadcast creates invalidate without a scan.
- **SSE is a hint.** Debounced (at most one hint per subscriber per
  interval, batched — never one publish per row); clients refetch via
  REST; missed hints harmless by construction. Graceful close sends
  jittered `retry:` so a deploy cannot produce a reconnect stampede;
  ETag/If-None-Match keeps the post-deploy refetch storm mostly 304s.
- **Watermark + counters agreement under crashes.** The conditional
  counter increment (`+= (visible_at > read_watermark)::int`) and the
  decrement guard (only if unread above the watermark) must hold under
  arbitrary kill points — the chaos suite's counter-drift metric is the
  proof.
- **environment_id in every key; single-org; no DEFAULT partition;**
  ordering timestamps DB-clock-sourced — unchanged from Phase 1 and
  re-asserted by every chaos test's final consistency check.

## Out of scope (deliberately)

Read replicas (documented caveat only). Multi-region. Outbound webhooks
(the escape hatch is status-timeline export — design it before improvising
if pressure arrives early). Jobs lease column / priority classes (parked,
year horizon — revisit if chunked fan-outs land earlier than push).

## Acceptance criteria

- [ ] Every chaos scenario above is an automated test in CI (nightly or
      labeled if too slow for every PR), each ending with a full
      consistency sweep: recounted counters == maintained counters, list
      == merge of sources, no orphaned jobs, no missing partitions.
- [ ] Counter-drift metric reads 0 across the entire chaos suite.
- [ ] Measured and documented: max sustained jobs/sec on the reference
      box before claim latency or vacuum falls behind (the "low
      thousands" claim becomes a number).
- [ ] 429 + `Retry-After` enforced and annotated; rate limits are
      cross-replica correct (two replicas, one bucket); the required
      `contract` job is green — the oasdiff delta is now permanently
      empty until the v1 release flips the gate to breaking-change
      detection.
- [ ] DLQ: an exhausted job parks with its `last_error`; replay re-runs it
      exactly-once-observably; parked jobs are visible in metrics.
- [ ] Status timeline rows append for every transition; no UPDATE ever
      touches an existing timeline row.
- [ ] Rolling deploy under live SSE load: zero lost notifications, no
      reconnect stampede (connection-rate metric stays under threshold),
      p99 hint latency recovers within the backoff envelope.
- [ ] Partition retention drops exactly the expired months; the stalled-
      maintenance alert fires in the simulation with ≥ N months of
      headroom remaining.

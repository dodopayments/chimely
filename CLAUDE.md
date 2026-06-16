# Dronte

Fair-source, self-hostable in-app notification inbox infrastructure: one Rust
binary + Postgres (source of truth) + Redis (real-time plane), a small HTTP
API, and a drop-in `<Inbox />`. The full plan lives in
`docs/dronte-project-plan.md`; week-scale risks in `docs/risk-register.md`.

## Repo layout

```
server/            Rust binary (single crate): API, SSE, workers
packages/client/   @dronte/client — headless TS core
packages/react/    @dronte/react  — hooks + <Inbox />
docs/              Fumadocs site (+ project plan, risk register)
project/           archive-v1/ (frozen v1 contracts) + openapi-baseline.yaml
```

## Non-negotiable invariants

Violating any of these is a bug even if all tests pass. They restate the
contracts in `project/archive-v1/schema.sql` (header comments) and the project
plan.

**The two-source inbox.** The inbox is a merge of two sources: direct
notifications (fan-out on write, one row per recipient) and broadcasts
(fan-out on read, one row per announcement, never materialized per
subscriber). The list, the unread count, and read state must agree across
both sources at all times — if a change touches one surface, prove the other
two still agree. A subscriber sees a broadcast iff `broadcast.created_at >=
subscriber.created_at` (`subscribers.created_at` is backdatable on import, so
the customer decides which historical broadcasts a migrated user sees).

**Mark-all-read is a watermark upsert.** Moving the per-subscriber
`read_watermark` is the ONLY implementation — never a bulk `UPDATE` over
notification rows (MVCC bloat on the hottest write path). Read state =
per-item exception OR at-or-below the watermark, for both sources.

**Ordering timestamps come from Postgres.** `created_at`, `visible_at`, and
every watermark move are computed inside the SQL statement
(`now()`/`clock_timestamp()`), never by an app replica. The unread-counter
increment is guarded against the mark-all-read race — the `+1` is conditioned
on `visible_at > read_watermark` and read under the per-subscriber counters
lock, so a concurrent watermark move can never be clobbered.

**Transactional outbox.** The outbox/job row is inserted in the SAME Postgres
transaction as the notification row. No dual-writes between Postgres and
Redis, ever.

**Redis is the hint/cache plane.** Redis loss may DELAY hints; it must never
LOSE data. Postgres is always authoritative — counters cached in Redis are
recomputable from Postgres at any moment.

**SSE is a hint, not a transport.** Clients refetch via REST (conditional,
ETag) on every hint and reconnect. Never treat an SSE event as delivery;
missed hints must be harmless by construction.

**Jobs are deleted on completion** — never status-flagged in place. Jobs
carry a `progress_cursor`, and large fan-outs run as resumable chunked jobs:
never one giant transaction, never N tiny rows.

**Claim queries are fair.** Worker claims round-robin across environments
with pending work (`FOR UPDATE SKIP LOCKED` per env) — one environment's
burst must not starve another's real-time jobs.

**environment_id is part of every key** — every PK, UNIQUE constraint, and
FK. Preferences carry a `channel` column (`'in_app'` is the only value for
now; the column exists so push transports never need a migration).
Subscribers are one-to-many endpoints — nothing may assume subscriber ↔
device is 1:1. No sequences anywhere — every id is an app-generated UUIDv7,
and the migration lint rejects serial/sequence defaults alongside the
missing-`environment_id` check, so id minting needs no cluster-wide
coordination and the schema stays distributable by `environment_id`.

**Single-org.** No organization concept anywhere — not in the schema, not in
the API, not in the admin UI. Environments are the isolation unit;
multi-tenancy is "run another instance". The admin plane is the sole, scoped
exception: it has instance-level **users with four fixed roles**
(`viewer`/`operator`/`developer`/`admin`, capability presets in
`server/src/roles.rs`). Roles are instance-wide — still no organizations, and
no per-environment user scoping. `admin_users`/`admin_sessions` are
instance-level tables (no `environment_id`), allowlisted in the migration
lint like the `environments` root.

**Licensing is settled** (plan, "Licensing"): FSL-1.1-MIT for `server/`
(fair source — free use/self-host, no competing commercialization, each
release converts to MIT after two years), MIT for `packages/*` and
`examples/`. Never call the server "open source" in docs or marketing —
"fair source" or "source available". Never add a dependency whose license
is incompatible with this split; any copyleft transitive dependency in
`server/` (including weak copyleft — MPL, LGPL) must be flagged for review
and explicitly allowed in `server/deny.toml`, never waved through. The
`cargo-deny` CI job is the gate. SDK runtime dependencies must be
permissive (they embed in customer frontends). External code contributions
require a CLA so exclusive commercialization rights hold.

## API contract rules

- The contract is **code-first via utoipa**; `dronte openapi` prints the
  generated spec (CI exports it; the docs site and `@dronte/client` types are
  built from it).
- **The generated spec is the published truth** (since the v1 flip). The
  hand-written convergence target retired to
  `project/archive-v1/openapi.yaml`; never converge code toward it again. The
  `contract` CI job runs oasdiff **breaking-change detection** of the live
  generated spec against `project/openapi-baseline.yaml` (the export frozen at
  the last release). Additive changes pass; a breaking change fails the gate
  and is recorded only by regenerating the baseline in the release commit
  (`cargo run -- openapi > project/openapi-baseline.yaml`).
- The frozen SDK surface (`project/archive-v1/sdk-api.d.ts`) is
  **additive-only**; `pnpm conformance` type-checks the built packages against
  it.
- `packages/client/src/generated/` and `docs/openapi/` are **generated**
  (`pnpm generate`) — never hand-edit them; CI fails if they are stale.
  `project/openapi-baseline.yaml` is NOT part of `pnpm generate`: it is a
  frozen per-release snapshot, bumped by hand only when cutting a release.

## Archived v1 contracts (project/archive-v1, read-only)

`project/archive-v1/{schema.sql,openapi.yaml,sdk-api.d.ts}` are the frozen v1
contracts (tagged `contract-v1`), archived at the v1 flip. They are
historical: the generated spec is now the published truth, so do not edit them
and do not converge code toward `openapi.yaml` anymore. `schema.sql` stays the
reference for the schema invariants the migrations implement, and
`sdk-api.d.ts` is still the additive-only SDK surface `pnpm conformance`
checks. `project/archive-v1/phase-*.md` are the (completed) executable phase
specs.

## Testing

- All DB tests run against **real Postgres + Redis via testcontainers** —
  no mocks for storage or pub/sub, ever. (cargo-nextest is the runner; CI
  also provides Postgres/Redis service containers.)
- Two-source merge and watermark invariants get proptest coverage (Phase 1).

## Comment style

- Comments are factual, not narrative. State the invariant, the contract
  reference, or the failure mode the code cannot express on its own. Do not
  restate what the next line does, address the reader, or argue for the
  change.
- A comment must earn its place. If the code is clear without it, write no
  comment.
- No semicolons and no em-dashes in comments. This applies doubly to doc
  comments (`///`). Write short declarative sentences instead.
- Exception: text quoted verbatim from a frozen contract
  (project/archive-v1/) keeps its original punctuation.
- Long literal text (OpenAPI descriptions and similar) uses raw strings
  (`r#"..."#`) with real newlines, never `\n` escapes.

## Commit & PR style

- Commit subjects and PR titles use Conventional Commits
  (`type(scope): summary`, e.g. `feat(admin): embed the dashboard SPA`).
  Common types: `feat`, `fix`, `refactor`, `docs`, `test`, `build`, `ci`,
  `chore`.
- Keep it concise. Commit summaries are short (target ≤ 50 chars); commit
  bodies and PR descriptions state what changed and why in as few words as
  possible. No verbose prose, no restating the diff.

## Stack decisions (settled — sessions do not relitigate)

**Server:** Rust stable (2024 edition, pinned via rust-toolchain.toml), axum 0.8 on tokio, sqlx (compile-time-checked raw SQL; built-in migrator, run on boot under advisory lock), Postgres ≥15, `fred` Redis client (resilient pub/sub), Redis Lua token bucket for cross-replica rate limiting, RustCrypto hmac+sha2, thiserror/anyhow, tracing + OTLP, metrics + Prometheus exporter. Single crate until compile times force a split.

**Contract tooling:** code-first via utoipa, rendered docs served from the binary via utoipa-scalar at /docs. Since the v1 flip the generated spec (`cargo run -- openapi`) is the published artifact; the hand-written convergence target retired to project/archive-v1/openapi.yaml. The `contract` CI job runs oasdiff breaking-change detection of the live spec against project/openapi-baseline.yaml (the export frozen at the last release). openapi-typescript consumes the generated spec for @dronte/client types in the same CI step. Annotation-vs-handler drift (utoipa response codes are hand-annotated) is guarded by the Rust contract-drift integration tests (server/tests/redteam_contract_drift*.rs), which assert the status a handler returns is the status its annotation declares. The light schemathesis run named in the original plan was never wired into CI; these tests are the guard in its place.

**Testing:** testcontainers-rs (Postgres + Redis), cargo-nextest, proptest for two-source merge and watermark invariants.

**TypeScript:** pnpm workspaces, tsup, vitest, Biome, changesets. `<Inbox />`: plain CSS with custom properties, @floating-ui/dom as the only runtime UI dep, no Tailwind in published packages.

**Admin SPA:** Vite + React + TanStack Query/Router, embedded via rust-embed.

**Build/ship:** GitHub Actions (Swatinem/rust-cache), cargo-chef multi-stage Docker, debian-slim image. Docs: Fumadocs (Next.js), with fumadocs-openapi rendering the exported spec so the docs site stays generated-from-code too. `npx dronte dev`: postgresql_embedded, Redis-less mode (exercises the LISTEN/NOTIFY fallback).

## Commands

```bash
# Rust (run inside server/)
cargo fmt --check && cargo clippy --all-targets -- -D warnings
cargo nextest run
cargo run -- openapi          # print the generated OpenAPI spec

# TypeScript (repo root)
pnpm install
pnpm lint                                # biome ci .
pnpm --filter "./packages/*" build       # react typechecks against client's dist
pnpm typecheck && pnpm test

# Generated artifacts (client types + docs spec) — commit the result
pnpm generate
```

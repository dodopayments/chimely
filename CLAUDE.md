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
specs/             FROZEN v1 contracts — read-only (see below)
```

## Non-negotiable invariants

Violating any of these is a bug even if all tests pass. They restate the
contracts in `specs/schema.sql` (header comments) and the project plan.

**The two-source inbox.** The inbox is a merge of two sources: direct
notifications (fan-out on write, one row per recipient) and broadcasts
(fan-out on read, one row per announcement, never materialized per
subscriber). The list, the unread count, and read state must agree across
both sources at all times — if a change touches one surface, prove the other
two still agree.

**Mark-all-read is a watermark upsert.** Moving the per-subscriber
`read_watermark` is the ONLY implementation — never a bulk `UPDATE` over
notification rows (MVCC bloat on the hottest write path). Read state =
per-item exception OR at-or-below the watermark, for both sources.

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
device is 1:1.

**Single-org.** No organization concept anywhere — not in the schema, not in
the API, not in the admin UI. Environments are the isolation unit;
multi-tenancy is "run another instance".

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
- **Until v1:** `specs/openapi.yaml` is the convergence target. NEVER edit
  `specs/openapi.yaml` to match the code — the oasdiff output (the `contract`
  CI job) is the to-do list, and full equivalence is the Phase 1/2 completion
  criterion.
- **After v1:** the generated spec becomes the published truth, the
  hand-written spec retires, and the CI gate flips to oasdiff
  breaking-change detection against the last release.
- `packages/client/src/generated/` and `docs/openapi/` are **generated**
  (`pnpm generate`) — never hand-edit them; CI fails if they are stale.

## specs/ is read-only

`specs/schema.sql`, `specs/openapi.yaml`, and `specs/sdk-api.d.ts` are frozen
v1 contracts (tagged `contract-v1`). Do not edit them to make code or CI
happy. `specs/phase-*.md` are the executable phase specs derived from the
plan.

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
- Exception: text quoted verbatim from a frozen contract (specs/) keeps its
  original punctuation.
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

**Contract tooling:** code-first via utoipa, rendered docs served from the binary via utoipa-scalar at /docs. Until v1: specs/openapi.yaml (Session 0) is the convergence target — CI exports the generated spec (`cargo run -- openapi`) and gates on oasdiff equivalence; full match is the Phase 1/2 completion criterion. After v1: the hand-written spec retires, the generated spec is the published artifact, and the CI gate becomes oasdiff breaking-change detection against the last release. openapi-typescript consumes the generated spec for @dronte/client types in the same CI step. A light schemathesis run guards against annotation-vs-handler drift (utoipa response codes are hand-annotated).

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

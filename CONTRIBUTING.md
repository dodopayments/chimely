# Contributing to Chimely

Thanks for wanting to help. Two things to know before your first PR:

- **CLA.** External code contributions require signing a Contributor License
  Agreement, so the project keeps relicensing and commercial-licensing
  flexibility. You will be asked on your first PR.
- **Invariants.** `CLAUDE.md` at the repo root is the living record of the
  design invariants (two-source inbox, watermark-only bulk operations,
  transactional outbox, and friends). Violating one is a bug even if all
  tests pass. Read it before touching the server.

## Prerequisites

- Rust (the toolchain is pinned by `rust-toolchain.toml`; rustup picks it up)
- cargo-nextest, the test runner (`cargo install cargo-nextest`)
- Node 20+ with pnpm (`corepack enable`)
- Docker (server tests run against real Postgres and Redis via
  testcontainers)

## Server (`server/`, Rust)

Builds are offline by default: the committed `.sqlx` cache carries the
compile-time query metadata, so no database is needed to compile.

```bash
cd server
SQLX_OFFLINE=true cargo fmt --check
SQLX_OFFLINE=true cargo clippy --all-targets -- -D warnings
SQLX_OFFLINE=true cargo nextest run        # needs Docker running
```

Run the server from source against a throwaway Postgres (from the repo
root):

```bash
docker run -d --name chimely-dev-pg \
  -e POSTGRES_PASSWORD=chimely -p 5432:5432 postgres:16-alpine

SQLX_OFFLINE=true \
DATABASE_URL=postgres://postgres:chimely@localhost:5432/postgres \
CHIMELY_DEV_ENVIRONMENT=demo \
CHIMELY_DEV_API_KEY=dev-secret-key \
cargo run --manifest-path server/Cargo.toml -- serve
```

`SQLX_OFFLINE=true` matters: without it the sqlx macros check queries against
the live `DATABASE_URL`, which is still empty on a first run (migrations only
apply when the compiled binary boots).

**Changing or adding SQL queries:** point `DATABASE_URL` at a Postgres that
has the migrations applied (boot the server against it once), run
`cargo sqlx prepare`, and commit the `.sqlx` changes. CI compiles offline and
fails on a stale cache.

The all-in-one stack (server + Postgres + Redis, built from your checkout) is
`docker compose up --build` at the repo root.

## TypeScript (`packages/`, `docs/`, `examples/`)

```bash
pnpm install
pnpm lint                                # biome ci .
pnpm --filter "./packages/*" build       # react typechecks against client's dist
pnpm typecheck
pnpm test
```

`@chimely/react` tests run against `@chimely/client`'s built `dist`, so
rebuild the client after switching branches or its features will be missing.

Package-visible changes need a changeset (`pnpm changeset`); pick `minor` for
features and `patch` for fixes, pre-1.0.

## Generated artifacts

The OpenAPI document is code-first (utoipa). `docs/openapi/` and
`packages/client/src/generated/` are derived from it:

```bash
pnpm generate    # regenerates both; commit the result
```

Never hand-edit generated files. A stale artifact shows up as an uncommitted
diff after `pnpm generate`; regenerate and commit it with the change that
moved the spec.

## Commits and PRs

Conventional Commits (`type(scope): summary`), subjects at or under about 50
characters, and concise bodies. `feat`, `fix`, `refactor`, `docs`, `test`,
`build`, `ci`, `chore` are the common types. PR descriptions state what
changed and why in as few words as possible.

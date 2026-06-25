# syntax=docker/dockerfile:1
# Multi-stage build. A Node stage builds the embedded admin SPA
# (server/admin -> server/admin/dist); the cargo-chef Rust stages then embed
# that bundle into the single binary via rust-embed. The image stays one file:
# `docker run chimely` ships the dashboard with no extra artifact.

# --- Admin SPA: server/admin -> server/admin/dist ---
FROM node:24-slim AS admin
RUN corepack enable && corepack prepare pnpm@11.5.2 --activate
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
WORKDIR /app
# The whole workspace is needed so pnpm can resolve the committed lockfile;
# the focused install pulls only the admin SPA's subtree.
COPY . .
RUN pnpm install --frozen-lockfile --filter chimely-admin...
RUN pnpm --filter chimely-admin build

# --- Rust build (cargo-chef: dependency layers cached independently of src) ---
FROM lukemathwalker/cargo-chef:latest-rust-1.96.0 AS chef
WORKDIR /app

FROM chef AS planner
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/build.rs ./build.rs
COPY server/src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/build.rs ./build.rs
COPY server/src ./src
# sqlx compile-time checks read the committed offline cache (no database in
# the image build); migrations/ is embedded by sqlx::migrate! at compile time.
COPY server/.sqlx ./.sqlx
COPY server/migrations ./migrations
# The built admin SPA, embedded by rust-embed (api::admin). build.rs leaves a
# populated admin/dist untouched (it only writes a placeholder when missing).
COPY --from=admin /app/server/admin/dist ./admin/dist
ENV SQLX_OFFLINE=true
RUN cargo build --release --bin chimely

FROM debian:trixie-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group chimely
COPY --from=builder /app/target/release/chimely /usr/local/bin/chimely
USER chimely
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/chimely"]
CMD ["serve"]

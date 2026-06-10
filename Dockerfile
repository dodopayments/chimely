# syntax=docker/dockerfile:1
# cargo-chef multi-stage build: dependency layers are cached independently of
# src/ changes. Builder rust version matches rust-toolchain.toml.
FROM lukemathwalker/cargo-chef:latest-rust-1.96.0 AS chef
WORKDIR /app

FROM chef AS planner
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/src ./src
# sqlx compile-time checks read the committed offline cache (no database in
# the image build); migrations/ is embedded by sqlx::migrate! at compile time.
COPY server/.sqlx ./.sqlx
COPY server/migrations ./migrations
ENV SQLX_OFFLINE=true
RUN cargo build --release --bin dronte

FROM debian:trixie-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group dronte
COPY --from=builder /app/target/release/dronte /usr/local/bin/dronte
USER dronte
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/dronte"]
CMD ["serve"]

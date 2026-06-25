//! Pool construction and boot-time migrations.
//!
//! Migrations are embedded (`sqlx::migrate!`) and run on boot. sqlx's
//! Postgres migrator takes a database-scoped advisory lock for the whole run.
//! N replicas racing on a fresh deploy apply each migration exactly once. The
//! losers block until the winner commits, then no-op.

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(
            std::env::var("DRONTE_PG_POOL_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(16),
        )
        .connect(database_url)
        .await?)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

/// Readiness probe. Postgres reachable and every embedded migration applied,
/// checked in one round-trip. Redis is the hint/cache plane and is
/// deliberately not consulted.
pub async fn ready(pool: &PgPool) -> anyhow::Result<bool> {
    let latest = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
    let applied: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version = $1 AND success)",
    )
    .bind(latest)
    .fetch_one(pool)
    .await?;
    Ok(applied)
}

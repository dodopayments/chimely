//! Runtime configuration, sourced from the environment. Every knob has a
//! production default; tests construct `Config` directly with tight timings.

use std::time::Duration;

use anyhow::Context;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    /// `None` = Redis-less mode: hints ride Postgres LISTEN/NOTIFY instead of
    /// Redis pub/sub (see `pubsub`). Redis is the hint/cache plane only. Its
    /// absence degrades hint latency and nothing else.
    pub redis_url: Option<String>,
    pub listen_addr: String,
    /// Months of notification partitions kept behind `now`. Older partitions
    /// are detached and dropped by the maintenance job.
    pub retention_months: u32,
    /// Days idempotency snapshots are kept (comfortably longer than any sane
    /// client retry horizon).
    pub idempotency_retention_days: u32,
    /// Hint debounce window: at most one published hint per subscriber per
    /// window.
    pub hint_debounce: Duration,
    pub worker_poll_interval: Duration,
    pub sse_ping_interval: Duration,
    /// Base for the graceful-shutdown `retry:` directive. Per-connection
    /// jitter in `[0, sse_retry_jitter)` is added so a deploy does not
    /// produce a reconnect stampede.
    pub sse_retry_base: Duration,
    pub sse_retry_jitter: Duration,
    /// Per-subscriber SSE connection cap, per replica (risk M3: dev
    /// environments without subscriber hashes are otherwise an open
    /// connection-exhaustion relay).
    pub sse_max_connections_per_subscriber: usize,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            database_url: std::env::var("DATABASE_URL")
                .context("DATABASE_URL is required (postgres://...)")?,
            redis_url: std::env::var("REDIS_URL").ok().filter(|s| !s.is_empty()),
            listen_addr: var_or("DRONTE_LISTEN_ADDR", "0.0.0.0:8080"),
            retention_months: parse_var("DRONTE_RETENTION_MONTHS", 12)?,
            idempotency_retention_days: parse_var("DRONTE_IDEMPOTENCY_RETENTION_DAYS", 30)?,
            hint_debounce: Duration::from_millis(parse_var("DRONTE_HINT_DEBOUNCE_MS", 1_000)?),
            worker_poll_interval: Duration::from_millis(parse_var("DRONTE_WORKER_POLL_MS", 250)?),
            sse_ping_interval: Duration::from_secs(parse_var("DRONTE_SSE_PING_SECS", 30)?),
            sse_retry_base: Duration::from_millis(parse_var("DRONTE_SSE_RETRY_BASE_MS", 2_000)?),
            sse_retry_jitter: Duration::from_millis(parse_var(
                "DRONTE_SSE_RETRY_JITTER_MS",
                8_000,
            )?),
            sse_max_connections_per_subscriber: parse_var(
                "DRONTE_SSE_MAX_CONNS_PER_SUBSCRIBER",
                8,
            )?,
        })
    }
}

fn var_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn parse_var<T: std::str::FromStr>(name: &str, default: T) -> anyhow::Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match std::env::var(name) {
        Ok(v) => v.parse().with_context(|| format!("parsing {name}={v}")),
        Err(_) => Ok(default),
    }
}

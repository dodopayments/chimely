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
    /// Dev-quickstart bootstrap (see `bootstrap`). When set, boot upserts an
    /// environment with this slug and require_subscriber_hash = false.
    /// Real environment management is the Phase 4 admin UI. Never set this
    /// in production.
    pub dev_environment: Option<String>,
    /// Plaintext management API key upserted into the dev environment, so
    /// the quickstart curl is copy-pasteable. Ignored without
    /// `dev_environment`.
    pub dev_api_key: Option<String>,
    /// First retry delay for a failed job. Attempt n waits roughly
    /// `base * 2^(n-1)`, equal-jittered, capped below.
    pub retry_backoff_base: Duration,
    /// Ceiling for the exponential retry delay.
    pub retry_backoff_cap: Duration,
    /// Cadence of the metrics sampler (queue depth, counter drift, dead
    /// letters, partition headroom).
    pub metrics_sample_interval: Duration,
    /// Subscribers recounted per drift sample, most recently active first.
    pub counter_drift_sample_size: i64,
    /// Management-plane token bucket, per API key. Rate is tokens added per
    /// second; burst is the bucket capacity. Zero rate disables the limit.
    pub api_key_rate_per_sec: f64,
    pub api_key_rate_burst: f64,
    /// Subscriber-plane token bucket, per subscriber. Zero rate disables.
    pub subscriber_rate_per_sec: f64,
    pub subscriber_rate_burst: f64,
    /// How long /readyz reports 503 before the listener actually closes, so
    /// load balancers drain the replica first.
    pub shutdown_readiness_grace: Duration,
    /// In-flight job drain budget after claiming stops. Past it the worker
    /// is aborted; at-least-once semantics make the rollback safe.
    pub shutdown_drain_deadline: Duration,
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
            dev_environment: std::env::var("DRONTE_DEV_ENVIRONMENT")
                .ok()
                .filter(|s| !s.is_empty()),
            dev_api_key: std::env::var("DRONTE_DEV_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            retry_backoff_base: Duration::from_millis(parse_var(
                "DRONTE_RETRY_BACKOFF_BASE_MS",
                5_000,
            )?),
            retry_backoff_cap: Duration::from_millis(parse_var(
                "DRONTE_RETRY_BACKOFF_CAP_MS",
                900_000,
            )?),
            metrics_sample_interval: Duration::from_millis(parse_var(
                "DRONTE_METRICS_SAMPLE_MS",
                15_000,
            )?),
            counter_drift_sample_size: parse_var("DRONTE_COUNTER_DRIFT_SAMPLE_SIZE", 50)?,
            api_key_rate_per_sec: parse_var("DRONTE_API_KEY_RATE_PER_SEC", 50.0)?,
            api_key_rate_burst: parse_var("DRONTE_API_KEY_RATE_BURST", 200.0)?,
            subscriber_rate_per_sec: parse_var("DRONTE_SUBSCRIBER_RATE_PER_SEC", 10.0)?,
            subscriber_rate_burst: parse_var("DRONTE_SUBSCRIBER_RATE_BURST", 50.0)?,
            shutdown_readiness_grace: Duration::from_millis(parse_var(
                "DRONTE_SHUTDOWN_GRACE_MS",
                5_000,
            )?),
            shutdown_drain_deadline: Duration::from_millis(parse_var(
                "DRONTE_SHUTDOWN_DRAIN_DEADLINE_MS",
                30_000,
            )?),
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

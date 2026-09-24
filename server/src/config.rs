//! Runtime configuration sourced from the environment. Every knob has a
//! production default. Tests construct `Config` directly with tight timings.

use std::time::Duration;

use anyhow::Context;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    /// `None` is Redis-less mode. Hints ride Postgres LISTEN/NOTIFY instead of
    /// Redis pub/sub. Redis is the hint/cache plane only. Its absence degrades
    /// hint latency and nothing else.
    pub redis_url: Option<String>,
    pub listen_addr: String,
    /// Months of notification partitions kept behind `now`. Older partitions
    /// are detached and dropped by the maintenance job.
    pub retention_months: u32,
    /// Days idempotency snapshots are kept. Longer than any client retry
    /// horizon.
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
    /// Per-subscriber SSE connection cap, per replica. Without it, dev
    /// environments lacking subscriber hashes are an open
    /// connection-exhaustion relay.
    pub sse_max_connections_per_subscriber: usize,
    /// Dev-quickstart bootstrap. When set, boot upserts an environment with
    /// this slug and require_subscriber_hash = false. Real environment
    /// management is the admin UI. Never set this in production.
    pub dev_environment: Option<String>,
    /// Plaintext management API key upserted into the dev environment, so
    /// the quickstart curl is copy-pasteable. Ignored without
    /// `dev_environment`.
    pub dev_api_key: Option<String>,
    /// Bootstrap (root) admin account, ensured at boot when both are set.
    /// The lockout-recovery path. Restart with these env vars to restore
    /// admin access. Humans get their own UI-created accounts. The password
    /// is never logged or echoed.
    pub admin_bootstrap_email: Option<String>,
    pub admin_bootstrap_password: Option<String>,
    /// Admin session lifetime. `expires_at` is stamped this far ahead at
    /// login. The maintenance job GCs rows past it.
    pub admin_session_ttl: Duration,
    /// Operator acknowledgement that TLS terminates in front of the binary
    /// (the binary serves plain HTTP). Gates the `Secure` cookie attribute
    /// and silences the boot-time TLS warning. The admin session cookie
    /// REQUIRES TLS in production. Unset, boot warns loudly.
    pub admin_tls_terminated: bool,
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
    /// second. Burst is the bucket capacity. Zero rate disables the limit.
    pub api_key_rate_per_sec: f64,
    pub api_key_rate_burst: f64,
    /// Subscriber-plane token bucket, per subscriber. Zero rate disables.
    pub subscriber_rate_per_sec: f64,
    pub subscriber_rate_burst: f64,
    /// How long /readyz reports 503 before the listener actually closes, so
    /// load balancers drain the replica first.
    pub shutdown_readiness_grace: Duration,
    /// In-flight job drain budget after claiming stops. Past it the worker
    /// is aborted. At-least-once semantics make the rollback safe.
    pub shutdown_drain_deadline: Duration,
    /// Accept the legacy subscriber hash form that omits the environment
    /// binding. On by default so deployed customer backends keep working.
    /// Flip to false once every backend computes the environment-bound form.
    pub subscriber_hash_legacy_accept: bool,
}

/// Manual impl so credentials can never reach logs through `{:?}`.
/// `database_url` and `redis_url` carry DSN passwords, `dev_api_key` is a
/// plaintext management key, `admin_bootstrap_password` is a plaintext
/// password. Presence is kept visible for Redis-less and bootstrap
/// diagnosis.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn redact<T>(value: &Option<T>) -> &'static str {
            match value {
                Some(_) => "Some([redacted])",
                None => "None",
            }
        }
        f.debug_struct("Config")
            .field("database_url", &"[redacted]")
            .field("redis_url", &redact(&self.redis_url))
            .field("listen_addr", &self.listen_addr)
            .field("retention_months", &self.retention_months)
            .field(
                "idempotency_retention_days",
                &self.idempotency_retention_days,
            )
            .field("hint_debounce", &self.hint_debounce)
            .field("worker_poll_interval", &self.worker_poll_interval)
            .field("sse_ping_interval", &self.sse_ping_interval)
            .field("sse_retry_base", &self.sse_retry_base)
            .field("sse_retry_jitter", &self.sse_retry_jitter)
            .field(
                "sse_max_connections_per_subscriber",
                &self.sse_max_connections_per_subscriber,
            )
            .field("dev_environment", &self.dev_environment)
            .field("dev_api_key", &redact(&self.dev_api_key))
            .field("admin_bootstrap_email", &self.admin_bootstrap_email)
            .field(
                "admin_bootstrap_password",
                &redact(&self.admin_bootstrap_password),
            )
            .field("admin_session_ttl", &self.admin_session_ttl)
            .field("admin_tls_terminated", &self.admin_tls_terminated)
            .field("retry_backoff_base", &self.retry_backoff_base)
            .field("retry_backoff_cap", &self.retry_backoff_cap)
            .field("metrics_sample_interval", &self.metrics_sample_interval)
            .field("counter_drift_sample_size", &self.counter_drift_sample_size)
            .field("api_key_rate_per_sec", &self.api_key_rate_per_sec)
            .field("api_key_rate_burst", &self.api_key_rate_burst)
            .field("subscriber_rate_per_sec", &self.subscriber_rate_per_sec)
            .field("subscriber_rate_burst", &self.subscriber_rate_burst)
            .field("shutdown_readiness_grace", &self.shutdown_readiness_grace)
            .field("shutdown_drain_deadline", &self.shutdown_drain_deadline)
            .field(
                "subscriber_hash_legacy_accept",
                &self.subscriber_hash_legacy_accept,
            )
            .finish()
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            database_url: std::env::var("DATABASE_URL")
                .context("DATABASE_URL is required (postgres://...)")?,
            redis_url: std::env::var("REDIS_URL").ok().filter(|s| !s.is_empty()),
            listen_addr: var_or("CHIMELY_LISTEN_ADDR", "0.0.0.0:8080"),
            retention_months: parse_var("CHIMELY_RETENTION_MONTHS", 12)?,
            idempotency_retention_days: parse_var("CHIMELY_IDEMPOTENCY_RETENTION_DAYS", 30)?,
            hint_debounce: Duration::from_millis(parse_var("CHIMELY_HINT_DEBOUNCE_MS", 1_000)?),
            worker_poll_interval: Duration::from_millis(parse_var("CHIMELY_WORKER_POLL_MS", 250)?),
            sse_ping_interval: Duration::from_secs(parse_var("CHIMELY_SSE_PING_SECS", 30)?),
            sse_retry_base: Duration::from_millis(parse_var("CHIMELY_SSE_RETRY_BASE_MS", 2_000)?),
            sse_retry_jitter: Duration::from_millis(parse_var(
                "CHIMELY_SSE_RETRY_JITTER_MS",
                8_000,
            )?),
            sse_max_connections_per_subscriber: parse_var(
                "CHIMELY_SSE_MAX_CONNS_PER_SUBSCRIBER",
                8,
            )?,
            dev_environment: std::env::var("CHIMELY_DEV_ENVIRONMENT")
                .ok()
                .filter(|s| !s.is_empty()),
            dev_api_key: std::env::var("CHIMELY_DEV_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            admin_bootstrap_email: std::env::var("CHIMELY_ADMIN_EMAIL")
                .ok()
                .filter(|s| !s.is_empty()),
            admin_bootstrap_password: std::env::var("CHIMELY_ADMIN_PASSWORD")
                .ok()
                .filter(|s| !s.is_empty()),
            admin_session_ttl: Duration::from_secs(parse_var(
                "CHIMELY_ADMIN_SESSION_TTL_SECS",
                28_800,
            )?),
            admin_tls_terminated: parse_var("CHIMELY_ADMIN_TLS_TERMINATED", false)?,
            retry_backoff_base: Duration::from_millis(parse_var(
                "CHIMELY_RETRY_BACKOFF_BASE_MS",
                5_000,
            )?),
            retry_backoff_cap: Duration::from_millis(parse_var(
                "CHIMELY_RETRY_BACKOFF_CAP_MS",
                900_000,
            )?),
            metrics_sample_interval: Duration::from_millis(parse_var(
                "CHIMELY_METRICS_SAMPLE_MS",
                15_000,
            )?),
            counter_drift_sample_size: parse_var("CHIMELY_COUNTER_DRIFT_SAMPLE_SIZE", 50)?,
            api_key_rate_per_sec: parse_var("CHIMELY_API_KEY_RATE_PER_SEC", 50.0)?,
            api_key_rate_burst: parse_var("CHIMELY_API_KEY_RATE_BURST", 200.0)?,
            subscriber_rate_per_sec: parse_var("CHIMELY_SUBSCRIBER_RATE_PER_SEC", 10.0)?,
            subscriber_rate_burst: parse_var("CHIMELY_SUBSCRIBER_RATE_BURST", 50.0)?,
            shutdown_readiness_grace: Duration::from_millis(parse_var(
                "CHIMELY_SHUTDOWN_GRACE_MS",
                5_000,
            )?),
            shutdown_drain_deadline: Duration::from_millis(parse_var(
                "CHIMELY_SHUTDOWN_DRAIN_DEADLINE_MS",
                30_000,
            )?),
            subscriber_hash_legacy_accept: parse_var(
                "CHIMELY_SUBSCRIBER_HASH_LEGACY_ACCEPT",
                true,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_redacts_credentials() {
        let cfg = Config {
            database_url: "postgres://user:s3cret-db-pw@host/db".into(),
            redis_url: Some("redis://:s3cret-redis-pw@host".into()),
            listen_addr: "127.0.0.1:0".into(),
            retention_months: 12,
            idempotency_retention_days: 30,
            hint_debounce: Duration::from_millis(1),
            worker_poll_interval: Duration::from_millis(1),
            sse_ping_interval: Duration::from_secs(1),
            sse_retry_base: Duration::from_millis(1),
            sse_retry_jitter: Duration::from_millis(1),
            sse_max_connections_per_subscriber: 8,
            dev_environment: Some("demo".into()),
            dev_api_key: Some("s3cret-api-key".into()),
            admin_bootstrap_email: Some("ops@example.com".into()),
            admin_bootstrap_password: Some("s3cret-admin-pw".into()),
            admin_session_ttl: Duration::from_secs(1),
            admin_tls_terminated: false,
            retry_backoff_base: Duration::from_millis(1),
            retry_backoff_cap: Duration::from_millis(1),
            metrics_sample_interval: Duration::from_millis(1),
            counter_drift_sample_size: 1,
            api_key_rate_per_sec: 0.0,
            api_key_rate_burst: 0.0,
            subscriber_rate_per_sec: 0.0,
            subscriber_rate_burst: 0.0,
            shutdown_readiness_grace: Duration::from_millis(1),
            shutdown_drain_deadline: Duration::from_millis(1),
            subscriber_hash_legacy_accept: true,
        };
        let out = format!("{cfg:?}");
        assert!(
            !out.contains("s3cret"),
            "credential leaked into Debug: {out}"
        );
        assert!(out.contains("[redacted]"));
        assert!(out.contains("listen_addr"));
        assert!(out.contains("ops@example.com"));
    }
}

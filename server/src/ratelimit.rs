//! Token bucket per key, evaluated atomically in Redis Lua so N replicas
//! share ONE bucket. The script reads the Redis clock via TIME, so replica
//! clock skew cannot double-fill a bucket.
//!
//! Redis is the hint/cache plane. A Redis outage must never take the API
//! down, so the limiter FAILS OPEN on Redis errors. In Redis-less mode an
//! in-process bucket applies. That degrades to per-replica buckets (more
//! permissive, never stricter) when several replicas run without Redis.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fred::interfaces::ClientLike;

/// `retry_after` is the time until a single token is available again,
/// surfaced as `Retry-After`.
pub enum Decision {
    Allowed,
    Limited { retry_after: Duration },
}

#[async_trait::async_trait]
pub trait RateLimiter: Send + Sync {
    /// Take one token from `key`'s bucket (capacity `burst`, refilling at
    /// `rate_per_sec`). `rate_per_sec <= 0` disables the limit.
    async fn check(&self, key: &str, rate_per_sec: f64, burst: f64) -> Decision;
}

pub async fn build(redis_url: Option<&str>) -> anyhow::Result<Arc<dyn RateLimiter>> {
    match redis_url {
        Some(url) => Ok(Arc::new(RedisRateLimiter::connect(url).await?)),
        None => Ok(Arc::new(LocalRateLimiter::default())),
    }
}

/// Take one token or return 429 with `Retry-After`.
pub async fn enforce(
    limiter: &dyn RateLimiter,
    key: &str,
    rate_per_sec: f64,
    burst: f64,
) -> Result<(), crate::error::ApiError> {
    match limiter.check(key, rate_per_sec, burst).await {
        Decision::Allowed => Ok(()),
        Decision::Limited { retry_after } => {
            metrics::counter!("dronte_rate_limited_total").increment(1);
            Err(crate::error::ApiError::rate_limited(retry_after))
        }
    }
}

// =============================================================================
// Redis (fred), the cross-replica implementation
// =============================================================================

/// Atomic refill-and-take. State is a hash {tokens, ts}. The clock is Redis
/// TIME, one clock for all replicas. Redis >= 5 replicates script EFFECTS, so
/// the non-determinism is safe. Returns {allowed, retry_after_seconds}. The
/// key expires once a full bucket's refill has elapsed twice. An idle bucket
/// reappears full, the steady state.
const TOKEN_BUCKET_LUA: &str = r#"
local rate = tonumber(ARGV[1])
local burst = tonumber(ARGV[2])
local t = redis.call('TIME')
local now = tonumber(t[1]) + tonumber(t[2]) / 1e6
local state = redis.call('HMGET', KEYS[1], 'tokens', 'ts')
local tokens = tonumber(state[1])
local ts = tonumber(state[2])
if tokens == nil or ts == nil then
  tokens = burst
  ts = now
end
tokens = math.min(burst, tokens + (now - ts) * rate)
local allowed = 0
local retry = 0
if tokens >= 1 then
  tokens = tokens - 1
  allowed = 1
else
  retry = (1 - tokens) / rate
end
redis.call('HSET', KEYS[1], 'tokens', tostring(tokens), 'ts', tostring(now))
redis.call('PEXPIRE', KEYS[1], math.ceil(burst / rate * 2000))
return {allowed, tostring(retry)}
"#;

pub struct RedisRateLimiter {
    client: fred::clients::Client,
    script: fred::types::scripts::Script,
}

impl RedisRateLimiter {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let config = fred::types::config::Config::from_url(url)?;
        let mut builder = fred::types::Builder::from_config(config);
        // Reconnect forever, but never hang a request on a dead Redis. Fail
        // open instead.
        builder
            .set_policy(fred::types::config::ReconnectPolicy::new_exponential(
                0, 100, 10_000, 2,
            ))
            .with_performance_config(|perf| {
                perf.default_command_timeout = Duration::from_secs(2);
            })
            .with_connection_config(|conn| {
                conn.connection_timeout = Duration::from_secs(2);
                conn.internal_command_timeout = Duration::from_secs(2);
            });
        let client = builder.build()?;
        client.init().await?;
        Ok(Self {
            client,
            script: fred::types::scripts::Script::from_lua(TOKEN_BUCKET_LUA),
        })
    }
}

#[async_trait::async_trait]
impl RateLimiter for RedisRateLimiter {
    async fn check(&self, key: &str, rate_per_sec: f64, burst: f64) -> Decision {
        if rate_per_sec <= 0.0 {
            return Decision::Allowed;
        }
        let result: Result<(i64, String), _> = self
            .script
            .evalsha_with_reload(
                &self.client,
                vec![format!("dronte:rl:{key}")],
                vec![rate_per_sec.to_string(), burst.to_string()],
            )
            .await;
        match result {
            Ok((1, _)) => Decision::Allowed,
            Ok((_, retry)) => Decision::Limited {
                retry_after: Duration::from_secs_f64(retry.parse::<f64>().unwrap_or(1.0).max(0.0)),
            },
            // Fail OPEN. Redis loss may loosen limits. It must never reject
            // traffic Postgres could serve.
            Err(err) => {
                metrics::counter!("dronte_rate_limit_errors_total").increment(1);
                tracing::warn!(error = ?err, "rate limiter unavailable; failing open");
                Decision::Allowed
            }
        }
    }
}

// =============================================================================
// In-process fallback (Redis-less mode), single-node semantics
// =============================================================================

/// Each bucket carries its OWN rate and burst. API-key and subscriber buckets
/// share this map with different parameters, so the GC must evaluate each
/// bucket's refill against the values it was created with, never the current
/// caller's.
#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    sampled_at: Instant,
    rate_per_sec: f64,
    burst: f64,
}

#[derive(Default)]
pub struct LocalRateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
}

#[async_trait::async_trait]
impl RateLimiter for LocalRateLimiter {
    async fn check(&self, key: &str, rate_per_sec: f64, burst: f64) -> Decision {
        if rate_per_sec <= 0.0 {
            return Decision::Allowed;
        }
        let mut buckets = self.buckets.lock().expect("rate limiter lock");
        let now = Instant::now();
        // Opportunistic GC. Drop buckets that have fully refilled per their
        // own parameters. A dropped bucket reappears full, the steady state.
        if buckets.len() > 10_000 {
            buckets.retain(|_, b| {
                b.tokens + now.duration_since(b.sampled_at).as_secs_f64() * b.rate_per_sec < b.burst
            });
        }
        let bucket = *buckets.entry(key.to_owned()).or_insert(Bucket {
            tokens: burst,
            sampled_at: now,
            rate_per_sec,
            burst,
        });
        let tokens = burst.min(
            bucket.tokens + now.duration_since(bucket.sampled_at).as_secs_f64() * rate_per_sec,
        );
        let (tokens, decision) = if tokens >= 1.0 {
            (tokens - 1.0, Decision::Allowed)
        } else {
            (
                tokens,
                Decision::Limited {
                    retry_after: Duration::from_secs_f64((1.0 - tokens) / rate_per_sec),
                },
            )
        };
        buckets.insert(
            key.to_owned(),
            Bucket {
                tokens,
                sampled_at: now,
                rate_per_sec,
                burst,
            },
        );
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_bucket_enforces_burst_then_refills() {
        let limiter = LocalRateLimiter::default();
        for _ in 0..3 {
            assert!(matches!(
                limiter.check("k", 1000.0, 3.0).await,
                Decision::Allowed
            ));
        }
        let Decision::Limited { retry_after } = limiter.check("k", 1000.0, 3.0).await else {
            panic!("burst exhausted; must limit");
        };
        assert!(retry_after <= Duration::from_millis(2));
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(matches!(
            limiter.check("k", 1000.0, 3.0).await,
            Decision::Allowed
        ));
    }

    #[tokio::test]
    async fn gc_judges_each_bucket_by_its_own_parameters() {
        let limiter = LocalRateLimiter::default();
        // A spent API-key-shaped bucket that refills so slowly (0.01/s) it
        // cannot return to full during the test. Its own parameters say "keep".
        for _ in 0..60 {
            assert!(matches!(
                limiter.check("api-key", 0.01, 200.0).await,
                Decision::Allowed
            ));
        }
        // Flood subscriber-shaped buckets past the GC threshold so the NEXT
        // subscriber check (rate=10, burst=50) runs the GC. With the caller's
        // parameters wrongly applied to every bucket, the API-key bucket (140
        // tokens >= burst 50) would be dropped and reborn full.
        for i in 0..10_001 {
            limiter.check(&format!("sub-{i}"), 10.0, 50.0).await;
        }
        let buckets = limiter.buckets.lock().expect("lock");
        let bucket = buckets.get("api-key").expect("survives the GC");
        assert!(
            bucket.tokens < 141.0,
            "bucket must keep its spent state, not be reborn full"
        );
    }

    #[tokio::test]
    async fn zero_rate_disables_the_limit() {
        let limiter = LocalRateLimiter::default();
        for _ in 0..100 {
            assert!(matches!(
                limiter.check("k", 0.0, 0.0).await,
                Decision::Allowed
            ));
        }
    }
}

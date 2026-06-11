//! Rate limiting: a token bucket per key, evaluated atomically in Redis Lua
//! so N replicas share ONE bucket (cross-replica correctness — the script
//! reads the Redis clock via TIME, so replica clock skew cannot double-fill
//! a bucket).
//!
//! Redis is the hint/cache plane: a Redis outage must never take the API
//! down, so the limiter FAILS OPEN on Redis errors (logged + counted). In
//! Redis-less mode an in-process bucket applies — correct for the
//! single-binary dev path, per-replica (more permissive, never stricter)
//! when several replicas run without Redis.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fred::interfaces::ClientLike;

/// One bucket decision. `retry_after` is the time until a single token is
/// available again, which the contract surfaces as `Retry-After`.
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

/// Take one token or answer the contract's 429 (`Retry-After` included).
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
// Redis (fred) — the cross-replica implementation
// =============================================================================

/// Atomic refill-and-take. State is a hash {tokens, ts}; the clock is Redis
/// TIME (one clock for all replicas; Redis >= 5 replicates script EFFECTS,
/// so the non-determinism is safe). Returns {allowed, retry_after_seconds}.
/// The key expires once a full bucket's refill has elapsed twice — an idle
/// bucket reappears full, which is exactly the steady state.
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
        // Same resilience posture as the pub/sub clients: reconnect forever,
        // but never hang a request on a dead Redis (fail open instead).
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
            // Fail OPEN: Redis loss may delay hints and loosen limits; it
            // must never reject traffic Postgres could serve.
            Err(err) => {
                metrics::counter!("dronte_rate_limit_errors_total").increment(1);
                tracing::warn!(error = ?err, "rate limiter unavailable; failing open");
                Decision::Allowed
            }
        }
    }
}

// =============================================================================
// In-process fallback (Redis-less mode) — single-node semantics
// =============================================================================

#[derive(Default)]
pub struct LocalRateLimiter {
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

#[async_trait::async_trait]
impl RateLimiter for LocalRateLimiter {
    async fn check(&self, key: &str, rate_per_sec: f64, burst: f64) -> Decision {
        if rate_per_sec <= 0.0 {
            return Decision::Allowed;
        }
        let mut buckets = self.buckets.lock().expect("rate limiter lock");
        let now = Instant::now();
        // Opportunistic GC: drop buckets that have fully refilled.
        if buckets.len() > 10_000 {
            buckets.retain(|_, (tokens, ts)| {
                *tokens + now.duration_since(*ts).as_secs_f64() * rate_per_sec < burst
            });
        }
        let (tokens, ts) = buckets
            .entry(key.to_owned())
            .or_insert((burst, now))
            .to_owned();
        let tokens = burst.min(tokens + now.duration_since(ts).as_secs_f64() * rate_per_sec);
        if tokens >= 1.0 {
            buckets.insert(key.to_owned(), (tokens - 1.0, now));
            Decision::Allowed
        } else {
            buckets.insert(key.to_owned(), (tokens, now));
            Decision::Limited {
                retry_after: Duration::from_secs_f64((1.0 - tokens) / rate_per_sec),
            }
        }
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

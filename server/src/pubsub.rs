//! The hint plane. Redis pub/sub (fred) when `REDIS_URL` is set, Postgres
//! LISTEN/NOTIFY otherwise (`npx dronte dev` Redis-less mode).
//!
//! Hints are refetch triggers, not transports (CLAUDE.md): losing this plane
//! delays hints and loses nothing — the jobs table rows survive and retry.
//! Every replica subscribes once to a single channel and fans in to local SSE
//! connections through a tokio broadcast channel.
//!
//! **PgBouncer warning (document loudly, per the plan):** the LISTEN/NOTIFY
//! fallback opens a DEDICATED DIRECT connection to `DATABASE_URL`.
//! Transaction-mode PgBouncer breaks LISTEN — in Redis-less mode the database
//! URL must point at Postgres directly (or a session-mode pooler).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fred::interfaces::{ClientLike, EventInterface, KeysInterface, PubsubInterface};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Redis channel / Postgres NOTIFY channel carrying hint envelopes.
const HINT_CHANNEL: &str = "dronte_hints";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hint {
    pub environment_id: Uuid,
    /// `None` targets every subscriber in the environment (the broadcast
    /// case, one message regardless of subscriber count).
    pub subscriber_id: Option<Uuid>,
    pub reason: String,
}

#[async_trait::async_trait]
pub trait PubSub: Send + Sync {
    async fn publish(&self, hint: &Hint) -> anyhow::Result<()>;

    /// Local fan-in. Every hint published by ANY replica is delivered to
    /// every receiver on every replica. SSE connections filter by
    /// (env, subscriber).
    fn subscribe(&self) -> broadcast::Receiver<Hint>;

    /// Debounce slot acquisition. True means the caller may publish now and
    /// owns the window. False means a hint for this key was already published
    /// within the window and the caller defers to the window end, so the
    /// last change is never silently swallowed.
    async fn try_acquire_debounce(&self, key: &str, window: Duration) -> anyhow::Result<bool>;
}

pub async fn build(redis_url: Option<&str>, pool: &PgPool) -> anyhow::Result<Arc<dyn PubSub>> {
    match redis_url {
        Some(url) => Ok(Arc::new(RedisPubSub::connect(url).await?)),
        None => Ok(Arc::new(PgPubSub::connect(pool).await?)),
    }
}

// =============================================================================
// Redis (fred)
// =============================================================================

pub struct RedisPubSub {
    client: fred::clients::Client,
    tx: broadcast::Sender<Hint>,
}

/// Without a command timeout fred buffers commands across reconnects
/// indefinitely. A hanging Redis would then stall the worker inside its claim
/// transaction and freeze deliver/counter_rebuild for every environment. With
/// the timeout an outage surfaces as a job error, backs off via fail_job, and
/// the worker stays live (hints delayed, nothing else).
const REDIS_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

impl RedisPubSub {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let config = fred::types::config::Config::from_url(url)?;
        let mut builder = fred::types::Builder::from_config(config.clone());
        builder
            .set_policy(fred::types::config::ReconnectPolicy::new_exponential(
                0, 100, 10_000, 2,
            ))
            .with_performance_config(|perf| {
                perf.default_command_timeout = REDIS_COMMAND_TIMEOUT;
            })
            .with_connection_config(|conn| {
                conn.connection_timeout = REDIS_COMMAND_TIMEOUT;
                conn.internal_command_timeout = REDIS_COMMAND_TIMEOUT;
            });
        let client = builder.build()?;
        client.init().await?;

        let mut sub_builder = fred::types::Builder::from_config(config);
        sub_builder
            .set_policy(fred::types::config::ReconnectPolicy::new_exponential(
                0, 100, 10_000, 2,
            ))
            .with_connection_config(|conn| {
                conn.connection_timeout = REDIS_COMMAND_TIMEOUT;
                conn.internal_command_timeout = REDIS_COMMAND_TIMEOUT;
            });
        let subscriber = sub_builder.build_subscriber_client()?;
        subscriber.init().await?;
        // Re-subscribes after every reconnect.
        subscriber.manage_subscriptions();
        subscriber.subscribe(HINT_CHANNEL).await?;

        let (tx, _) = broadcast::channel(1024);
        let fan_in = tx.clone();
        let mut message_rx = subscriber.message_rx();
        tokio::spawn(async move {
            // Keep the subscriber client alive for as long as the fan-in runs.
            let _subscriber = subscriber;
            loop {
                match message_rx.recv().await {
                    Ok(message) => {
                        if let Some(hint) = message
                            .value
                            .as_bytes()
                            .and_then(|b| serde_json::from_slice::<Hint>(b).ok())
                        {
                            let _ = fan_in.send(hint);
                        }
                    }
                    // Lagging (or a reconnect-induced gap) loses hints, never
                    // data — clients refetch on the next hint or reconnect.
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "hint fan-in lagged; continuing");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(Self { client, tx })
    }
}

#[async_trait::async_trait]
impl PubSub for RedisPubSub {
    async fn publish(&self, hint: &Hint) -> anyhow::Result<()> {
        let payload = serde_json::to_string(hint)?;
        let receivers: i64 = self.client.publish(HINT_CHANNEL, payload).await?;
        // Every healthy replica holds exactly one subscription, so zero
        // receivers is always the degraded window right after a Redis
        // recovery, before any fan-in subscriber has reattached. A
        // fire-and-forget publish there would LOSE the hint. Failing the
        // job instead retries it via fail_job, which turns the loss into
        // the delay the contract allows.
        anyhow::ensure!(receivers > 0, "hint published to zero receivers");
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<Hint> {
        self.tx.subscribe()
    }

    async fn try_acquire_debounce(&self, key: &str, window: Duration) -> anyhow::Result<bool> {
        // SET NX PX: cross-replica debounce. Key loss (Redis restart) can only
        // cause an extra hint, never a missed one.
        let acquired: Option<String> = self
            .client
            .set(
                format!("dronte:debounce:{key}"),
                1,
                Some(fred::types::Expiration::PX(window.as_millis() as i64)),
                Some(fred::types::SetOptions::NX),
                false,
            )
            .await?;
        Ok(acquired.is_some())
    }
}

// =============================================================================
// Postgres LISTEN/NOTIFY fallback
// =============================================================================

pub struct PgPubSub {
    pool: PgPool,
    tx: broadcast::Sender<Hint>,
    /// Per-replica debounce state. Redis-less mode is the single-binary dev
    /// path. With multiple replicas the debounce degrades to per-replica,
    /// producing more hints than strictly needed, never fewer.
    debounce: Mutex<HashMap<String, std::time::Instant>>,
}

impl PgPubSub {
    pub async fn connect(pool: &PgPool) -> anyhow::Result<Self> {
        // PgListener holds a dedicated connection for the lifetime of the
        // loop (see module docs re: PgBouncer).
        let mut listener = PgListener::connect_with(pool).await?;
        listener.listen(HINT_CHANNEL).await?;

        let (tx, _) = broadcast::channel(1024);
        let fan_in = tx.clone();
        tokio::spawn(async move {
            loop {
                match listener.recv().await {
                    Ok(notification) => {
                        if let Ok(hint) = serde_json::from_str::<Hint>(notification.payload()) {
                            let _ = fan_in.send(hint);
                        }
                    }
                    Err(err) => {
                        tracing::warn!(error = ?err, "LISTEN connection lost; reconnecting");
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        });

        Ok(Self {
            pool: pool.clone(),
            tx,
            debounce: Mutex::new(HashMap::new()),
        })
    }
}

#[async_trait::async_trait]
impl PubSub for PgPubSub {
    async fn publish(&self, hint: &Hint) -> anyhow::Result<()> {
        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(HINT_CHANNEL)
            .bind(serde_json::to_string(hint)?)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<Hint> {
        self.tx.subscribe()
    }

    async fn try_acquire_debounce(&self, key: &str, window: Duration) -> anyhow::Result<bool> {
        let mut map = self.debounce.lock().expect("debounce lock");
        let now = std::time::Instant::now();
        map.retain(|_, t| now.duration_since(*t) < window);
        match map.get(key) {
            Some(_) => Ok(false),
            None => {
                map.insert(key.to_owned(), now);
                Ok(true)
            }
        }
    }
}

//! Shared handler state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sqlx::PgPool;
use tokio::sync::watch;
use uuid::Uuid;

use crate::config::Config;
use crate::pubsub::PubSub;
use crate::ratelimit::RateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub cfg: Arc<Config>,
    pub pubsub: Arc<dyn PubSub>,
    pub ratelimit: Arc<dyn RateLimiter>,
    /// Per-replica SSE connection counts keyed by (environment, subscriber).
    /// Enforces the per-subscriber cap.
    pub sse_connections: Arc<Mutex<HashMap<(Uuid, Uuid), usize>>>,
    /// Flips to `true` when shutdown begins. /readyz answers 503 from that
    /// moment so load balancers drain the replica before the listener closes.
    /// The listener itself stays open until `shutdown` flips.
    pub draining: watch::Receiver<bool>,
    /// Flips to `true` when the listener is about to close. SSE streams answer
    /// with a jittered `retry:` directive and close. Workers stop claiming.
    pub shutdown: watch::Receiver<bool>,
}

impl AppState {
    pub fn new(
        pool: PgPool,
        cfg: Arc<Config>,
        pubsub: Arc<dyn PubSub>,
        ratelimit: Arc<dyn RateLimiter>,
        draining: watch::Receiver<bool>,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            pool,
            cfg,
            pubsub,
            ratelimit,
            sse_connections: Arc::new(Mutex::new(HashMap::new())),
            draining,
            shutdown,
        }
    }
}

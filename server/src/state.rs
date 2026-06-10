//! Shared handler state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sqlx::PgPool;
use tokio::sync::watch;
use uuid::Uuid;

use crate::config::Config;
use crate::pubsub::PubSub;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub cfg: Arc<Config>,
    pub pubsub: Arc<dyn PubSub>,
    /// Per-replica SSE connection counts keyed by (environment, subscriber).
    /// Enforces the per-subscriber cap (risk M3).
    pub sse_connections: Arc<Mutex<HashMap<(Uuid, Uuid), usize>>>,
    /// Flips to `true` on graceful shutdown. SSE streams answer with a
    /// jittered `retry:` directive and close.
    pub shutdown: watch::Receiver<bool>,
}

impl AppState {
    pub fn new(
        pool: PgPool,
        cfg: Arc<Config>,
        pubsub: Arc<dyn PubSub>,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            pool,
            cfg,
            pubsub,
            sse_connections: Arc::new(Mutex::new(HashMap::new())),
            shutdown,
        }
    }
}

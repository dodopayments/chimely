//! Integration-test harness: real Postgres (+ optional Redis) via
//! testcontainers — no storage or pub/sub mocks, ever (CLAUDE.md).
//!
//! Each test gets its own containers, its own environment row, and an
//! in-process router served on an ephemeral port. The worker loop is NOT
//! spawned by default — tests drive `worker::sweep_once` (or
//! `spawn_worker()`) so job processing is deterministic.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dronte::auth::compute_subscriber_hash;
use dronte::config::Config;
use dronte::pubsub::PubSub;
use dronte::state::AppState;
use dronte::{db, http, ids, partitions, pubsub, worker};
use reqwest::header::{HeaderMap, HeaderValue};
use sha2::Digest as _;
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres as PostgresImage;
use testcontainers_modules::redis::Redis as RedisImage;
use testcontainers_modules::testcontainers::core::IntoContainerPort as _;
use testcontainers_modules::testcontainers::runners::AsyncRunner as _;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt as _};
use uuid::Uuid;

pub const RETENTION_MONTHS: u32 = 12;

pub struct TestApp {
    pub pool: PgPool,
    pub base: String,
    pub client: reqwest::Client,
    pub cfg: Arc<Config>,
    pub pubsub: Arc<dyn PubSub>,
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    pub env: TestEnvironment,
    pub redis: Option<ContainerAsync<RedisImage>>,
    _pg: ContainerAsync<PostgresImage>,
}

pub struct TestEnvironment {
    pub id: Uuid,
    pub slug: String,
    pub api_key: String,
    pub hmac_secret: String,
    pub require_subscriber_hash: bool,
}

pub async fn spawn() -> TestApp {
    spawn_inner(false, true).await
}

pub async fn spawn_with_redis() -> TestApp {
    spawn_inner(true, true).await
}

/// `require_hash = false` ⇒ a dev-mode environment (quickstart path).
pub async fn spawn_dev_mode() -> TestApp {
    spawn_inner(false, false).await
}

async fn spawn_inner(with_redis: bool, require_hash: bool) -> TestApp {
    // postgres:15 on purpose — the schema contract targets Postgres >=15, so
    // tests pin the floor (a query needing 16+ features must fail here).
    let pg = PostgresImage::default()
        .with_tag("15-alpine")
        .start()
        .await
        .expect("starting postgres container");
    let pg_port = pg
        .get_host_port_ipv4(5432.tcp())
        .await
        .expect("postgres port");
    let database_url = format!("postgres://postgres:postgres@127.0.0.1:{pg_port}/postgres");

    let (redis, redis_url) = if with_redis {
        // A FIXED host port: docker reassigns ephemeral published ports on
        // container restart, which would break the Redis kill/recover tests
        // (clients must be able to reconnect to the same URL).
        let port = free_port();
        let redis = RedisImage::default()
            .with_tag("7-alpine")
            .with_mapped_port(port, 6379.tcp())
            .start()
            .await
            .expect("starting redis container");
        (Some(redis), Some(format!("redis://127.0.0.1:{port}")))
    } else {
        (None, None)
    };

    let pool = retry_connect(&database_url).await;
    db::migrate(&pool).await.expect("migrations");
    partitions::run(&pool, RETENTION_MONTHS, 30)
        .await
        .expect("partition maintenance");

    let cfg = Arc::new(Config {
        database_url,
        redis_url: redis_url.clone(),
        listen_addr: "127.0.0.1:0".into(),
        retention_months: RETENTION_MONTHS,
        idempotency_retention_days: 30,
        hint_debounce: Duration::from_millis(250),
        worker_poll_interval: Duration::from_millis(25),
        sse_ping_interval: Duration::from_millis(400),
        sse_retry_base: Duration::from_millis(100),
        sse_retry_jitter: Duration::from_millis(100),
        sse_max_connections_per_subscriber: 2,
    });

    let pubsub = pubsub::build(redis_url.as_deref(), &pool)
        .await
        .expect("pubsub");
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let state = AppState::new(
        pool.clone(),
        cfg.clone(),
        pubsub.clone(),
        shutdown_rx.clone(),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let router = http::router(state);
    let mut serve_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                serve_shutdown.changed().await.ok();
            })
            .await
            .ok();
    });

    let mut app = TestApp {
        pool,
        base: format!("http://{addr}"),
        client: reqwest::Client::new(),
        cfg,
        pubsub,
        shutdown_tx,
        env: TestEnvironment {
            id: Uuid::nil(),
            slug: String::new(),
            api_key: String::new(),
            hmac_secret: String::new(),
            require_subscriber_hash: require_hash,
        },
        redis,
        _pg: pg,
    };
    app.env = app.create_environment(require_hash).await;
    app
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("probing a free port")
        .local_addr()
        .expect("local addr")
        .port()
}

async fn retry_connect(url: &str) -> PgPool {
    for _ in 0..50 {
        if let Ok(pool) = db::connect(url).await
            && sqlx::query("SELECT 1").execute(&pool).await.is_ok()
        {
            return pool;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("postgres container never became connectable");
}

impl TestApp {
    /// Insert an environment + API key directly (key management is the Phase
    /// 4 admin UI; v1 has no HTTP surface for it).
    pub async fn create_environment(&self, require_hash: bool) -> TestEnvironment {
        let id = ids::new_uuid();
        let slug = format!("env-{}", &id.as_simple().to_string()[..12]);
        let hmac_secret = format!("whsec_{}", ids::new_uuid().as_simple());
        sqlx::query(
            "INSERT INTO environments
                 (id, slug, name, subscriber_hmac_secret, require_subscriber_hash)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(&slug)
        .bind(&slug)
        .bind(&hmac_secret)
        .bind(require_hash)
        .execute(&self.pool)
        .await
        .expect("insert environment");

        let api_key = format!("drnt_test_{}", ids::new_uuid().as_simple());
        let key_hash: Vec<u8> = sha2::Sha256::digest(api_key.as_bytes()).to_vec();
        sqlx::query(
            "INSERT INTO api_keys (environment_id, id, name, key_hash, key_prefix)
             VALUES ($1, $2, 'test', $3, $4)",
        )
        .bind(id)
        .bind(ids::new_uuid())
        .bind(key_hash)
        .bind(&api_key[..14])
        .execute(&self.pool)
        .await
        .expect("insert api key");

        TestEnvironment {
            id,
            slug,
            api_key,
            hmac_secret,
            require_subscriber_hash: require_hash,
        }
    }

    // ----- HTTP helpers ------------------------------------------------------

    pub fn mgmt_post(&self, path: &str, body: serde_json::Value) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.base))
            .bearer_auth(&self.env.api_key)
            .json(&body)
    }

    pub fn subscriber_headers(&self, subscriber: &str) -> HeaderMap {
        self.subscriber_headers_for(&self.env, subscriber)
    }

    pub fn subscriber_headers_for(&self, env: &TestEnvironment, subscriber: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Dronte-Environment",
            HeaderValue::from_str(&env.slug).unwrap(),
        );
        headers.insert(
            "X-Dronte-Subscriber",
            HeaderValue::from_str(subscriber).unwrap(),
        );
        headers.insert(
            "X-Dronte-Subscriber-Hash",
            HeaderValue::from_str(&compute_subscriber_hash(&env.hmac_secret, subscriber)).unwrap(),
        );
        headers
    }

    pub async fn create_notification(&self, subscriber: &str, category: &str) -> serde_json::Value {
        let res = self
            .mgmt_post(
                "/v1/notifications",
                serde_json::json!({ "subscriber_id": subscriber, "category": category }),
            )
            .send()
            .await
            .expect("create notification");
        assert_eq!(
            res.status(),
            201,
            "create notification failed: {}",
            res.text().await.unwrap()
        );
        res.json().await.expect("create notification body")
    }

    pub async fn create_broadcast(&self, category: &str) -> serde_json::Value {
        let res = self
            .mgmt_post(
                "/v1/broadcasts",
                serde_json::json!({ "category": category }),
            )
            .send()
            .await
            .expect("create broadcast");
        assert_eq!(res.status(), 201, "create broadcast failed");
        res.json().await.expect("create broadcast body")
    }

    pub async fn list_items(&self, subscriber: &str) -> serde_json::Value {
        let res = self
            .client
            .get(format!("{}/v1/inbox/items", self.base))
            .headers(self.subscriber_headers(subscriber))
            .send()
            .await
            .expect("list items");
        assert_eq!(res.status(), 200);
        res.json().await.expect("list body")
    }

    /// Full pagination at the given page size; returns all items in order.
    pub async fn list_all_items(&self, subscriber: &str, limit: i64) -> Vec<serde_json::Value> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut url = format!("{}/v1/inbox/items?limit={limit}", self.base);
            if let Some(c) = &cursor {
                url.push_str(&format!("&cursor={c}"));
            }
            let res = self
                .client
                .get(url)
                .headers(self.subscriber_headers(subscriber))
                .send()
                .await
                .expect("list items");
            assert_eq!(res.status(), 200);
            let page: serde_json::Value = res.json().await.expect("page");
            let page_items = page["items"].as_array().expect("items").clone();
            items.extend(page_items);
            match page["next_cursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => return items,
            }
        }
    }

    pub async fn counts(&self, subscriber: &str) -> (i64, i64) {
        let res = self
            .client
            .get(format!("{}/v1/inbox/counts", self.base))
            .headers(self.subscriber_headers(subscriber))
            .send()
            .await
            .expect("counts");
        assert_eq!(res.status(), 200);
        let body: serde_json::Value = res.json().await.expect("counts body");
        (
            body["unread"].as_i64().unwrap(),
            body["unseen"].as_i64().unwrap(),
        )
    }

    pub async fn post_inbox(&self, subscriber: &str, path: &str) -> reqwest::Response {
        self.client
            .post(format!("{}{path}", self.base))
            .headers(self.subscriber_headers(subscriber))
            .send()
            .await
            .expect("inbox post")
    }

    // ----- Worker / jobs -----------------------------------------------------

    /// One fair worker pass.
    pub async fn sweep(&self) -> u64 {
        worker::sweep_once(&self.pool, self.pubsub.as_ref(), &self.cfg)
            .await
            .expect("sweep")
    }

    /// Sweep until the queue drains (bounded; debounce-deferred hints are
    /// waited out so tests see the trailing-edge publish too).
    pub async fn drain_jobs(&self) {
        for _ in 0..200 {
            let due: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM jobs WHERE run_at <= now() + interval '2 seconds'",
            )
            .fetch_one(&self.pool)
            .await
            .expect("count jobs");
            if due == 0 {
                return;
            }
            if self.sweep().await == 0 {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        panic!("jobs never drained");
    }

    pub async fn job_count(&self, env: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id = $1")
            .bind(env)
            .fetch_one(&self.pool)
            .await
            .expect("job count")
    }

    pub fn spawn_worker(&self) {
        tokio::spawn(worker::run(
            self.pool.clone(),
            self.pubsub.clone(),
            self.cfg.clone(),
            self.shutdown_tx.subscribe(),
        ));
    }

    /// Stop the Postgres container (readiness-failure tests).
    pub async fn _pg_stop(&self) {
        self._pg
            .stop_with_timeout(Some(1))
            .await
            .expect("stopping postgres");
    }

    // ----- Direct DB peeks ----------------------------------------------------

    pub async fn counter_row(&self, subscriber: &str) -> (i32, i32, DateTime<Utc>) {
        sqlx::query_as(
            "SELECT c.unread_direct_count, c.unseen_direct_count, c.updated_at
               FROM subscriber_counters c
               JOIN subscribers s ON s.environment_id = c.environment_id
                                 AND s.id = c.subscriber_id
              WHERE c.environment_id = $1 AND s.subscriber_id = $2",
        )
        .bind(self.env.id)
        .bind(subscriber)
        .fetch_one(&self.pool)
        .await
        .expect("counter row")
    }
}

/// A live SSE connection with line-level access to frames.
pub struct SseStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
}

impl SseStream {
    pub async fn connect(app: &TestApp, subscriber: &str, last_event_id: Option<&str>) -> Self {
        let hash = compute_subscriber_hash(&app.env.hmac_secret, subscriber);
        let url = format!(
            "{}/v1/inbox/stream?environment={}&subscriber_id={subscriber}&subscriber_hash={hash}",
            app.base, app.env.slug,
        );
        let mut req = app.client.get(url);
        if let Some(id) = last_event_id {
            req = req.header("Last-Event-ID", id);
        }
        let response = req.send().await.expect("sse connect");
        assert_eq!(response.status(), 200, "sse connect failed");
        Self {
            response,
            buffer: Vec::new(),
        }
    }

    pub async fn try_connect(app: &TestApp, subscriber: &str) -> reqwest::Response {
        let hash = compute_subscriber_hash(&app.env.hmac_secret, subscriber);
        let url = format!(
            "{}/v1/inbox/stream?environment={}&subscriber_id={subscriber}&subscriber_hash={hash}",
            app.base, app.env.slug,
        );
        app.client
            .get(url)
            .send()
            .await
            .expect("sse connect attempt")
    }

    /// Next full SSE frame (blank-line delimited) within the timeout; None on
    /// timeout. Comment-only frames are returned too (keep-alive assertions).
    pub async fn next_frame(&mut self, timeout: Duration) -> Option<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(pos) = find_frame_end(&self.buffer) {
                let frame: Vec<u8> = self.buffer.drain(..pos + 2).collect();
                let text = String::from_utf8_lossy(&frame).trim_end().to_string();
                if text.is_empty() {
                    continue;
                }
                return Some(text);
            }
            let chunk = tokio::time::timeout_at(deadline, self.response.chunk()).await;
            match chunk {
                Ok(Ok(Some(bytes))) => self.buffer.extend_from_slice(&bytes),
                Ok(Ok(None)) | Ok(Err(_)) | Err(_) => return None,
            }
        }
    }

    /// Next `event: hint` frame, skipping keep-alive comments.
    pub async fn next_hint(&mut self, timeout: Duration) -> Option<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.checked_duration_since(tokio::time::Instant::now())?;
            let frame = self.next_frame(remaining).await?;
            if frame.contains("event: hint") {
                return Some(frame);
            }
        }
    }
}

fn find_frame_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(2).position(|w| w == b"\n\n")
}

pub fn event_id(frame: &str) -> Option<String> {
    frame
        .lines()
        .find_map(|l| l.strip_prefix("id: "))
        .map(str::to_owned)
}

//! Integration-test harness: real Postgres and optional Redis via
//! testcontainers. No storage or pub/sub mocks.
//!
//! Each test gets its own containers, its own environment row, and an
//! in-process router on an ephemeral port. The worker loop is not spawned by
//! default. Tests drive `worker::sweep_once` or `spawn_worker()` so job
//! processing is deterministic.

// Each integration test is its own crate compiling this shared harness and
// uses a subset of its helpers, so per-binary dead_code is a false positive.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use chimely::auth::compute_subscriber_hash;
use chimely::config::Config;
use chimely::pubsub::PubSub;
use chimely::state::AppState;
use chimely::{db, http, ids, partitions, pubsub, ratelimit, worker};
use chrono::{DateTime, Utc};
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
/// The seed admin the harness creates and logs in as by default.
pub const ADMIN_TEST_EMAIL: &str = "admin@test.chimely";
pub const ADMIN_TEST_PASSWORD: &str = "test-admin-password";

pub struct TestApp {
    pub pool: PgPool,
    pub base: String,
    pub client: reqwest::Client,
    pub cfg: Arc<Config>,
    pub pubsub: Arc<dyn PubSub>,
    pub ratelimit: Arc<dyn ratelimit::RateLimiter>,
    pub draining_tx: tokio::sync::watch::Sender<bool>,
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
    spawn_inner(false, true, |_| {}).await
}

pub async fn spawn_with_redis() -> TestApp {
    spawn_inner(true, true, |_| {}).await
}

/// `require_hash = false` is a dev-mode environment (quickstart path).
pub async fn spawn_dev_mode() -> TestApp {
    spawn_inner(false, false, |_| {}).await
}

/// Custom config knobs (rate limits, backoff, shutdown timings).
pub async fn spawn_configured(with_redis: bool, configure: impl FnOnce(&mut Config)) -> TestApp {
    spawn_inner(with_redis, true, configure).await
}

async fn spawn_inner(
    with_redis: bool,
    require_hash: bool,
    configure: impl FnOnce(&mut Config),
) -> TestApp {
    // The schema contract targets Postgres >=15. Pinning the floor makes a
    // query needing 16+ features fail here.
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
        // A fixed host port so the Redis kill/recover tests reconnect to the
        // same URL across an outage. The outage is simulated with pause/unpause
        // (see chaos.rs) which keeps this published port bound, avoiding the
        // stop/start "port is already allocated" race on the down window.
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

    let mut cfg = Config {
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
        dev_environment: None,
        dev_api_key: None,
        // The harness seeds the admin user and logs in directly.
        // Bootstrap-from-env has its own test.
        admin_bootstrap_email: None,
        admin_bootstrap_password: None,
        admin_session_ttl: Duration::from_secs(3600),
        // No TLS in tests. Cookies omit Secure so reqwest's cookie store
        // replays them over plain HTTP to 127.0.0.1.
        admin_tls_terminated: false,
        retry_backoff_base: Duration::from_millis(40),
        retry_backoff_cap: Duration::from_millis(500),
        metrics_sample_interval: Duration::from_millis(200),
        counter_drift_sample_size: 100,
        // Rate limits default off in tests. Rate-limit tests opt in via
        // spawn_configured.
        api_key_rate_per_sec: 0.0,
        api_key_rate_burst: 0.0,
        subscriber_rate_per_sec: 0.0,
        subscriber_rate_burst: 0.0,
        shutdown_readiness_grace: Duration::from_millis(150),
        shutdown_drain_deadline: Duration::from_secs(5),
        // Off matches the production default. Tests that need the scrub
        // opt in via spawn_configured.
        log_scrub_identifiers: false,
    };
    configure(&mut cfg);
    let cfg = Arc::new(cfg);

    let pubsub = pubsub::build(redis_url.as_deref(), &pool)
        .await
        .expect("pubsub");
    let ratelimit = ratelimit::build(redis_url.as_deref())
        .await
        .expect("ratelimit");
    let (draining_tx, draining_rx) = tokio::sync::watch::channel(false);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let state = AppState::new(
        pool.clone(),
        cfg.clone(),
        pubsub.clone(),
        ratelimit.clone(),
        draining_rx,
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
        // Cookie store so the admin session cookie set by /admin/api/login is
        // replayed on later admin requests.
        client: reqwest::Client::builder()
            .cookie_store(true)
            .build()
            .expect("reqwest client"),
        cfg,
        pubsub,
        ratelimit,
        draining_tx,
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
    // Seed a default admin user and log the harness client in so admin_get and
    // admin_post carry a live session by default.
    app.seed_admin(ADMIN_TEST_EMAIL, ADMIN_TEST_PASSWORD, "admin")
        .await;
    app.login(ADMIN_TEST_EMAIL, ADMIN_TEST_PASSWORD).await;
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
    /// Insert an environment and API key directly. Key management lives in the
    /// admin UI. There is no HTTP surface for it.
    pub async fn create_environment(&self, require_hash: bool) -> TestEnvironment {
        let id = ids::new_uuid();
        let slug = format!("env-{}", &id.as_simple().to_string()[..12]);
        let hmac_secret = format!("shmac_{}", ids::new_uuid().as_simple());
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

        let api_key = format!("chml_test_{}", ids::new_uuid().as_simple());
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

    /// Insert an admin user directly (bypassing the API). Returns its uuid.
    pub async fn seed_admin(&self, email: &str, password: &str, role: &str) -> Uuid {
        let id = ids::new_uuid();
        let hash = chimely::auth::hash_password(password).expect("hash password");
        sqlx::query(
            "INSERT INTO admin_users (id, email, name, role, password_hash)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(email.to_lowercase())
        .bind(email)
        .bind(role)
        .bind(hash)
        .execute(&self.pool)
        .await
        .expect("seed admin user");
        id
    }

    /// Log the harness client in (stores the session cookie in its jar).
    pub async fn login(&self, email: &str, password: &str) {
        let res = self
            .client
            .post(format!("{}/admin/api/login", self.base))
            .header("x-chimely-admin", "1")
            .json(&serde_json::json!({ "email": email, "password": password }))
            .send()
            .await
            .expect("login request");
        assert_eq!(
            res.status(),
            200,
            "login failed: {}",
            res.text().await.unwrap()
        );
    }

    /// A fresh cookie-store client logged in as the given user (role tests).
    pub async fn login_client(&self, email: &str, password: &str) -> reqwest::Client {
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .build()
            .expect("reqwest client");
        let res = client
            .post(format!("{}/admin/api/login", self.base))
            .header("x-chimely-admin", "1")
            .json(&serde_json::json!({ "email": email, "password": password }))
            .send()
            .await
            .expect("login request");
        assert_eq!(res.status(), 200, "login failed for {email}");
        client
    }

    /// Admin-plane GET. The harness client's session cookie authenticates it.
    pub fn admin_get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{path}", self.base))
    }

    pub fn admin_post(&self, path: &str, body: serde_json::Value) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.base))
            .header("x-chimely-admin", "1")
            .json(&body)
    }

    pub fn admin_patch(&self, path: &str, body: serde_json::Value) -> reqwest::RequestBuilder {
        self.client
            .patch(format!("{}{path}", self.base))
            .header("x-chimely-admin", "1")
            .json(&body)
    }

    pub fn admin_delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .delete(format!("{}{path}", self.base))
            .header("x-chimely-admin", "1")
    }

    pub fn subscriber_headers(&self, subscriber: &str) -> HeaderMap {
        self.subscriber_headers_for(&self.env, subscriber)
    }

    pub fn subscriber_headers_for(&self, env: &TestEnvironment, subscriber: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Chimely-Environment",
            HeaderValue::from_str(&env.slug).unwrap(),
        );
        headers.insert(
            "X-Chimely-Subscriber",
            HeaderValue::from_str(subscriber).unwrap(),
        );
        headers.insert(
            "X-Chimely-Subscriber-Hash",
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

    /// Full pagination at the given page size. Returns all items in order.
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

    /// Sweep until the queue drains, bounded. Debounce-deferred hints are
    /// waited out so tests see the trailing-edge publish too.
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

    pub async fn dead_letter_count(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM dead_letters")
            .fetch_one(&self.pool)
            .await
            .expect("dead letter count")
    }

    /// Status rows for one notification, oldest first.
    pub async fn timeline_rows(&self, notification: Uuid) -> Vec<(String, DateTime<Utc>)> {
        sqlx::query_as(
            "SELECT status, occurred_at FROM notification_status_log
              WHERE environment_id = $1 AND notification_id = $2
              ORDER BY occurred_at, status",
        )
        .bind(self.env.id)
        .bind(notification)
        .fetch_all(&self.pool)
        .await
        .expect("timeline rows")
    }

    /// GET /v1/notifications/{id}/timeline (management plane).
    pub async fn timeline_api(&self, id: &str) -> reqwest::Response {
        self.client
            .get(format!("{}/v1/notifications/{id}/timeline", self.base))
            .bearer_auth(&self.env.api_key)
            .send()
            .await
            .expect("timeline request")
    }

    /// A second server replica over the SAME Postgres (and Redis, if any):
    /// its own hint-plane and rate-limiter connections, router, listener,
    /// and shutdown switches. Cross-replica behavior (shared rate-limit
    /// buckets, SSE reconnect against a surviving replica) is tested
    /// against this.
    pub async fn spawn_replica(&self) -> Replica {
        let pubsub = pubsub::build(self.cfg.redis_url.as_deref(), &self.pool)
            .await
            .expect("replica pubsub");
        let ratelimit = ratelimit::build(self.cfg.redis_url.as_deref())
            .await
            .expect("replica ratelimit");
        let (draining_tx, draining_rx) = tokio::sync::watch::channel(false);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let state = AppState::new(
            self.pool.clone(),
            self.cfg.clone(),
            pubsub.clone(),
            ratelimit,
            draining_rx,
            shutdown_rx.clone(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("replica bind");
        let addr = listener.local_addr().expect("replica addr");
        let mut serve_shutdown = shutdown_rx;
        tokio::spawn(async move {
            axum::serve(listener, http::router(state))
                .with_graceful_shutdown(async move {
                    serve_shutdown.changed().await.ok();
                })
                .await
                .ok();
        });
        Replica {
            base: format!("http://{addr}"),
            pubsub,
            draining_tx,
            shutdown_tx,
        }
    }

    /// The chaos suite's final consistency sweep. Recounted counters equal
    /// maintained counters, the API list equals a recomputed merge of both
    /// sources, no orphaned or parked jobs, partitions cover the horizon.
    pub async fn assert_consistent(&self) {
        // Counter drift across EVERY subscriber, not a sample.
        let (unread_drift, unseen_drift) =
            chimely::metrics_sampler::counter_drift(&self.pool, i64::MAX)
                .await
                .expect("counter drift");
        assert_eq!(
            (unread_drift, unseen_drift),
            (0, 0),
            "sampled recount diverged from maintained counters"
        );

        // The API list must equal the recomputed two-source merge.
        let subscribers: Vec<String> =
            sqlx::query_scalar("SELECT subscriber_id FROM subscribers WHERE environment_id = $1")
                .bind(self.env.id)
                .fetch_all(&self.pool)
                .await
                .expect("subscribers");
        for external in subscribers {
            let expected: Vec<(String, Uuid)> = sqlx::query_as(
                "WITH me AS (SELECT id, created_at FROM subscribers
                              WHERE environment_id = $1 AND subscriber_id = $2)
                 SELECT kind, id FROM (
                     SELECT 'notification' AS kind, n.id, n.visible_at AS occurred_at
                       FROM notifications n, me, subscriber_counters c
                      WHERE c.environment_id = $1 AND c.subscriber_id = me.id
                        AND n.environment_id = $1 AND n.subscriber_id = me.id
                        AND n.visible_at <= now()
                        AND NOT (n.archived_at IS NOT NULL
                              OR (n.unarchived_at IS NULL
                                  AND n.visible_at <= c.archive_watermark))
                        AND NOT EXISTS (SELECT 1 FROM preferences p
                              WHERE p.environment_id = $1 AND p.subscriber_id = me.id
                                AND p.category = n.category AND p.channel = 'in_app'
                                AND p.enabled = false)
                     UNION ALL
                     SELECT 'broadcast', b.id, b.created_at
                       FROM broadcasts b, me, subscriber_counters c
                      WHERE c.environment_id = $1 AND c.subscriber_id = me.id
                        AND b.environment_id = $1 AND b.created_at >= me.created_at
                        AND NOT COALESCE((SELECT ba.archived FROM broadcast_archives ba
                              WHERE ba.environment_id = $1 AND ba.subscriber_id = me.id
                                AND ba.broadcast_id = b.id),
                              b.created_at <= c.archive_watermark)
                        AND NOT EXISTS (SELECT 1 FROM preferences p
                              WHERE p.environment_id = $1 AND p.subscriber_id = me.id
                                AND p.category = b.category AND p.channel = 'in_app'
                                AND p.enabled = false)
                 ) merged ORDER BY occurred_at DESC, id DESC",
            )
            .bind(self.env.id)
            .bind(&external)
            .fetch_all(&self.pool)
            .await
            .expect("merge recompute");
            let expected_ids: Vec<String> = expected
                .into_iter()
                .map(|(kind, id)| match kind.as_str() {
                    "notification" => ids::typeid(ids::NOTIFICATION, id),
                    _ => ids::typeid(ids::BROADCAST, id),
                })
                .collect();
            let listed: Vec<String> = self
                .list_all_items(&external, 100)
                .await
                .into_iter()
                .map(|item| item["id"].as_str().expect("item id").to_owned())
                .collect();
            assert_eq!(listed, expected_ids, "list != merge for {external}");
        }

        // No orphaned deliver jobs (payload ids must exist) and nothing
        // parked unless a test asserted dead letters on purpose.
        let orphans: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs j
             CROSS JOIN LATERAL jsonb_array_elements_text(
                 CASE WHEN jsonb_typeof(j.payload->'notification_ids') = 'array'
                      THEN j.payload->'notification_ids' END) AS t(nid)
             WHERE j.job_type = 'deliver'
               AND NOT EXISTS (SELECT 1 FROM notifications n
                     WHERE n.environment_id = j.environment_id AND n.id = t.nid::uuid)",
        )
        .fetch_one(&self.pool)
        .await
        .expect("orphan check");
        assert_eq!(orphans, 0, "deliver jobs referencing missing notifications");

        // Partition headroom for every partitioned table.
        let mut conn = self.pool.acquire().await.expect("conn");
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
            .fetch_one(&mut *conn)
            .await
            .expect("db now");
        for table in partitions::PARTITIONED_TABLES {
            let remaining = partitions::remaining_at(&mut conn, table, now)
                .await
                .expect("remaining_at");
            assert!(remaining >= 12, "{table} headroom shrank: {remaining}");
        }
    }
}

pub struct Replica {
    pub base: String,
    pub pubsub: Arc<dyn PubSub>,
    pub draining_tx: tokio::sync::watch::Sender<bool>,
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
}

/// A live SSE connection with line-level access to frames.
pub struct SseStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
}

impl SseStream {
    pub async fn connect(app: &TestApp, subscriber: &str, last_event_id: Option<&str>) -> Self {
        Self::connect_to(&app.base, app, subscriber, last_event_id).await
    }

    /// Connect against an arbitrary base URL (a second replica).
    pub async fn connect_to(
        base: &str,
        app: &TestApp,
        subscriber: &str,
        last_event_id: Option<&str>,
    ) -> Self {
        let hash = compute_subscriber_hash(&app.env.hmac_secret, subscriber);
        let url = format!(
            "{base}/v1/inbox/stream?environment={}&subscriber_id={subscriber}&subscriber_hash={hash}",
            app.env.slug,
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

    /// Next full SSE frame (blank-line delimited) within the timeout. None on
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

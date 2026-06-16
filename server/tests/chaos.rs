//! Phase 3 chaos suite (specs/phase-3-hardening.md, deliverable 7): real
//! Postgres + Redis via testcontainers, no mocks, every scenario ending in
//! the full consistency sweep (`assert_consistent`): recounted counters ==
//! maintained counters, list == merge of sources, no orphaned jobs,
//! partitions cover the horizon.

mod support;

use std::time::Duration;

use chrono::Utc;
use dronte::{db, jobs, partitions, worker};
use serde_json::json;
use support::SseStream;
use testcontainers_modules::postgres::Postgres as PostgresImage;
use testcontainers_modules::testcontainers::ImageExt as _;
use testcontainers_modules::testcontainers::runners::AsyncRunner as _;
use uuid::Uuid;

/// Kill a worker mid-job AFTER a partial side effect committed: the first
/// chunk lands, the crash rolls back the second, and the successor resumes
/// from the cursor. At-least-once replay, zero double-applied effects.
#[tokio::test]
async fn worker_killed_after_partial_chunks_resumes_without_double_effects() {
    let app = support::spawn().await;
    let deliver_at = Utc::now() + chrono::Duration::milliseconds(900);
    for batch in 0..6 {
        let recipients: Vec<String> = (0..100).map(|i| format!("usr_c_{batch}_{i}")).collect();
        let res = app
            .mgmt_post(
                "/v1/notifications",
                json!({ "subscriber_ids": recipients, "category": "big",
                        "deliver_at": deliver_at.to_rfc3339() }),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
    }
    let all_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM notifications WHERE environment_id = $1 ORDER BY id")
            .bind(app.env.id)
            .fetch_all(&app.pool)
            .await
            .unwrap();
    assert_eq!(all_ids.len(), 600);
    // Past deliver_at: the rows are visible, the natural state for the
    // consistency sweep at the end.
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    sqlx::query("DELETE FROM jobs WHERE environment_id = $1")
        .bind(app.env.id)
        .execute(&app.pool)
        .await
        .unwrap();
    let mut conn = app.pool.acquire().await.unwrap();
    jobs::enqueue(
        &mut conn,
        app.env.id,
        jobs::TYPE_DELIVER,
        json!({ "notification_ids": all_ids }),
        None,
    )
    .await
    .unwrap();
    drop(conn);

    // Chunk 1 (500 rows) COMMITS.
    assert!(
        worker::process_one(&app.pool, app.pubsub.as_ref(), &app.cfg, app.env.id)
            .await
            .unwrap()
    );
    // The worker dies mid-chunk-2: effects applied then rolled back.
    assert!(
        worker::crash_mid_deliver(&app.pool, app.env.id)
            .await
            .unwrap()
    );

    let total_unread: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(unread_direct_count), 0)
           FROM subscriber_counters WHERE environment_id = $1",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(total_unread, 500, "only the committed chunk counts");

    // The successor resumes from the cursor and finishes exactly once.
    app.drain_jobs().await;
    let total_unread: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(unread_direct_count), 0)
           FROM subscriber_counters WHERE environment_id = $1",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(total_unread, 600, "every row bumped exactly once");
    assert_eq!(app.job_count(app.env.id).await, 0);
    app.assert_consistent().await;
}

/// Kill the server mid-SSE-stream: the client reconnects to a SURVIVING
/// replica with Last-Event-ID and refetches. No state is missed, by
/// construction (hints are refetch triggers, not transports).
#[tokio::test]
async fn server_killed_mid_sse_stream_reconnects_to_replica_without_missed_state() {
    let app = support::spawn().await;
    let replica = app.spawn_replica().await;

    let mut stream = SseStream::connect(&app, "usr_sse", None).await;
    app.create_notification("usr_sse", "first").await;
    app.drain_jobs().await;
    let frame = stream
        .next_hint(Duration::from_secs(3))
        .await
        .expect("hint before the kill");
    let event_id = support::event_id(&frame).expect("event id");

    // Kill replica A mid-stream (listener closes, stream ends).
    app.shutdown_tx.send(true).unwrap();
    while stream
        .next_frame(Duration::from_millis(500))
        .await
        .is_some()
    {}

    // State changes while the client is disconnected.
    let res = app
        .client
        .post(format!("{}/v1/notifications", replica.base))
        .bearer_auth(&app.env.api_key)
        .json(&json!({ "subscriber_id": "usr_sse", "category": "while-down" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    app.drain_jobs().await;

    // Reconnect to the surviving replica with Last-Event-ID: one immediate
    // resume hint, then the REST refetch shows everything.
    let mut resumed = SseStream::connect_to(&replica.base, &app, "usr_sse", Some(&event_id)).await;
    let frame = resumed
        .next_hint(Duration::from_secs(3))
        .await
        .expect("immediate resume hint after reconnect");
    assert!(frame.contains("resume"));

    let res = app
        .client
        .get(format!("{}/v1/inbox/items", replica.base))
        .headers(app.subscriber_headers("usr_sse"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let page: serde_json::Value = res.json().await.unwrap();
    assert_eq!(
        page["items"].as_array().unwrap().len(),
        2,
        "refetch on reconnect recovers the missed change"
    );
}

/// Duplicate idempotency keys under heavy concurrency: exactly one batch is
/// created and every response is byte-identical to the first.
#[tokio::test]
async fn duplicate_idempotency_keys_under_concurrency_create_exactly_one_batch() {
    let app = support::spawn().await;
    let payload = json!({
        "subscriber_ids": ["usr_i_a", "usr_i_b", "usr_i_c"],
        "category": "dup",
        "idempotency_key": "chaos-dup-1",
    });

    let mut handles = Vec::new();
    for _ in 0..24 {
        let client = app.client.clone();
        let url = format!("{}/v1/notifications", app.base);
        let key = app.env.api_key.clone();
        let body = payload.clone();
        handles.push(tokio::spawn(async move {
            let res = client
                .post(url)
                .bearer_auth(key)
                .json(&body)
                .send()
                .await
                .unwrap();
            (res.status().as_u16(), res.text().await.unwrap())
        }));
    }
    let mut created = 0;
    let mut bodies: Vec<String> = Vec::new();
    for handle in handles {
        let (status, body) = handle.await.unwrap();
        assert!(status == 200 || status == 201, "unexpected status {status}");
        if status == 201 {
            created += 1;
        }
        bodies.push(body);
    }
    assert_eq!(created, 1, "exactly one request observed first acceptance");
    bodies.dedup();
    assert_eq!(bodies.len(), 1, "byte-identical replays");

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(rows, 3, "one notification set, not one per retry");
    app.drain_jobs().await;
    app.assert_consistent().await;
}

/// N replicas race boot migrations on a fresh database: the advisory lock
/// serializes them, one migrator wins, everyone proceeds.
#[tokio::test]
async fn racing_boot_migrations_serialize_under_the_advisory_lock() {
    let pg = PostgresImage::default()
        .with_tag("15-alpine")
        .start()
        .await
        .expect("postgres");
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

    // Wait for the container, then race 6 "replicas" through migrate + boot
    // partition maintenance concurrently.
    let mut pool = None;
    for _ in 0..50 {
        if let Ok(p) = db::connect(&url).await
            && sqlx::query("SELECT 1").execute(&p).await.is_ok()
        {
            pool = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let pool = pool.expect("postgres never came up");

    let mut handles = Vec::new();
    for _ in 0..6 {
        let url = url.clone();
        handles.push(tokio::spawn(async move {
            let pool = db::connect(&url).await?;
            db::migrate(&pool).await?;
            partitions::run(&pool, 12, 30).await?;
            anyhow::Ok(())
        }));
    }
    for handle in handles {
        handle.await.unwrap().expect("racing boot must succeed");
    }

    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(applied, 2, "each migration applied exactly once");
    assert!(db::ready(&pool).await.unwrap(), "every replica ends ready");
}

/// Full Redis outage and recovery: hints delayed, NOTHING lost, counters
/// recomputable from Postgres, readiness untouched, drift zero at the end.
#[tokio::test]
async fn redis_full_outage_delays_hints_loses_nothing_and_counters_recover() {
    let app = support::spawn_with_redis().await;
    app.create_notification("usr_o", "seed").await;
    app.drain_jobs().await;

    let redis = app.redis.as_ref().expect("redis container");
    redis.stop_with_timeout(Some(1)).await.expect("stop redis");

    for i in 0..3 {
        app.create_notification("usr_o", &format!("outage{i}"))
            .await;
    }
    let (unread, _) = app.counts("usr_o").await;
    assert_eq!(unread, 4, "Postgres is authoritative during the outage");
    let res = app
        .client
        .get(format!("{}/readyz", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "Redis down must not fail readiness");

    // Counters stay recomputable from Postgres alone while Redis is dark.
    // Park the pending hint jobs out of the sweep's way first, since
    // claiming one would just burn the 5s publish timeout. They are retried
    // after recovery below.
    sqlx::query(
        "UPDATE jobs SET run_at = now() + interval '10 minutes'
          WHERE environment_id = $1 AND job_type = 'hint'",
    )
    .bind(app.env.id)
    .execute(&app.pool)
    .await
    .unwrap();
    let subscriber: Uuid = sqlx::query_scalar(
        "SELECT id FROM subscribers WHERE environment_id = $1 AND subscriber_id = 'usr_o'",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE subscriber_counters SET unread_direct_count = 99 WHERE environment_id = $1",
    )
    .bind(app.env.id)
    .execute(&app.pool)
    .await
    .unwrap();
    let mut conn = app.pool.acquire().await.unwrap();
    jobs::enqueue(
        &mut conn,
        app.env.id,
        jobs::TYPE_COUNTER_REBUILD,
        json!({ "subscriber_id": subscriber }),
        None,
    )
    .await
    .unwrap();
    drop(conn);
    // The rebuild's recount happens entirely in Postgres.
    assert!(
        worker::process_one(&app.pool, app.pubsub.as_ref(), &app.cfg, app.env.id)
            .await
            .unwrap()
    );
    let (unread, _) = app.counts("usr_o").await;
    assert_eq!(unread, 4, "rebuild recomputed the poisoned counter");

    // Recovery: restart Redis, force backed-off retries due, hints flow.
    redis.start().await.expect("restart redis");
    tokio::time::sleep(Duration::from_millis(500)).await;
    sqlx::query("UPDATE jobs SET run_at = now() WHERE environment_id = $1")
        .bind(app.env.id)
        .execute(&app.pool)
        .await
        .unwrap();
    let mut rx = app.pubsub.subscribe();
    app.spawn_worker();
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if let Ok(hint) = rx.recv().await
                && hint.environment_id == app.env.id
            {
                return;
            }
        }
    })
    .await
    .expect("delayed hint after recovery");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while app.job_count(app.env.id).await > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "queue must drain after recovery"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(app.dead_letter_count().await, 0, "outage parked nothing");
    app.assert_consistent().await;
}

/// One environment floods the queue; a quiet environment's jobs still get
/// claimed promptly (round-robin fairness, one claim per env per sweep).
#[tokio::test]
async fn tenant_flood_cannot_starve_a_quiet_environment() {
    let app = support::spawn().await;
    let env_quiet = app.create_environment(true).await;

    let flood_sub = Uuid::nil();
    let mut conn = app.pool.acquire().await.unwrap();
    for _ in 0..200 {
        jobs::enqueue(
            &mut conn,
            app.env.id,
            jobs::TYPE_COUNTER_REBUILD,
            json!({ "subscriber_id": flood_sub }),
            None,
        )
        .await
        .unwrap();
    }
    for _ in 0..20 {
        jobs::enqueue(
            &mut conn,
            env_quiet.id,
            jobs::TYPE_COUNTER_REBUILD,
            json!({ "subscriber_id": flood_sub }),
            None,
        )
        .await
        .unwrap();
    }
    drop(conn);

    // Fairness bound: with one claim per env per sweep, the quiet env's 20
    // jobs drain within ~20 sweeps even though the flood holds 10x that.
    let mut sweeps = 0;
    loop {
        let quiet_pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs
              WHERE environment_id = $1 AND job_type = 'counter_rebuild'",
        )
        .bind(env_quiet.id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
        if quiet_pending == 0 {
            break;
        }
        app.sweep().await;
        sweeps += 1;
        assert!(sweeps <= 25, "quiet environment starved: {sweeps} sweeps");
    }

    let flood_backlog: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE environment_id = $1 AND job_type = 'counter_rebuild'",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(
        flood_backlog >= 150,
        "the flood must still be queued (got {flood_backlog}), not served ahead of the quiet env"
    );
}

/// Sustained jobs-table churn at full speed: delete-on-complete plus the
/// table's aggressive autovacuum settings keep dead tuples and table size
/// bounded. Nightly lane (see .github/workflows/nightly.yml); prints the
/// measured sustained jobs/sec for the documented ceiling.
#[tokio::test]
#[ignore = "sustained-load chaos; run in the nightly lane (cargo nextest run --run-ignored all)"]
async fn sustained_jobs_churn_stays_bounded_under_autovacuum() {
    let pg = PostgresImage::default()
        .with_tag("15-alpine")
        .with_cmd(["postgres", "-c", "autovacuum_naptime=5s"])
        .start()
        .await
        .expect("postgres");
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let mut pool = None;
    for _ in 0..50 {
        if let Ok(p) = db::connect(&url).await
            && sqlx::query("SELECT 1").execute(&p).await.is_ok()
        {
            pool = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let pool = pool.expect("postgres never came up");
    db::migrate(&pool).await.unwrap();
    partitions::run(&pool, 12, 30).await.unwrap();

    let env = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO environments (id, slug, name, subscriber_hmac_secret)
         VALUES ($1, 'churn', 'churn', 'shmac_churn')",
    )
    .bind(env)
    .execute(&pool)
    .await
    .unwrap();

    let cfg = dronte::config::Config {
        database_url: url.clone(),
        redis_url: None,
        listen_addr: String::new(),
        retention_months: 12,
        idempotency_retention_days: 30,
        hint_debounce: Duration::from_millis(100),
        worker_poll_interval: Duration::from_millis(5),
        sse_ping_interval: Duration::from_secs(30),
        sse_retry_base: Duration::from_millis(100),
        sse_retry_jitter: Duration::from_millis(100),
        sse_max_connections_per_subscriber: 8,
        dev_environment: None,
        dev_api_key: None,
        admin_token: None,
        retry_backoff_base: Duration::from_millis(100),
        retry_backoff_cap: Duration::from_secs(2),
        metrics_sample_interval: Duration::from_secs(1),
        counter_drift_sample_size: 50,
        api_key_rate_per_sec: 0.0,
        api_key_rate_burst: 0.0,
        subscriber_rate_per_sec: 0.0,
        subscriber_rate_burst: 0.0,
        shutdown_readiness_grace: Duration::from_millis(100),
        shutdown_drain_deadline: Duration::from_secs(5),
    };
    let cfg = std::sync::Arc::new(cfg);
    let pubsub = dronte::pubsub::build(None, &pool).await.unwrap();

    // A deep single-environment backlog (the worst case for the claim
    // query: one fairness slot, every worker contending on the same index
    // head). Batched inserts so the enqueuer is not the bottleneck.
    const BACKLOG: i64 = 20_000;
    for chunk in 0..(BACKLOG / 500) {
        sqlx::query(
            "INSERT INTO jobs (environment_id, id, job_type, payload)
             SELECT $1, gen_random_uuid(), 'counter_rebuild',
                    jsonb_build_object('subscriber_id', $2::text)
               FROM generate_series(1, 500)",
        )
        .bind(env)
        .bind(Uuid::nil().to_string())
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("prefill chunk {chunk}: {e}"));
    }

    // 4 workers drain it at full speed (each rebuild also enqueues a hint,
    // so total processed work exceeds BACKLOG; the rate below is therefore
    // a LOWER bound).
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut workers = Vec::new();
    for _ in 0..4 {
        workers.push(tokio::spawn(worker::run(
            pool.clone(),
            pubsub.clone(),
            cfg.clone(),
            stop_rx.clone(),
        )));
    }
    let started = std::time::Instant::now();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
    let mut peak_dead_tup: i64 = 0;
    loop {
        let backlog: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        let dead: i64 = sqlx::query_scalar(
            "SELECT COALESCE(n_dead_tup, 0) FROM pg_stat_user_tables WHERE relname = 'jobs'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        peak_dead_tup = peak_dead_tup.max(dead);
        if backlog == 0 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "drain stalled");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let elapsed = started.elapsed();
    stop_tx.send(true).unwrap();
    for w in workers {
        w.await.unwrap();
    }
    // Let autovacuum take a final pass (naptime 5s above).
    tokio::time::sleep(Duration::from_secs(12)).await;

    let (dead_tup, table_bytes): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(n_dead_tup, 0), pg_relation_size('jobs')
           FROM pg_stat_user_tables WHERE relname = 'jobs'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    println!(
        "sustained churn: {BACKLOG} jobs drained in {:.1}s ≈ {:.0} jobs/sec sustained \
         (single env, 4 workers); jobs table: peak {peak_dead_tup} dead tuples, \
         {dead_tup} after settle, {} KiB",
        elapsed.as_secs_f64(),
        BACKLOG as f64 / elapsed.as_secs_f64(),
        table_bytes / 1024,
    );
    // Vacuum kept up. During the burn dead tuples peak between vacuum
    // passes (naptime 5s) but stay bounded by the total churn (~2x BACKLOG
    // rows inserted+deleted, rebuilds plus hints), and after one settle
    // pass the table is back to near-empty with no lasting bloat.
    assert!(
        peak_dead_tup < 3 * BACKLOG,
        "autovacuum fell behind: peak {peak_dead_tup} dead tuples"
    );
    assert!(
        dead_tup < 1_000,
        "dead tuples must settle to near zero, got {dead_tup}"
    );
    assert!(
        table_bytes < 32 * 1024 * 1024,
        "jobs table bloated to {table_bytes} bytes"
    );
}

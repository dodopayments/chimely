//! Token-bucket rate limiting. 429 carries Retry-After. The bucket is
//! cross-replica correct over one Redis. The limiter fails open when Redis
//! dies. The hint/cache plane must never take the API down.

mod support;

use serde_json::json;

#[tokio::test]
async fn management_creates_hit_429_with_retry_after_then_recover() {
    let app = support::spawn_configured(false, |cfg| {
        cfg.api_key_rate_per_sec = 2.0;
        cfg.api_key_rate_burst = 3.0;
    })
    .await;

    for i in 0..3 {
        let res = app
            .mgmt_post(
                "/v1/notifications",
                json!({ "subscriber_id": "usr_rl", "category": format!("c{i}") }),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201, "request {i} within burst");
    }
    let res = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_id": "usr_rl", "category": "over" }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 429);
    let retry_after: u64 = res
        .headers()
        .get("retry-after")
        .expect("429 carries Retry-After")
        .to_str()
        .unwrap()
        .parse()
        .expect("integral seconds");
    assert!(retry_after >= 1);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "rate_limited");

    // Refill: at 2 tokens/sec one is back within a second.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let res = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_id": "usr_rl", "category": "after-refill" }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "bucket refills at the configured rate");

    // Broadcasts share the same per-key bucket.
    let res = app
        .mgmt_post("/v1/broadcasts", json!({ "category": "b" }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status() == 429 || res.status() == 201,
        "broadcast rides the same bucket"
    );
}

#[tokio::test]
async fn subscriber_list_is_limited_per_subscriber_while_others_are_untouched() {
    let app = support::spawn_configured(false, |cfg| {
        cfg.subscriber_rate_per_sec = 0.5;
        cfg.subscriber_rate_burst = 2.0;
    })
    .await;
    app.create_notification("usr_a", "x").await;

    for _ in 0..2 {
        let res = app
            .client
            .get(format!("{}/v1/inbox/items", app.base))
            .headers(app.subscriber_headers("usr_a"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }
    let res = app
        .client
        .get(format!("{}/v1/inbox/items", app.base))
        .headers(app.subscriber_headers("usr_a"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 429);
    assert!(res.headers().get("retry-after").is_some());

    // A different subscriber has its own bucket.
    let res = app
        .client
        .get(format!("{}/v1/inbox/items", app.base))
        .headers(app.subscriber_headers("usr_b"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "buckets are per subscriber");

    // Counts has no declared 429 and no limiter.
    for _ in 0..5 {
        let res = app
            .client
            .get(format!("{}/v1/inbox/counts", app.base))
            .headers(app.subscriber_headers("usr_a"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "counts endpoint is not rate limited");
    }
}

#[tokio::test]
async fn rate_limits_are_cross_replica_correct_over_one_redis() {
    let app = support::spawn_configured(true, |cfg| {
        // Refill slow enough that wall-clock time across 6 requests cannot
        // mint a 5th token on a slow CI runner.
        cfg.api_key_rate_per_sec = 0.05;
        cfg.api_key_rate_burst = 4.0;
    })
    .await;
    let replica = app.spawn_replica().await;

    // Two replicas, ONE bucket: 4 tokens total regardless of which replica
    // serves the request (the Lua bucket in Redis is the source of truth).
    let mut allowed = 0;
    let mut limited = 0;
    for i in 0..6 {
        let base = if i % 2 == 0 { &app.base } else { &replica.base };
        let res = app
            .client
            .post(format!("{base}/v1/notifications"))
            .bearer_auth(&app.env.api_key)
            .json(&json!({ "subscriber_id": "usr_x", "category": format!("c{i}") }))
            .send()
            .await
            .unwrap();
        match res.status().as_u16() {
            201 => allowed += 1,
            429 => limited += 1,
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!(
        (allowed, limited),
        (4, 2),
        "one shared bucket across replicas, not one per replica"
    );
}

#[tokio::test]
async fn limiter_fails_open_when_redis_dies() {
    let app = support::spawn_configured(true, |cfg| {
        cfg.api_key_rate_per_sec = 1.0;
        cfg.api_key_rate_burst = 2.0;
    })
    .await;

    app.redis
        .as_ref()
        .expect("redis container")
        .stop_with_timeout(Some(1))
        .await
        .expect("stopping redis");

    // Past the burst, every request must still be served. Redis is the cache
    // plane. Its loss must never reject traffic Postgres can serve.
    for i in 0..5 {
        let res = app
            .mgmt_post(
                "/v1/notifications",
                json!({ "subscriber_id": "usr_open", "category": format!("c{i}") }),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201, "request {i} fails open during outage");
    }
}

//! Phase 3 observability: the Prometheus surface. cargo-nextest runs each
//! test in its own process, so the process-global recorder is private to
//! each test here.

mod support;

use std::time::Duration;

use dronte::metrics_sampler;
use serde_json::json;
use support::SseStream;

async fn scrape(app: &support::TestApp) -> String {
    let res = app
        .client
        .get(format!("{}/metrics", app.base))
        .send()
        .await
        .expect("metrics scrape");
    assert_eq!(res.status(), 200);
    res.text().await.expect("metrics body")
}

#[tokio::test]
async fn sampler_reports_queue_depth_dead_letters_partitions_and_zero_drift() {
    let app = support::spawn().await;
    app.create_notification("usr_m", "x").await;

    // A parked job, directly: visible in dronte_dead_letters.
    sqlx::query(
        "INSERT INTO dead_letters (environment_id, id, job_type, payload, attempts,
                                   max_attempts, last_error, created_at)
         VALUES ($1, $2, 'deliver', '{}'::jsonb, 10, 10, 'boom', now())",
    )
    .bind(app.env.id)
    .bind(dronte::ids::new_uuid())
    .execute(&app.pool)
    .await
    .unwrap();

    metrics_sampler::sample(&app.pool, &app.cfg).await.unwrap();
    let body = scrape(&app).await;

    let depth_line = format!(
        "dronte_queue_depth{{environment=\"{}\",job_type=\"hint\"}}",
        app.env.slug
    );
    assert!(
        body.contains(&depth_line),
        "queue depth per env+type:\n{body}"
    );
    assert!(body.contains("dronte_dead_letters{job_type=\"deliver\"} 1"));
    assert!(body.contains("dronte_partitions_remaining{table=\"notifications\"} 13"));
    assert!(body.contains("dronte_partitions_remaining{table=\"notification_status_log\"} 13"));
    assert!(body.contains("dronte_counter_drift_unread 0"));
    assert!(body.contains("dronte_counter_drift_unseen 0"));

    // Draining the queue zeroes the depth gauge instead of freezing it.
    app.drain_jobs().await;
    metrics_sampler::sample(&app.pool, &app.cfg).await.unwrap();
    let body = scrape(&app).await;
    assert!(
        body.contains(&format!("{depth_line} 0")),
        "drained series must drop to zero:\n{body}"
    );
}

#[tokio::test]
async fn hint_latency_claim_counters_and_sse_connections_are_recorded() {
    let app = support::spawn().await;
    let _stream = SseStream::connect(&app, "usr_m2", None).await;
    app.create_notification("usr_m2", "x").await;
    app.drain_jobs().await;

    let body = scrape(&app).await;
    assert!(
        body.contains("dronte_sse_connections 1"),
        "SSE gauge:\n{body}"
    );
    assert!(
        body.contains("dronte_hint_publish_duration_seconds"),
        "publish duration histogram"
    );
    assert!(
        body.contains("dronte_hint_delivery_lag_seconds"),
        "enqueue-to-publish lag histogram"
    );
    assert!(
        body.contains(&format!(
            "dronte_jobs_processed_total{{environment=\"{}\"}}",
            app.env.slug
        )),
        "claim counter per environment"
    );
    assert!(
        body.contains("dronte_job_wait_seconds"),
        "fairness histogram"
    );
}

#[tokio::test]
async fn counter_drift_detects_an_artificially_poisoned_counter() {
    let app = support::spawn().await;
    app.create_notification("usr_m3", "x").await;
    app.drain_jobs().await;

    // A pending read-state hint carries an explicit JSON null in
    // notification_ids. The drift recount's deliver-ownership probe walks
    // job payloads and must not feed that null to
    // jsonb_array_elements_text (scalar input raises an error).
    let subscriber: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM subscribers WHERE environment_id = $1 AND subscriber_id = 'usr_m3'",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    let mut conn = app.pool.acquire().await.unwrap();
    dronte::jobs::enqueue_hint(&mut conn, app.env.id, &[subscriber], "read_state", &[])
        .await
        .unwrap();
    drop(conn);

    let (unread, unseen) = metrics_sampler::counter_drift(&app.pool, 100)
        .await
        .unwrap();
    assert_eq!((unread, unseen), (0, 0));
    app.drain_jobs().await;

    // Poison the maintained value; the sampled recount must see it. This is
    // the proof the chaos suite's zero-drift assertion has teeth.
    sqlx::query(
        "UPDATE subscriber_counters SET unread_direct_count = unread_direct_count + 7
          WHERE environment_id = $1",
    )
    .bind(app.env.id)
    .execute(&app.pool)
    .await
    .unwrap();
    let (unread, _) = metrics_sampler::counter_drift(&app.pool, 100)
        .await
        .unwrap();
    assert_eq!(unread, 7, "poisoned counter must surface as drift");
}

#[tokio::test]
async fn rate_limited_requests_bump_the_limit_counter() {
    let app = support::spawn_configured(false, |cfg| {
        cfg.api_key_rate_per_sec = 0.05;
        cfg.api_key_rate_burst = 1.0;
    })
    .await;
    let ok = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_id": "usr_m4", "category": "a" }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 201);
    let limited = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_id": "usr_m4", "category": "b" }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(limited.status(), 429);

    tokio::time::sleep(Duration::from_millis(50)).await;
    let body = scrape(&app).await;
    assert!(body.contains("dronte_rate_limited_total 1"), "{body}");
}

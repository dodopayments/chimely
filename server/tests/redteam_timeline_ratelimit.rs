//! GET /v1/notifications/{id}/timeline is a management-plane endpoint and must
//! enforce the per-API-key token bucket via enforce_api_key_limit(). It was the
//! one management endpoint that lacked that call. The 429 carries Retry-After
//! and error.code = "rate_limited".

mod support;

#[tokio::test]
async fn timeline_reads_draw_down_the_api_key_bucket_and_429_past_the_burst() {
    // burst = 2 covers the create and the first timeline read. The near-zero
    // refill rate cannot mint a replacement token within the test's wall-clock.
    let app = support::spawn_configured(false, |cfg| {
        cfg.api_key_rate_per_sec = 0.01;
        cfg.api_key_rate_burst = 2.0;
    })
    .await;

    let body = app.create_notification("usr_tl", "x").await;
    let id = body["notifications"][0]["id"]
        .as_str()
        .expect("notification id")
        .to_owned();

    let res = app.timeline_api(&id).await;
    assert_eq!(res.status(), 200, "first timeline read is within the burst");

    let res = app.timeline_api(&id).await;
    assert_eq!(
        res.status(),
        429,
        "a timeline read past the api-key burst must be rate limited, \
         not served unmetered"
    );

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
}

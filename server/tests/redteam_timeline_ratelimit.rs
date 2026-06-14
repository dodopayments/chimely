//! RED-TEAM regression guard (REAL handler): GET
//! /v1/notifications/{id}/timeline is a management-plane endpoint and must
//! enforce the per-API-key token bucket like every other management call.
//! get_notification_timeline was the only management endpoint that lacked the
//! enforce_api_key_limit() call. Reverting that one line leaves the timeline
//! read unmetered: the bucket never drains through it, the 429 below never
//! arrives, and this test goes red.
//!
//! Contract: the 429 carries Retry-After and error.code = "rate_limited"
//! (specs/openapi.yaml RateLimited).

mod support;

#[tokio::test]
async fn timeline_reads_draw_down_the_api_key_bucket_and_429_past_the_burst() {
    // burst = 2: the create below spends one token and the first timeline read
    // spends the second, so the next timeline read finds the bucket empty. The
    // near-zero refill rate means no wall-clock during the test can mint a
    // replacement token.
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

    // First read is served within the burst: proof the endpoint works and is
    // metered by the same bucket the create just drew from.
    let res = app.timeline_api(&id).await;
    assert_eq!(res.status(), 200, "first timeline read is within the burst");

    // Bucket now empty (create + one read = the burst of 2). The next read on
    // the SAME api key must be refused.
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

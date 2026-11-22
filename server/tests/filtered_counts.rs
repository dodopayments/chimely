//! Exact category-filtered unread counts across both inbox sources.

mod support;

use chrono::{Duration, Utc};
use serde_json::{Value, json};
use support::{TestApp, TestEnvironment};

const SUB: &str = "usr_filtered_counts";

async fn filtered_counts(app: &TestApp, subscriber: &str, filters: Value) -> Vec<i64> {
    filtered_counts_in(app, &app.env, subscriber, filters).await
}

async fn filtered_counts_in(
    app: &TestApp,
    env: &TestEnvironment,
    subscriber: &str,
    filters: Value,
) -> Vec<i64> {
    let res = app
        .client
        .post(format!("{}/v1/inbox/counts", app.base))
        .headers(app.subscriber_headers_for(env, subscriber))
        .json(&json!({ "filters": filters }))
        .send()
        .await
        .expect("filtered counts request");
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    let body: Value = res.json().await.expect("filtered counts body");
    body["counts"]
        .as_array()
        .expect("counts array")
        .iter()
        .map(|count| count["unread"].as_i64().expect("unread count"))
        .collect()
}

async fn create_notification_in(
    app: &TestApp,
    env: &TestEnvironment,
    subscriber: &str,
    category: &str,
) {
    let res = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .bearer_auth(&env.api_key)
        .json(&json!({ "subscriber_id": subscriber, "category": category }))
        .send()
        .await
        .expect("create notification");
    assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
}

async fn create_broadcast_in(app: &TestApp, env: &TestEnvironment, category: &str) {
    let res = app
        .client
        .post(format!("{}/v1/broadcasts", app.base))
        .bearer_auth(&env.api_key)
        .json(&json!({ "category": category }))
        .send()
        .await
        .expect("create broadcast");
    assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
}

async fn set_both_item_states(app: &TestApp, direct_id: &str, broadcast_id: &str, state: &str) {
    for path in [
        format!("/v1/inbox/notifications/{direct_id}/{state}"),
        format!("/v1/inbox/broadcasts/{broadcast_id}/{state}"),
    ] {
        assert_eq!(app.post_inbox(SUB, &path).await.status(), 204);
    }
}

async fn set_category_enabled(app: &TestApp, category: &str, enabled: bool) {
    let res = app
        .client
        .put(format!("{}/v1/inbox/preferences", app.base))
        .headers(app.subscriber_headers(SUB))
        .json(&json!({
            "preferences": [{
                "category": category,
                "channel": "in_app",
                "enabled": enabled
            }]
        }))
        .send()
        .await
        .expect("set category preference");
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn category_filters_are_exact_batched_and_ordered() {
    let app = support::spawn().await;
    app.create_notification(SUB, "billing").await;
    app.create_notification(SUB, "system").await;
    app.create_notification(SUB, "billing.detail").await;
    app.create_broadcast("billing").await;
    app.create_broadcast("refund").await;
    let scheduled = app
        .mgmt_post(
            "/v1/notifications",
            json!({
                "subscriber_id": SUB,
                "category": "future",
                "deliver_at": (Utc::now() + Duration::hours(1)).to_rfc3339()
            }),
        )
        .send()
        .await
        .expect("create scheduled notification");
    assert_eq!(scheduled.status(), 201);

    let counts = filtered_counts(
        &app,
        SUB,
        json!([
            { "categories": ["billing"] },
            { "categories": ["billing", "refund"] },
            { "categories": ["missing"] },
            { "categories": ["billing", "billing"] },
            { "categories": ["future"] }
        ]),
    )
    .await;

    assert_eq!(counts, [2, 3, 0, 2, 0]);
}

#[tokio::test]
async fn filtered_counts_follow_read_archive_and_mute_state_for_both_sources() {
    let app = support::spawn().await;
    let direct = app.create_notification(SUB, "target.direct").await;
    let broadcast = app.create_broadcast("target.broadcast").await;
    let direct_id = direct["notifications"][0]["id"].as_str().unwrap();
    let broadcast_id = broadcast["id"].as_str().unwrap();
    let filters = json!([
        { "categories": ["target.direct"] },
        { "categories": ["target.broadcast"] },
        { "categories": ["target.direct", "target.broadcast"] }
    ]);

    assert_eq!(filtered_counts(&app, SUB, filters.clone()).await, [1, 1, 2]);

    set_both_item_states(&app, direct_id, broadcast_id, "read").await;
    assert_eq!(filtered_counts(&app, SUB, filters.clone()).await, [0, 0, 0]);

    set_both_item_states(&app, direct_id, broadcast_id, "unread").await;
    assert_eq!(filtered_counts(&app, SUB, filters.clone()).await, [1, 1, 2]);

    assert_eq!(
        app.post_inbox(SUB, "/v1/inbox/read-all").await.status(),
        200
    );
    assert_eq!(filtered_counts(&app, SUB, filters.clone()).await, [0, 0, 0]);

    set_both_item_states(&app, direct_id, broadcast_id, "unread").await;
    assert_eq!(
        filtered_counts(&app, SUB, filters.clone()).await,
        [1, 1, 2],
        "explicit unread exceptions below the watermark count"
    );

    set_both_item_states(&app, direct_id, broadcast_id, "archive").await;
    assert_eq!(filtered_counts(&app, SUB, filters.clone()).await, [0, 0, 0]);

    set_both_item_states(&app, direct_id, broadcast_id, "unarchive").await;
    assert_eq!(filtered_counts(&app, SUB, filters.clone()).await, [1, 1, 2]);

    assert_eq!(
        app.post_inbox(SUB, "/v1/inbox/archive-all").await.status(),
        200
    );
    assert_eq!(filtered_counts(&app, SUB, filters.clone()).await, [0, 0, 0]);

    set_both_item_states(&app, direct_id, broadcast_id, "unarchive").await;
    assert_eq!(
        filtered_counts(&app, SUB, filters.clone()).await,
        [1, 1, 2],
        "explicit unarchive exceptions below the watermark count"
    );

    set_category_enabled(&app, "target.direct", false).await;
    set_category_enabled(&app, "target.broadcast", false).await;
    assert_eq!(filtered_counts(&app, SUB, filters.clone()).await, [0, 0, 0]);

    set_category_enabled(&app, "target.direct", true).await;
    assert_eq!(filtered_counts(&app, SUB, filters).await, [1, 0, 1]);
}

#[tokio::test]
async fn filtered_counts_respect_broadcast_visibility_and_environment_isolation() {
    let app = support::spawn().await;
    app.create_broadcast("shared.category").await;

    let filter = json!([{ "categories": ["shared.category"] }]);
    assert_eq!(
        filtered_counts(&app, SUB, filter.clone()).await,
        [0],
        "broadcasts from before subscriber creation stay invisible"
    );
    app.create_broadcast("shared.category").await;
    assert_eq!(filtered_counts(&app, SUB, filter.clone()).await, [1]);

    let env_b = app.create_environment(true).await;
    create_notification_in(&app, &env_b, SUB, "shared.category").await;
    create_broadcast_in(&app, &env_b, "shared.category").await;
    assert_eq!(
        filtered_counts_in(&app, &env_b, SUB, filter.clone()).await,
        [2]
    );
    assert_eq!(
        filtered_counts(&app, SUB, filter).await,
        [1],
        "rows from another environment never contribute"
    );
}

#[tokio::test]
async fn filtered_count_validation_is_bounded_and_declared() {
    let app = support::spawn().await;

    let res = app
        .client
        .post(format!("{}/v1/inbox/counts", app.base))
        .headers(app.subscriber_headers(SUB))
        .json(&json!({ "filters": [] }))
        .send()
        .await
        .expect("invalid filtered counts request");
    assert_eq!(res.status(), 400);
    let error: Value = res.json().await.expect("error body");
    assert_eq!(error["error"]["code"], "invalid_request");

    let doc = chimely::openapi::api_doc();
    let post = doc
        .paths
        .paths
        .get("/v1/inbox/counts")
        .expect("counts path")
        .post
        .as_ref()
        .expect("filtered counts operation");
    assert!(post.responses.responses.contains_key("400"));
}

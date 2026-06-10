//! Task 2: POST /v1/notifications and /v1/broadcasts — transactional outbox,
//! idempotency (byte-identical replay), deliver_at, validation, subscriber
//! upsert.

mod support;

use chrono::{Months, Utc};
use serde_json::json;

#[tokio::test]
async fn create_notification_commits_rows_counters_and_outbox_job_together() {
    let app = support::spawn().await;
    let body = app.create_notification("usr_1", "payment.failed").await;
    assert!(body["idempotency_key"].as_str().unwrap().len() > 4);
    let id = body["notifications"][0]["id"].as_str().unwrap();
    assert!(id.starts_with("notif_"), "{id}");

    let notif_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(notif_count, 1);

    // Counters bumped in the same transaction (conditional increment).
    let (unread, unseen, _) = app.counter_row("usr_1").await;
    assert_eq!((unread, unseen), (1, 1));

    // The outbox hint job committed with the insert — no dual writes.
    let (job_type, payload): (String, serde_json::Value) =
        sqlx::query_as("SELECT job_type, payload FROM jobs WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(job_type, "hint");
    assert_eq!(payload["reason"], "notification");
}

#[tokio::test]
async fn batch_create_fans_out_one_row_per_recipient() {
    let app = support::spawn().await;
    let recipients: Vec<String> = (0..50).map(|i| format!("usr_{i}")).collect();
    let res = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_ids": recipients, "category": "bulk.test" }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["notifications"].as_array().unwrap().len(), 50);

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(rows, 50);
    let (unread, _, _) = app.counter_row("usr_7").await;
    assert_eq!(unread, 1);
}

#[tokio::test]
async fn idempotent_replay_is_byte_identical_and_never_reruns_the_batch() {
    let app = support::spawn().await;
    let req = json!({
        "subscriber_ids": ["usr_a", "usr_b"],
        "category": "payment.failed",
        "payload": { "title": "Payment failed", "amount": 42 },
        "idempotency_key": "order-123",
    });

    let first = app
        .mgmt_post("/v1/notifications", req.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 201);
    let first_bytes = first.bytes().await.unwrap();

    let replay = app
        .mgmt_post("/v1/notifications", req.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200, "replay must be 200");
    let replay_bytes = replay.bytes().await.unwrap();
    assert_eq!(first_bytes, replay_bytes, "replay must be byte-identical");

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(rows, 2, "the batch must not re-run");
    let (unread, _, _) = app.counter_row("usr_a").await;
    assert_eq!(unread, 1, "counters must not double-bump");
}

#[tokio::test]
async fn concurrent_same_key_requests_create_exactly_one_batch() {
    let app = support::spawn().await;
    let req = json!({
        "subscriber_id": "usr_race",
        "category": "race.test",
        "idempotency_key": "race-key",
    });
    let mut handles = Vec::new();
    for _ in 0..8 {
        let app_client = app.client.clone();
        let base = app.base.clone();
        let key = app.env.api_key.clone();
        let req = req.clone();
        handles.push(tokio::spawn(async move {
            let res = app_client
                .post(format!("{base}/v1/notifications"))
                .bearer_auth(key)
                .json(&req)
                .send()
                .await
                .unwrap();
            (res.status().as_u16(), res.bytes().await.unwrap().to_vec())
        }));
    }
    let results: Vec<(u16, Vec<u8>)> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    let created = results.iter().filter(|(s, _)| *s == 201).count();
    let replayed = results.iter().filter(|(s, _)| *s == 200).count();
    assert_eq!(created, 1, "exactly one writer wins");
    assert_eq!(replayed, 7);
    let first = &results[0].1;
    assert!(
        results.iter().all(|(_, b)| b == first),
        "all responses byte-identical"
    );

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn validation_rejects_malformed_creates() {
    let app = support::spawn().await;
    let cases = [
        json!({ "category": "x" }), // no recipients
        json!({ "subscriber_id": "a", "subscriber_ids": ["b"], "category": "x" }), // both
        json!({ "subscriber_ids": [], "category": "x" }), // empty
        json!({ "subscriber_ids": (0..101).map(|i| i.to_string()).collect::<Vec<_>>(), "category": "x" }),
        json!({ "subscriber_id": "a", "category": "" }), // empty category
        json!({ "subscriber_id": "a", "category": "c".repeat(256) }),
        json!({ "subscriber_id": "a", "category": "x", "payload": [1, 2] }), // non-object payload
        json!({ "subscriber_id": "a", "category": "x",
                "payload": { "blob": "y".repeat(17 * 1024) } }), // > 16 KiB
        json!({ "subscriber_id": "a", "category": "x",
                "deliver_at": (Utc::now() - chrono::Duration::hours(1)).to_rfc3339() }),
        json!({ "subscriber_id": "a", "category": "x",
                "deliver_at": (Utc::now() + Months::new(14)).to_rfc3339() }),
        json!({ "subscriber_id": "a", "category": "x", "idempotency_key": "k".repeat(256) }),
    ];
    for case in cases {
        let res = app
            .mgmt_post("/v1/notifications", case.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400, "expected 400 for {case}");
        let body: serde_json::Value = res.json().await.unwrap();
        assert_eq!(
            body["error"]["code"], "invalid_request",
            "error envelope for {case}"
        );
    }
    // Nothing leaked into the tables.
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(rows, 0);
}

/// S6 regression: idempotent replay must win over wall-clock validation. A
/// retry whose deliver_at has passed in the meantime still gets the snapshot.
#[tokio::test]
async fn replay_returns_the_snapshot_even_after_deliver_at_has_passed() {
    let app = support::spawn().await;
    let req = json!({
        "subscriber_id": "usr_late",
        "category": "x",
        "idempotency_key": "late-retry",
        "deliver_at": (Utc::now() + chrono::Duration::milliseconds(700)).to_rfc3339(),
    });
    let first = app
        .mgmt_post("/v1/notifications", req.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 201);
    let first_bytes = first.bytes().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(900)).await; // deliver_at is now past
    let replay = app
        .mgmt_post("/v1/notifications", req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        replay.status(),
        200,
        "replay must not re-validate deliver_at"
    );
    assert_eq!(replay.bytes().await.unwrap(), first_bytes);
}

/// S5 regression: extractor rejections must use the contract's error
/// envelope, never axum's plain-text 400/415/422.
#[tokio::test]
async fn malformed_bodies_get_the_error_envelope() {
    let app = support::spawn().await;
    let res = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .bearer_auth(&app.env.api_key)
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.expect("envelope body");
    assert_eq!(body["error"]["code"], "invalid_request");

    // Wrong content type is a 400 envelope too (the contract has no 415).
    let res = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .bearer_auth(&app.env.api_key)
        .header("content-type", "text/plain")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.expect("envelope body");
    assert_eq!(body["error"]["code"], "invalid_request");

    // Malformed query parameters on the subscriber plane as well.
    let res = app
        .client
        .get(format!("{}/v1/inbox/items?limit=banana", app.base))
        .headers(app.subscriber_headers("usr_q"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.expect("envelope body");
    assert_eq!(body["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn management_plane_requires_a_valid_bearer_key() {
    let app = support::spawn().await;
    let body = json!({ "subscriber_id": "u", "category": "x" });

    let res = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401, "missing key");

    let res = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .bearer_auth("drnt_test_wrong")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401, "unknown key");

    // Revoked keys stop working.
    sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE environment_id = $1")
        .bind(app.env.id)
        .execute(&app.pool)
        .await
        .unwrap();
    let res = app
        .mgmt_post("/v1/notifications", body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401, "revoked key");
}

#[tokio::test]
async fn scheduled_creates_are_durable_but_invisible_and_uncounted() {
    let app = support::spawn().await;
    let deliver_at = Utc::now() + chrono::Duration::hours(2);
    let res = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_id": "usr_s", "category": "digest",
                    "deliver_at": deliver_at.to_rfc3339() }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);

    // Durable immediately…
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(rows, 1);
    // …but invisible to the subscriber and NOT counted at create.
    let items = app.list_all_items("usr_s", 20).await;
    assert!(items.is_empty(), "scheduled item leaked into the list");
    let (unread, unseen) = app.counts("usr_s").await;
    assert_eq!((unread, unseen), (0, 0));

    // The deliver job rides the outbox, scheduled for deliver_at.
    let (job_type, run_at): (String, chrono::DateTime<Utc>) =
        sqlx::query_as("SELECT job_type, run_at FROM jobs WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(job_type, "deliver");
    assert_eq!(run_at.timestamp(), deliver_at.timestamp());
}

#[tokio::test]
async fn broadcast_create_is_one_row_with_idempotent_replay() {
    let app = support::spawn().await;
    let req = json!({
        "category": "product.update",
        "payload": { "title": "v2 launched" },
        "idempotency_key": "launch-1",
    });
    let first = app
        .mgmt_post("/v1/broadcasts", req.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 201);
    let first_bytes = first.bytes().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&first_bytes).unwrap();
    assert!(body["id"].as_str().unwrap().starts_with("bcast_"));
    assert_eq!(body["idempotency_key"], "launch-1");
    assert!(body["created_at"].as_str().is_some());

    let replay = app.mgmt_post("/v1/broadcasts", req).send().await.unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.bytes().await.unwrap(), first_bytes);

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM broadcasts WHERE environment_id = $1")
        .bind(app.env.id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "one row per announcement, never materialized");

    // Env-wide hint job (subscriber_ids null), regardless of subscriber count.
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE environment_id = $1 AND job_type = 'hint'",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(payload["subscriber_ids"].is_null());
}

#[tokio::test]
async fn subscriber_upsert_backdates_on_first_create_only() {
    let app = support::spawn().await;
    let backdate = "2020-06-01T00:00:00Z";
    let res = app
        .client
        .put(format!("{}/v1/subscribers/usr_import", app.base))
        .bearer_auth(&app.env.api_key)
        .json(&json!({ "created_at": backdate }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["subscriber_id"], "usr_import");
    assert!(
        body["created_at"]
            .as_str()
            .unwrap()
            .starts_with("2020-06-01")
    );

    // Second upsert with a different backdate: ignored.
    let res = app
        .client
        .put(format!("{}/v1/subscribers/usr_import", app.base))
        .bearer_auth(&app.env.api_key)
        .json(&json!({ "created_at": "2024-01-01T00:00:00Z" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(
        body["created_at"]
            .as_str()
            .unwrap()
            .starts_with("2020-06-01")
    );

    // Body is optional.
    let res = app
        .client
        .put(format!("{}/v1/subscribers/usr_nobody", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn environments_are_hard_isolated() {
    let app = support::spawn().await;
    let other = app.create_environment(true).await;

    app.create_notification("usr_shared", "x").await;

    // The other environment's key sees nothing of this environment.
    let res = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .bearer_auth(&other.api_key)
        .json(&json!({ "subscriber_id": "usr_shared", "category": "y" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);

    let per_env: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "SELECT environment_id, count(*) FROM notifications GROUP BY environment_id",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(per_env.len(), 2);
    assert!(per_env.iter().all(|(_, n)| *n == 1));
}

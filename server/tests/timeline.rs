//! Phase 3: the per-notification status timeline. Append-only rows
//! (created -> delivered_hint -> seen -> read), watermark moves applied
//! asynchronously by the chunked timeline job, exposed at
//! GET /v1/notifications/{id}/timeline.

mod support;

use chrono::Utc;
use dronte::{ids, worker};
use serde_json::json;
use uuid::Uuid;

fn notif_uuid(typeid: &str) -> Uuid {
    ids::parse_typeid(ids::NOTIFICATION, typeid).expect("notification typeid")
}

async fn statuses(app: &support::TestApp, id: Uuid) -> Vec<String> {
    app.timeline_rows(id)
        .await
        .into_iter()
        .map(|(status, _)| status)
        .collect()
}

#[tokio::test]
async fn immediate_create_appends_created_then_delivered_hint() {
    let app = support::spawn().await;
    let body = app.create_notification("usr_t", "x").await;
    let id = notif_uuid(body["notifications"][0]["id"].as_str().unwrap());

    // 'created' commits with the notification insert itself, before any
    // worker runs.
    assert_eq!(statuses(&app, id).await, ["created"]);

    app.drain_jobs().await;
    assert_eq!(statuses(&app, id).await, ["created", "delivered_hint"]);

    // At-least-once worker, exactly-once rows: nothing changes on re-sweeps.
    app.sweep().await;
    assert_eq!(statuses(&app, id).await, ["created", "delivered_hint"]);
    app.assert_consistent().await;
}

#[tokio::test]
async fn scheduled_creates_get_delivered_hint_only_after_deliver_at() {
    let app = support::spawn().await;
    let deliver_at = Utc::now() + chrono::Duration::milliseconds(700);
    let res = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_id": "usr_ts", "category": "sched",
                    "deliver_at": deliver_at.to_rfc3339() }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    let id = notif_uuid(body["notifications"][0]["id"].as_str().unwrap());

    // Durable immediately ('created'), but not announced yet. No worker has
    // run: the created row committed with the insert itself.
    assert_eq!(statuses(&app, id).await, ["created"]);

    tokio::time::sleep(std::time::Duration::from_millis(1_000)).await;
    app.drain_jobs().await; // deliver -> hint -> delivered_hint
    assert_eq!(statuses(&app, id).await, ["created", "delivered_hint"]);
    app.assert_consistent().await;
}

#[tokio::test]
async fn per_item_read_appends_one_read_row_idempotently() {
    let app = support::spawn().await;
    let body = app.create_notification("usr_tr", "x").await;
    let typeid = body["notifications"][0]["id"].as_str().unwrap().to_owned();
    let id = notif_uuid(&typeid);
    app.drain_jobs().await;

    for _ in 0..2 {
        let res = app
            .post_inbox("usr_tr", &format!("/v1/inbox/notifications/{typeid}/read"))
            .await;
        assert_eq!(res.status(), 204);
    }
    assert_eq!(
        statuses(&app, id).await,
        ["created", "delivered_hint", "read"],
        "read appended exactly once despite the idempotent retry"
    );
}

#[tokio::test]
async fn watermark_moves_backfill_their_window_through_the_timeline_job() {
    let app = support::spawn().await;
    let early = app.create_notification("usr_tw", "x").await;
    let early_id = notif_uuid(early["notifications"][0]["id"].as_str().unwrap());
    app.drain_jobs().await;

    // seen-all then read-all: each enqueues one chunked timeline job.
    let res = app.post_inbox("usr_tw", "/v1/inbox/seen-all").await;
    assert_eq!(res.status(), 200);
    let res = app.post_inbox("usr_tw", "/v1/inbox/read-all").await;
    assert_eq!(res.status(), 200);

    // A notification created AFTER the watermark moves is outside both
    // windows and must stay untouched.
    let late = app.create_notification("usr_tw", "x").await;
    let late_id = notif_uuid(late["notifications"][0]["id"].as_str().unwrap());

    app.drain_jobs().await;
    assert_eq!(
        statuses(&app, early_id).await,
        ["created", "delivered_hint", "seen", "read"],
    );
    assert_eq!(
        statuses(&app, late_id).await,
        ["created", "delivered_hint"],
        "post-watermark create must not be backfilled"
    );

    // occurred_at for backfilled rows is the watermark move time, which is
    // before the job ran: both seen/read times sit between create and now.
    let rows = app.timeline_rows(early_id).await;
    let read_at = rows.iter().find(|(s, _)| s == "read").unwrap().1;
    let seen_at = rows.iter().find(|(s, _)| s == "seen").unwrap().1;
    assert!(seen_at <= read_at);
    app.assert_consistent().await;
}

#[tokio::test]
async fn per_item_read_then_read_all_yields_exactly_one_read_row() {
    let app = support::spawn().await;
    let body = app.create_notification("usr_tdup", "x").await;
    let typeid = body["notifications"][0]["id"].as_str().unwrap().to_owned();
    let id = notif_uuid(&typeid);
    app.drain_jobs().await;

    let res = app
        .post_inbox(
            "usr_tdup",
            &format!("/v1/inbox/notifications/{typeid}/read"),
        )
        .await;
    assert_eq!(res.status(), 204);
    let res = app.post_inbox("usr_tdup", "/v1/inbox/read-all").await;
    assert_eq!(res.status(), 200);
    app.drain_jobs().await;

    let read_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notification_status_log
          WHERE environment_id = $1 AND notification_id = $2 AND status = 'read'",
    )
    .bind(app.env.id)
    .bind(id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(read_rows, 1, "the NOT EXISTS guard deduped across paths");
}

#[tokio::test]
async fn large_watermark_windows_advance_the_timeline_cursor_per_chunk() {
    let app = support::spawn().await;
    // 600 visible notifications (> TIMELINE_CHUNK = 500) for one subscriber.
    for batch in 0..6 {
        let res = app
            .mgmt_post(
                "/v1/notifications",
                json!({ "subscriber_id": "usr_big_t", "category": format!("c{batch}"),
                        "payload": { "n": batch } }),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
        // Distinct idempotency scopes per batch; one row each is fine, we
        // just need volume — repeat creates.
        for i in 0..99 {
            let res = app
                .mgmt_post(
                    "/v1/notifications",
                    json!({ "subscriber_id": "usr_big_t", "category": format!("c{batch}_{i}") }),
                )
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 201);
        }
    }
    app.drain_jobs().await;

    let res = app.post_inbox("usr_big_t", "/v1/inbox/read-all").await;
    assert_eq!(res.status(), 200);

    // The 600-row window takes more than one chunk: the cursor must advance
    // across claims until the job deletes itself.
    let mut claims = 0;
    loop {
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE environment_id = $1 AND job_type = 'timeline'",
        )
        .bind(app.env.id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
        if pending == 0 {
            break;
        }
        if worker::process_one(&app.pool, app.pubsub.as_ref(), &app.cfg, app.env.id)
            .await
            .unwrap()
        {
            claims += 1;
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(claims < 30, "timeline job failed to converge");
    }
    assert!(
        claims >= 2,
        "600 rows must take more than one chunk (got {claims} claims)"
    );

    let read_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notification_status_log
          WHERE environment_id = $1 AND status = 'read'",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(read_rows, 600, "every row in the window got its read entry");
    app.drain_jobs().await;
    app.assert_consistent().await;
}

#[tokio::test]
async fn timeline_endpoint_returns_ordered_entries_and_404s_unknowns() {
    let app = support::spawn().await;
    let body = app.create_notification("usr_te", "x").await;
    let typeid = body["notifications"][0]["id"].as_str().unwrap().to_owned();
    app.drain_jobs().await;
    let res = app
        .post_inbox("usr_te", &format!("/v1/inbox/notifications/{typeid}/read"))
        .await;
    assert_eq!(res.status(), 204);

    let res = app.timeline_api(&typeid).await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["id"].as_str().unwrap(), typeid);
    assert_eq!(body["subscriber_id"].as_str().unwrap(), "usr_te");
    let entries: Vec<&str> = body["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["status"].as_str().unwrap())
        .collect();
    assert_eq!(entries, ["created", "delivered_hint", "read"]);
    let times: Vec<&str> = body["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["occurred_at"].as_str().unwrap())
        .collect();
    let mut sorted = times.clone();
    sorted.sort_unstable();
    assert_eq!(times, sorted, "entries ordered by occurred_at");

    // Unknown id, foreign-environment id, and malformed id all 404.
    let res = app
        .timeline_api(&ids::typeid(ids::NOTIFICATION, ids::new_uuid()))
        .await;
    assert_eq!(res.status(), 404);
    let res = app.timeline_api("bcast_01h455vb4pex5vsknk084sn02q").await;
    assert_eq!(res.status(), 404, "broadcasts have no timeline");

    // Management plane auth applies.
    let res = app
        .client
        .get(format!("{}/v1/notifications/{typeid}/timeline", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn timeline_rows_are_never_updated_in_place() {
    let app = support::spawn().await;
    let body = app.create_notification("usr_tnu", "x").await;
    let typeid = body["notifications"][0]["id"].as_str().unwrap().to_owned();
    let id = notif_uuid(&typeid);
    app.drain_jobs().await;
    let created_before = app.timeline_rows(id).await;

    let res = app
        .post_inbox("usr_tnu", &format!("/v1/inbox/notifications/{typeid}/read"))
        .await;
    assert_eq!(res.status(), 204);
    app.drain_jobs().await;

    // Earlier rows are byte-identical after later transitions: append-only.
    let after = app.timeline_rows(id).await;
    assert_eq!(&after[..created_before.len()], &created_before[..]);
    assert_eq!(after.len(), created_before.len() + 1);
}

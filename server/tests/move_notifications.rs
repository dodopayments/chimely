//! Management endpoints that list the stored notifications of one subscriber
//! and move them to another subscriber. A move keeps the read and archive
//! state of each notification and keeps the counters exact. The seen state of
//! the target does not change.

mod support;

use std::collections::HashMap;

use serde_json::{Value, json};

const SRC: &str = "usr_move_src";
const DST: &str = "usr_move_dst";

fn notif_id(created: &Value) -> String {
    created["notifications"][0]["id"]
        .as_str()
        .expect("notification id")
        .to_owned()
}

async fn move_ids(
    app: &support::TestApp,
    from: &str,
    to: &str,
    ids: &[String],
) -> reqwest::Response {
    app.mgmt_post(
        &format!("/v1/subscribers/{from}/notifications/move"),
        json!({ "to_subscriber_id": to, "notification_ids": ids }),
    )
    .send()
    .await
    .expect("move request")
}

async fn moved(app: &support::TestApp, from: &str, to: &str, ids: &[String]) -> i64 {
    let res = move_ids(app, from, to, ids).await;
    assert_eq!(res.status(), 200, "move failed");
    let body: Value = res.json().await.expect("move body");
    body["moved"].as_i64().expect("moved count")
}

async fn list_page(app: &support::TestApp, sub: &str, query: &str) -> reqwest::Response {
    app.client
        .get(format!(
            "{}/v1/subscribers/{sub}/notifications{query}",
            app.base
        ))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .expect("list request")
}

/// `(read, archived)` per notification id, from the subscriber's own inbox.
async fn state_of(app: &support::TestApp, sub: &str) -> HashMap<String, (bool, bool)> {
    app.list_all_items(sub, 100)
        .await
        .into_iter()
        .map(|item| {
            (
                item["id"].as_str().unwrap().to_owned(),
                (
                    item["read"].as_bool().unwrap(),
                    item["archived"].as_bool().unwrap(),
                ),
            )
        })
        .collect()
}

#[tokio::test]
async fn move_keeps_read_and_archive_state() {
    let app = support::spawn().await;

    // A target notice older than every source notice. The target never opens
    // its inbox, so this notice stays unseen.
    let dst_older = notif_id(&app.create_notification(DST, "c").await);

    // Archived by the archive watermark.
    let archived = notif_id(&app.create_notification(SRC, "c").await);
    assert!(
        app.post_inbox(SRC, "/v1/inbox/archive-all")
            .await
            .status()
            .is_success()
    );
    // Read by the read watermark, and unread by an override below it.
    let by_watermark = notif_id(&app.create_notification(SRC, "c").await);
    let by_override = notif_id(&app.create_notification(SRC, "c").await);
    assert!(
        app.post_inbox(SRC, "/v1/inbox/read-all")
            .await
            .status()
            .is_success()
    );
    assert!(
        app.post_inbox(
            SRC,
            &format!("/v1/inbox/notifications/{by_override}/unread"),
        )
        .await
        .status()
        .is_success()
    );
    // Read by item, above the watermark.
    let by_item = notif_id(&app.create_notification(SRC, "c").await);
    assert!(
        app.post_inbox(SRC, &format!("/v1/inbox/notifications/{by_item}/read"))
            .await
            .status()
            .is_success()
    );
    // Plain unread.
    let fresh = notif_id(&app.create_notification(SRC, "c").await);
    // The source opens its inbox, so every source notice is seen.
    assert!(
        app.post_inbox(SRC, "/v1/inbox/seen-all")
            .await
            .status()
            .is_success()
    );
    app.drain_jobs().await;

    // The target reads everything it has after every source notice exists, so
    // its read watermark is above all of them.
    let dst_own = notif_id(&app.create_notification(DST, "c").await);
    assert!(
        app.post_inbox(DST, "/v1/inbox/read-all")
            .await
            .status()
            .is_success()
    );
    app.drain_jobs().await;

    let before = state_of(&app, SRC).await;
    let src_counts = app.counts(SRC).await;
    assert_eq!(src_counts.0, 2, "by_override and fresh are unread");
    assert_eq!(src_counts.1, 0, "the source saw every notice");
    let dst_before = app.counts(DST).await;
    assert_eq!(dst_before, (0, 2), "dst_older and dst_own are unseen");

    let all = vec![
        archived.clone(),
        by_watermark.clone(),
        by_override.clone(),
        by_item.clone(),
        fresh.clone(),
    ];
    assert_eq!(moved(&app, SRC, DST, &all).await, 5);
    app.drain_jobs().await;

    let after = state_of(&app, DST).await;
    for id in &all {
        assert_eq!(after.get(id), before.get(id), "state of {id} changed");
    }
    assert_eq!(after.get(&dst_own), Some(&(true, false)));
    assert_eq!(after.get(&dst_older), Some(&(true, false)));
    // The target seen_watermark does not move: the target's own unseen notices
    // stay unseen, and the five moved notices show as unseen.
    assert_eq!(app.counts(DST).await.1, dst_before.1 + 5);
    assert!(state_of(&app, SRC).await.is_empty());
    assert_eq!(app.counts(DST).await.0, src_counts.0);
    assert_eq!(app.counts(SRC).await, (0, 0));
    app.assert_consistent().await;

    // The per-item deltas stay exact on the moved rows.
    assert!(
        app.post_inbox(DST, &format!("/v1/inbox/notifications/{by_override}/read"))
            .await
            .status()
            .is_success()
    );
    assert_eq!(app.counts(DST).await.0, src_counts.0 - 1);
    assert!(
        app.post_inbox(
            DST,
            &format!("/v1/inbox/notifications/{by_override}/unread"),
        )
        .await
        .status()
        .is_success()
    );
    assert_eq!(app.counts(DST).await.0, src_counts.0);
    app.drain_jobs().await;
    app.assert_consistent().await;
}

#[tokio::test]
async fn move_is_idempotent_and_ignores_foreign_ids() {
    let app = support::spawn().await;
    let mine = notif_id(&app.create_notification(SRC, "c").await);
    let foreign = notif_id(&app.create_notification("usr_third", "c").await);

    let ids = vec![mine.clone(), foreign.clone()];
    assert_eq!(moved(&app, SRC, DST, &ids).await, 1);
    assert_eq!(
        moved(&app, SRC, DST, &ids).await,
        0,
        "a repeat moves nothing"
    );
    assert!(state_of(&app, "usr_third").await.contains_key(&foreign));
    assert!(state_of(&app, DST).await.contains_key(&mine));

    // An unknown source moves nothing and creates nothing.
    assert_eq!(moved(&app, "usr_nobody", DST, &ids).await, 0);
    app.drain_jobs().await;
    app.assert_consistent().await;
}

#[tokio::test]
async fn move_copies_mutes_and_skips_pending_deliveries() {
    let app = support::spawn().await;
    let res = app
        .client
        .put(format!("{}/v1/subscribers/{SRC}/preferences", app.base))
        .bearer_auth(&app.env.api_key)
        .json(&json!({ "preferences": [{ "category": "muted", "channel": "in_app", "enabled": false }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let visible = notif_id(&app.create_notification(SRC, "muted").await);
    let deliver_at = chrono::Utc::now() + chrono::Duration::hours(1);
    let res = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_id": SRC, "category": "c", "deliver_at": deliver_at }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let pending = notif_id(&res.json().await.unwrap());

    assert_eq!(
        moved(&app, SRC, DST, &[visible.clone(), pending.clone()]).await,
        1
    );

    let prefs: Value = app
        .client
        .get(format!("{}/v1/subscribers/{DST}/preferences", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let muted = prefs["preferences"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["category"] == "muted" && p["enabled"] == false);
    assert!(muted, "the mute must follow the move: {prefs}");
    assert_eq!(app.counts(DST).await.0, 0, "a muted notice is not counted");

    let src_rows: Value = list_page(&app, SRC, "").await.json().await.unwrap();
    let still_there = src_rows["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["id"] == pending.as_str());
    assert!(still_there, "a pending delivery stays with the source");
    app.drain_jobs().await;
    app.assert_consistent().await;
}

#[tokio::test]
async fn list_pages_with_a_cursor() {
    let app = support::spawn().await;
    for _ in 0..150 {
        app.create_notification(SRC, "c").await;
    }
    let first: Value = list_page(&app, SRC, "").await.json().await.unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 100);
    let item = &first["items"][0];
    assert!(item["id"].as_str().unwrap().starts_with("notif_"));
    assert_eq!(item["category"], "c");
    assert!(item["payload"].is_object());
    assert!(item["visible_at"].is_string());
    let cursor = first["next_cursor"].as_str().expect("a next cursor");

    let second: Value = list_page(&app, SRC, &format!("?cursor={cursor}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 50);
    assert!(second["next_cursor"].is_null());

    let mut seen: Vec<String> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["items"].as_array().unwrap())
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 150);

    let empty: Value = list_page(&app, "usr_nobody", "")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(empty, json!({ "items": [], "next_cursor": null }));
    assert_eq!(list_page(&app, SRC, "?limit=0").await.status(), 400);
    assert_eq!(list_page(&app, SRC, "?limit=101").await.status(), 400);
    assert_eq!(list_page(&app, SRC, "?cursor=bad!").await.status(), 400);
}

#[tokio::test]
async fn move_validates_its_input() {
    let app = support::spawn().await;
    let id = notif_id(&app.create_notification(SRC, "c").await);
    assert_eq!(
        move_ids(&app, SRC, SRC, std::slice::from_ref(&id))
            .await
            .status(),
        400,
        "to == from"
    );
    assert_eq!(
        move_ids(&app, SRC, "", std::slice::from_ref(&id))
            .await
            .status(),
        400,
        "empty target"
    );
    assert_eq!(
        move_ids(&app, SRC, "a\nb", std::slice::from_ref(&id))
            .await
            .status(),
        400,
        "newline"
    );
    let long = "x".repeat(256);
    assert_eq!(
        move_ids(&app, SRC, &long, std::slice::from_ref(&id))
            .await
            .status(),
        400,
        "too long"
    );
    let many: Vec<String> = (0..101).map(|_| id.clone()).collect();
    assert_eq!(
        move_ids(&app, SRC, DST, &many).await.status(),
        400,
        "over 100 ids"
    );
    assert_eq!(
        move_ids(&app, SRC, DST, &["bcast_x".to_owned()])
            .await
            .status(),
        400,
        "not a notification id"
    );
}

#[tokio::test]
async fn management_endpoints_need_an_api_key() {
    let app = support::spawn().await;
    let res = app
        .client
        .get(format!("{}/v1/subscribers/{SRC}/notifications", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let res = app
        .client
        .post(format!(
            "{}/v1/subscribers/{SRC}/notifications/move",
            app.base
        ))
        .json(&json!({ "to_subscriber_id": DST, "notification_ids": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

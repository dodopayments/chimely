//! Regression guard for the now()-pinned-at-BEGIN watermark clobber on the
//! DELIVER trigger. A scheduled notification coming due during a mark-all-read
//! lost its deliver `+1` permanently, with no second client racing the write.
//!
//! Invariant: the list, the unread count, and read state must agree across
//! both sources at all times.
//!
//! mark_all_read reads `clock_timestamp()` under the counters lock. By the time
//! it holds the lock the deliver bump has committed, so the watermark is newer
//! than the delivered row's visible_at and covers it as read. The exactly-once
//! deliver bump and the watermark move compose into a consistent state.
//!
//! This test produces the `+1` with the unmodified worker and transcribes
//! mark_all_read verbatim below, because a real handler cannot be paused
//! mid-transaction. The real-handler guard is `redteam_markall_watermark_lock`.
//! Keep this transcription in sync with inbox.rs::mark_all_read.

mod support;

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

const SUB: &str = "usr_deliver_markall_race";

/// The internal subscribers.id for the customer-facing id.
async fn internal_id(app: &support::TestApp, external: &str) -> Uuid {
    sqlx::query_scalar(
        "SELECT id FROM subscribers WHERE environment_id = $1 AND subscriber_id = $2",
    )
    .bind(app.env.id)
    .bind(external)
    .fetch_one(&app.pool)
    .await
    .expect("subscriber internal id")
}

#[tokio::test]
async fn mark_all_read_covers_a_scheduled_notification_delivered_in_its_begin_gap() {
    let app = support::spawn().await;

    // Baseline: subscriber and counters row exist, unread = 0, watermark = epoch.
    let res = app
        .client
        .put(format!("{}/v1/subscribers/{SUB}", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .expect("upsert subscriber");
    assert_eq!(res.status(), 200);
    let sub_id = internal_id(&app, SUB).await;

    // A scheduled notification through the real handler: durable immediately,
    // visible_at = deliver_at in the near future, uncounted until the deliver
    // job bumps it. The 800ms/1100ms shape mirrors the worker suite's
    // create_due_scheduled.
    let deliver_at = Utc::now() + chrono::Duration::milliseconds(800);
    let created = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_id": SUB, "category": "reminder",
                    "deliver_at": deliver_at.to_rfc3339() }),
        )
        .send()
        .await
        .expect("create scheduled notification");
    assert_eq!(created.status(), 201);
    let body: serde_json::Value = created.json().await.expect("create body");
    let scheduled_id = body["notifications"][0]["id"].as_str().unwrap().to_owned();

    // mark-all-read step 1: BEGIN pins now() = t_mar before deliver_at, with the
    // counters row not yet locked, mirroring the gap before the handler's FOR
    // UPDATE.
    let mut mark_tx = app.pool.begin().await.expect("begin mark-all txn");
    let t_mar: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *mark_tx)
        .await
        .expect("pin t_mar");
    assert!(
        t_mar < deliver_at,
        "precondition: mark-all's now() is pinned before deliver_at \
         (t_mar={t_mar}, deliver_at={deliver_at})"
    );

    // The real worker delivers inside the gap. mark_tx holds no counters lock
    // yet, so the deliver claims the counters row, bumps unread_direct_count += 1
    // against the still-epoch watermark, and deletes the job, all committed.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let processed = app.sweep().await;
    assert!(
        processed >= 1,
        "the unmodified worker must have delivered the scheduled notification"
    );
    let after_deliver: i32 = sqlx::query_scalar(
        "SELECT unread_direct_count FROM subscriber_counters WHERE subscriber_id = $1",
    )
    .bind(sub_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        after_deliver, 1,
        "the unmodified deliver worker bumped unread_direct_count to 1"
    );

    // mark-all-read step 2: lock and watermark move, transcribed verbatim from
    // inbox.rs::mark_all_read. The FOR UPDATE takes the counters lock after the
    // deliver committed and released it, and read_watermark = clock_timestamp()
    // is evaluated under the lock, so it is newer than the delivered row's
    // visible_at and covers it.
    let _old_watermark: DateTime<Utc> = sqlx::query_scalar(
        "SELECT read_watermark FROM subscriber_counters
          WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .fetch_one(&mut *mark_tx)
    .await
    .expect("lock counters");
    let new_watermark: DateTime<Utc> = sqlx::query_scalar(
        "UPDATE subscriber_counters SET
             read_watermark = clock_timestamp(), unread_direct_count = 0, updated_at = clock_timestamp()
          WHERE environment_id = $1 AND subscriber_id = $2
          RETURNING read_watermark",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .fetch_one(&mut *mark_tx)
    .await
    .expect("move watermark");
    mark_tx.commit().await.expect("commit mark-all");

    // The watermark, captured under the lock after the deliver committed, covers
    // the delivered row, so the merge rule reads it (visible_at <= read_watermark)
    // rather than stranding it above the mark.
    let row_read: bool = sqlx::query_scalar(
        "SELECT n.visible_at <= c.read_watermark
           FROM notifications n
           JOIN subscriber_counters c
             ON c.environment_id = n.environment_id AND c.subscriber_id = n.subscriber_id
          WHERE n.environment_id = $1 AND n.subscriber_id = $2",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(
        row_read,
        "fixed: the delivered row must sit at/below the moved watermark (read) \
         (t_mar={t_mar}, new_watermark={new_watermark}, deliver_at={deliver_at})"
    );

    // The two-source invariant holds.
    let items = app.list_all_items(SUB, 10).await;
    assert!(
        items.iter().any(|i| i["id"] == scheduled_id.as_str()),
        "the delivered reminder is in the list"
    );
    let visible_unread = items
        .iter()
        .filter(|i| !i["read"].as_bool().unwrap())
        .count() as i64;
    assert_eq!(
        visible_unread, 0,
        "the delivered reminder is read after mark-all-read covered it"
    );

    let (unread, _) = app.counts(SUB).await;
    assert_eq!(
        unread, visible_unread,
        "two-source invariant holds: /v1/inbox/counts reports unread={unread}, list shows \
         {visible_unread} unread"
    );
    assert_eq!(
        unread, 0,
        "no permanent drift: the exactly-once deliver bump and the watermark move composed \
         into a consistent state instead of clobbering the counter"
    );
}

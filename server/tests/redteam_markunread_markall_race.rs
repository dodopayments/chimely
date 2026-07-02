//! Guards the interaction between mark-unread and mark-all-read.
//!
//! A mark-unread that commits its `+1` and unread override inside
//! mark-all-read's BEGIN->FOR UPDATE gap must end covered: the watermark is
//! read with clock_timestamp() under the counters lock, so it lands above the
//! override's instant, the counter zeroes, and the override GC clears
//! unread_at. Without the GC the item would sit explicitly unread in the list
//! while the counter reads 0, a permanent two-source drift.
//!
//! The mark-all-read side is transcribed verbatim from inbox.rs::mark_all_read
//! (watermark move + both override GCs). Keep it in sync.

mod support;

use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

const SUB: &str = "usr_unread_race";

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
async fn mark_all_read_covers_a_concurrent_unread_override() {
    let app = support::spawn().await;

    let res = app
        .client
        .put(format!("{}/v1/subscribers/{SUB}", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .expect("upsert subscriber");
    assert_eq!(res.status(), 200);
    let sub_id = internal_id(&app, SUB).await;

    // One notification, then a real mark-all-read: the item is watermark-read
    // and the counter is 0.
    let created = app.create_notification(SUB, "racer").await;
    let notif_id = created["notifications"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let res = app
        .client
        .post(format!("{}/v1/inbox/read-all", app.base))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .expect("first read-all");
    assert_eq!(res.status(), 200);

    // Racing mark-all-read step 1: BEGIN pins now(), counters not locked yet.
    let mut mark_tx = app.pool.begin().await.expect("begin mark-all txn");
    let _t_mar: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *mark_tx)
        .await
        .expect("pin t_mar");
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Concurrent mark-unread through the real handler. The item sits below
    // the current watermark, so the handler writes unread_at and bumps the
    // counter to 1, then commits and releases the counters lock.
    let res = app
        .client
        .post(format!(
            "{}/v1/inbox/notifications/{notif_id}/unread",
            app.base
        ))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .expect("mark unread");
    assert_eq!(res.status(), 204);
    let (unread_mid, _) = app.counts(SUB).await;
    assert_eq!(unread_mid, 1, "the unread override was counted");

    // Racing mark-all-read step 2, transcribed verbatim from
    // inbox.rs::mark_all_read: lock, watermark move under the lock, both
    // override GCs bound to the installed watermark.
    let _old: DateTime<Utc> = sqlx::query_scalar(
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
             read_watermark = clock_timestamp(), unread_direct_count = 0,
             updated_at = clock_timestamp()
          WHERE environment_id = $1 AND subscriber_id = $2
          RETURNING read_watermark",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .fetch_one(&mut *mark_tx)
    .await
    .expect("move watermark");
    sqlx::query(
        "DELETE FROM broadcast_reads
          WHERE environment_id = $1 AND subscriber_id = $2
            AND broadcast_created_at <= $3",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .bind(new_watermark)
    .execute(&mut *mark_tx)
    .await
    .expect("broadcast override GC");
    sqlx::query(
        "UPDATE notifications SET unread_at = NULL
          WHERE environment_id = $1 AND subscriber_id = $2
            AND unread_at IS NOT NULL AND visible_at <= $3",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .bind(new_watermark)
    .execute(&mut *mark_tx)
    .await
    .expect("direct override GC");
    mark_tx.commit().await.expect("commit mark-all");

    // The override died with the watermark move: no unread_at survives, the
    // list reports the item read, and the count agrees.
    let override_left: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM notifications
          WHERE environment_id = $1 AND subscriber_id = $2 AND unread_at IS NOT NULL)",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(!override_left, "the unread override was GC'd");

    let items = app.list_all_items(SUB, 10).await;
    let visible_unread = items
        .iter()
        .filter(|i| !i["read"].as_bool().unwrap())
        .count() as i64;
    assert_eq!(
        visible_unread, 0,
        "the item reads as covered by the watermark"
    );
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(
        unread, visible_unread,
        "two-source invariant holds after the race"
    );
}

//! Guards archive-all's watermark move against writes committing in its
//! BEGIN->FOR UPDATE gap, mirroring the mark-all-read race guards.
//!
//! The watermark is read with clock_timestamp() under the counters lock, so
//! anything that committed before the lock was taken sits at or below it:
//! archived and uncounted, consistent, never stranded. The override GCs bind
//! the installed watermark, so overrides above it are never destroyed.
//!
//! The archive-all side is transcribed verbatim from inbox.rs::archive_all.
//! Keep it in sync.

mod support;

use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

const SUB: &str = "usr_archive_race";

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

/// Runs the transcribed archive-all step 2 (lock, watermark move, both GCs)
/// on an already-begun transaction and returns the installed watermark.
async fn transcribed_archive_all(
    app: &support::TestApp,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sub_id: Uuid,
) -> DateTime<Utc> {
    sqlx::query(
        "SELECT 1 FROM subscriber_counters
          WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .execute(&mut **tx)
    .await
    .expect("lock counters");
    let new_watermark: DateTime<Utc> = sqlx::query_scalar(
        "UPDATE subscriber_counters SET
             archive_watermark = clock_timestamp(), unread_direct_count = 0,
             updated_at = clock_timestamp()
          WHERE environment_id = $1 AND subscriber_id = $2
          RETURNING archive_watermark",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .fetch_one(&mut **tx)
    .await
    .expect("move archive watermark");
    sqlx::query(
        "UPDATE notifications SET archived_at = NULL, unarchived_at = NULL
          WHERE environment_id = $1 AND subscriber_id = $2
            AND (archived_at IS NOT NULL OR unarchived_at IS NOT NULL)
            AND visible_at <= $3",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .bind(new_watermark)
    .execute(&mut **tx)
    .await
    .expect("direct override GC");
    sqlx::query(
        "DELETE FROM broadcast_archives
          WHERE environment_id = $1 AND subscriber_id = $2
            AND broadcast_created_at <= $3",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .bind(new_watermark)
    .execute(&mut **tx)
    .await
    .expect("broadcast override GC");
    new_watermark
}

#[tokio::test]
async fn archive_all_covers_a_concurrently_created_notification() {
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

    // archive-all step 1: BEGIN pins now(), counters not locked yet.
    let mut tx = app.pool.begin().await.expect("begin archive-all txn");
    let _t: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *tx)
        .await
        .expect("pin now");
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Concurrent create through the real handler: +1 committed in the gap.
    app.create_notification(SUB, "racer").await;
    let (unread_mid, _) = app.counts(SUB).await;
    assert_eq!(unread_mid, 1, "the create was counted");

    let _wm = transcribed_archive_all(&app, &mut tx, sub_id).await;
    tx.commit().await.expect("commit archive-all");

    // The racer sits at or below the installed watermark: archived and
    // uncounted, consistent across list and count.
    let items = app.list_all_items(SUB, 10).await;
    assert!(items.is_empty(), "default view is empty after archive-all");
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0, "counter zeroed, racer covered not stranded");
}

#[tokio::test]
async fn archive_all_gc_covers_an_override_committed_in_its_gap() {
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
    let created = app.create_broadcast("announce").await;
    let bcast_id = created["id"].as_str().unwrap().to_owned();

    // archive-all step 1 pins BEGIN; the individual archive commits its
    // override row (above the old epoch watermark) inside the gap.
    let mut tx = app.pool.begin().await.expect("begin archive-all txn");
    let _t: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *tx)
        .await
        .expect("pin now");
    tokio::time::sleep(Duration::from_millis(20)).await;
    let res = app
        .client
        .post(format!(
            "{}/v1/inbox/broadcasts/{bcast_id}/archive",
            app.base
        ))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .expect("archive broadcast");
    assert_eq!(res.status(), 204);

    let wm = transcribed_archive_all(&app, &mut tx, sub_id).await;
    tx.commit().await.expect("commit archive-all");

    // The override row was redundant once the watermark covered it: GC'd,
    // and the item stays archived by the watermark. No resurrection.
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM broadcast_archives WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(rows, 0, "redundant override GC'd (wm={wm})");
    let items = app.list_all_items(SUB, 10).await;
    assert!(items.is_empty(), "the broadcast stays archived");
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0, "two-source invariant holds");
}

//! Guards that mark_all_read installs `read_watermark = clock_timestamp()`
//! under the counters row lock, so a direct notification committing its `+1`
//! inside the BEGIN->FOR UPDATE gap is covered by the watermark, not zeroed
//! while stranded above a stale watermark.
//!
//! Enforces the CLAUDE.md invariant that the list, the unread count, and read
//! state agree across both sources at all times.
//!
//! Reading the watermark with `now()` pinned at BEGIN installs a watermark
//! older than a create that commits in the gap. That create then sits unread
//! in the list while the counter reads 0, a permanent two-source drift that no
//! counter_rebuild reconciles.
//!
//! This forces that interleaving against mark_all_read transcribed verbatim
//! below. The real-handler guard for the same property is
//! `redteam_markall_watermark_lock`. Keep this transcription in sync with
//! inbox.rs::mark_all_read.

mod support;

use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

const SUB: &str = "usr_markall_race";

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
async fn mark_all_read_does_not_clobber_a_concurrently_created_notification() {
    let app = support::spawn().await;

    // Baseline: subscriber + counters row exist, unread = 0, watermark = epoch.
    let res = app
        .client
        .put(format!("{}/v1/subscribers/{SUB}", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .expect("upsert subscriber");
    assert_eq!(res.status(), 200);
    let sub_id = internal_id(&app, SUB).await;

    // mark-all-read step 1: BEGIN pins now() = t_mar. The non-locking read
    // mirrors the gap before the handler's FOR UPDATE, with no counters lock
    // held yet.
    let mut mark_tx = app.pool.begin().await.expect("begin mark-all txn");
    let t_mar: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *mark_tx)
        .await
        .expect("pin t_mar");

    // Widen the BEGIN->FOR UPDATE window so the concurrent create lands inside
    // it deterministically, with t_ins strictly newer than t_mar.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Concurrent create through the real handler. visible_at = t_ins > t_mar.
    // Its conditional increment reads watermark = epoch, so t_ins > epoch
    // increments by 1, then commits.
    let created = app.create_notification(SUB, "racer").await;
    let racer_id = created["notifications"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let after_create: i32 = sqlx::query_scalar(
        "SELECT unread_direct_count FROM subscriber_counters WHERE subscriber_id = $1",
    )
    .bind(sub_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(after_create, 1, "real create handler bumped the counter");

    // mark-all-read step 2: lock and watermark move, transcribed verbatim from
    // inbox.rs::mark_all_read. The FOR UPDATE takes the counters lock now, after
    // the create committed and released it. The UPDATE writes read_watermark =
    // clock_timestamp() evaluated under the lock, so newer than the racer's
    // visible_at, and zeroes the counter. A `now()` here would install a
    // watermark older than the racer and strand it unread-uncounted.
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

    // The watermark captured under the lock covers the racer, so by the merge
    // query's rule (visible_at <= read_watermark) the racer is read, not
    // stranded above it.
    let racer_read: bool = sqlx::query_scalar(
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
        racer_read,
        "fixed: the racer must sit at/below the moved watermark (read) \
         (t_mar={t_mar}, new_watermark={new_watermark})"
    );

    // Two-source invariant: the racer is in the list marked read, and the
    // unread count agrees with the zero visible-unread count.
    let items = app.list_all_items(SUB, 10).await;
    assert!(
        items.iter().any(|i| i["id"] == racer_id.as_str()),
        "the racer is in the list"
    );
    let visible_unread = items
        .iter()
        .filter(|i| !i["read"].as_bool().unwrap())
        .count() as i64;
    assert_eq!(
        visible_unread, 0,
        "the racer is read after mark-all-read covered it"
    );

    let (unread, _) = app.counts(SUB).await;
    assert_eq!(
        unread, visible_unread,
        "two-source invariant holds: /v1/inbox/counts reports unread={unread}, list shows \
         {visible_unread} unread"
    );
    assert_eq!(
        unread, 0,
        "no permanent drift: mark-all-read covered the raced-in create instead of \
         clobbering its counter"
    );
}

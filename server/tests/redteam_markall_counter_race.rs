//! REGRESSION GUARD (was a RED-TEAM finding): the now()-pinned-at-BEGIN
//! watermark clobber is FIXED. `mark_all_read` now installs `read_watermark =
//! clock_timestamp()` read UNDER the counters row lock (inbox.rs), so a direct
//! notification that commits its `+1` inside the handler's BEGIN->FOR UPDATE gap
//! is correctly COVERED by the watermark (read) instead of being zeroed while
//! stranded above a stale watermark.
//!
//! INVARIANT (CLAUDE.md): "The list, the unread count, and read state must agree
//! across both sources at all times."
//!
//! HISTORICAL BUG: mark_all_read did `pool.begin()` — Postgres pins `now()` at
//! BEGIN — and only LATER took the counters lock, then wrote `read_watermark =
//! now()` plus `unread_direct_count = 0`. A create that committed its `+1` in
//! the gap, with `visible_at` newer than the pinned `now()`, landed ABOVE the
//! watermark (unread in the list) while the counter read 0 — permanent
//! two-source drift, never reconciled (no counter_rebuild fires).
//!
//! This test forces that exact interleaving (a real `POST /v1/notifications`
//! committing inside a widened BEGIN->FOR UPDATE gap) against the FIXED
//! mark_all_read transcribed verbatim below, and asserts the invariant holds:
//! the racer ends READ, and list and count agree at 0. The mark-all-read
//! linearizes at lock-acquisition time, after the create committed, so the new
//! notification is correctly part of "everything read". The real-handler guard
//! for the same property (watermark captured after the lock) is
//! `redteam_markall_watermark_lock`; keep this transcription in sync with
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

    // --- mark-all-read transaction, step 1: BEGIN (pins now() = t_mar) -------
    // Real handler: `state.pool.begin()`. We pin t_mar with a NON-locking read
    // and crucially do NOT take the counters lock yet, mirroring the gap before
    // the handler's FOR UPDATE.
    let mut mark_tx = app.pool.begin().await.expect("begin mark-all txn");
    let t_mar: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *mark_tx)
        .await
        .expect("pin t_mar");

    // Widen the BEGIN->FOR UPDATE window so the concurrent create lands inside
    // it deterministically (and so t_ins is strictly newer than t_mar).
    tokio::time::sleep(Duration::from_millis(20)).await;

    // --- concurrent create through the REAL handler --------------------------
    // visible_at = t_ins = the create txn's now() > t_mar. Its conditional
    // increment reads watermark = epoch, so t_ins > epoch => +1. Commits.
    let created = app.create_notification(SUB, "racer").await;
    let racer_id = created["notifications"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // The real create handler counted the new row: unread_direct_count == 1.
    let after_create: i32 = sqlx::query_scalar(
        "SELECT unread_direct_count FROM subscriber_counters WHERE subscriber_id = $1",
    )
    .bind(sub_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(after_create, 1, "real create handler bumped the counter");

    // --- mark-all-read transaction, step 2: lock + watermark move ------------
    // Transcribed verbatim from the FIXED inbox.rs::mark_all_read. The FOR
    // UPDATE takes the counters lock NOW (the create already committed and
    // released it), and the UPDATE writes read_watermark = clock_timestamp()
    // (evaluated UNDER the lock, AFTER the create committed, so NEWER than the
    // racer's visible_at) and zeroes the counter. The pre-fix `now()` here would
    // install a watermark OLDER than the racer and strand it unread-uncounted.
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

    // The fix: the watermark (captured under the lock, after the create
    // committed) covers the racer, so by the merge query's own rule
    // (visible_at <= read_watermark) the racer is READ — not stranded above it.
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

    // --- the two-source invariant holds --------------------------------------
    // The racer is in the list, marked read; the unread count agrees with the
    // (zero) visible-unread count.
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

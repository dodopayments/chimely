//! REGRESSION GUARD (was a RED-TEAM finding; extends redteam_markall_counter_race):
//! the now()-pinned-at-BEGIN watermark clobber is FIXED for the DELIVER trigger
//! too. The hole was never specific to a concurrently-created direct
//! notification — it also swallowed the worker's exactly-once DELIVER bump: a
//! scheduled notification coming due *during* a mark-all-read lost its `+1`
//! permanently, with NO second client racing the write.
//!
//! INVARIANT (CLAUDE.md): "The list, the unread count, and read state must agree
//! across both sources at all times."
//!
//! HISTORICAL BUG: `mark_all_read` captured `now()` at BEGIN (before its FOR
//! UPDATE) and wrote `read_watermark = now()` + `unread_direct_count = 0`. The
//! deliver job's `+1` (committed by the unmodified worker inside the
//! BEGIN->FOR UPDATE gap) was for a row whose `visible_at` (= its `deliver_at`)
//! was newer than the pinned `now()`, so the row sat ABOVE the stale watermark
//! (unread in the list) while the counter was zeroed — permanent drift, never
//! reconciled.
//!
//! THE FIX: mark_all_read reads `clock_timestamp()` UNDER the counters lock. By
//! the time it holds the lock the deliver bump has committed, so the watermark
//! is NEWER than the delivered row's visible_at and correctly covers it (read).
//! The exactly-once deliver bump and the watermark move now compose into a
//! consistent state instead of permanent drift.
//!
//! This test produces the `+1` with the UNMODIFIED worker (`worker::sweep_once`)
//! and transcribes the FIXED mark_all_read verbatim below (a real handler cannot
//! be paused mid-transaction). The real-handler guard for the watermark-after-
//! lock property is `redteam_markall_watermark_lock`; keep this transcription in
//! sync with inbox.rs::mark_all_read.

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

    // A SCHEDULED notification through the REAL handler: durable immediately,
    // visible_at = deliver_at in the near future, UNCOUNTED until the deliver
    // job bumps it. The 800ms/1100ms shape mirrors the worker suite's
    // `create_due_scheduled`.
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

    // --- mark-all-read transaction, step 1: BEGIN (pins now() = t_mar) -------
    // t_mar is pinned BEFORE deliver_at and the counters row is NOT yet locked,
    // mirroring the gap before the handler's FOR UPDATE.
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

    // --- the REAL worker delivers, INSIDE the gap ----------------------------
    // Wait out deliver_at so the deliver job is due, then run one real sweep.
    // mark_tx holds no counters lock yet, so the deliver claims the counters
    // row, bumps unread_direct_count += 1 against the still-epoch watermark, and
    // DELETEs the job — all committed.
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

    // --- mark-all-read transaction, step 2: lock + watermark move ------------
    // Transcribed verbatim from the FIXED inbox.rs::mark_all_read. The FOR
    // UPDATE takes the counters lock NOW (the deliver already committed and
    // released it), and read_watermark = clock_timestamp() is evaluated UNDER
    // the lock — AFTER the deliver committed, so NEWER than the delivered row's
    // visible_at. The pre-fix `now()` here would land BELOW that row and strand
    // it unread-uncounted.
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

    // The fix: the watermark (captured under the lock, after the deliver
    // committed) covers the delivered row, so it is READ by the merge rule
    // (visible_at <= read_watermark) — not stranded above the mark.
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

    // --- the two-source invariant holds --------------------------------------
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

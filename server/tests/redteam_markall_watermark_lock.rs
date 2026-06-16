//! `mark_all_read` must capture its `read_watermark` from the wall clock after
//! it acquires the counters row lock, never from the transaction timestamp
//! pinned at BEGIN. Guards the invariant that the list, the unread count, and
//! read state agree across both sources at all times.
//!
//! Gap-clobber failure mode: the handler `pool.begin()`s, which pins `now()` at
//! BEGIN, and only later takes the counters lock with `SELECT read_watermark
//! ... FOR UPDATE` before writing `read_watermark = now()`. A `+1` that commits
//! in the BEGIN-to-FOR-UPDATE gap, for a row whose ordering timestamp is newer
//! than the pinned `now()`, is clobbered to `unread_direct_count = 0` while the
//! row stays above the watermark and unread in the list. The drift is permanent.
//!
//! This test runs against the unmodified handler. It holds the counters lock so
//! the real `POST /v1/inbox/read-all` blocks at its FOR UPDATE after it has
//! begun, lets the pinned `now()` age, then releases the lock and asserts the
//! installed watermark is the lock-time wall clock, not the stale BEGIN value.
//! Fails on a `now()` handler, passes on the `clock_timestamp()` fix.

mod support;

use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

const SUB: &str = "usr_markall_lock";

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

/// True once some other backend is blocked waiting on a row lock while running
/// the mark-all-read `FOR UPDATE` on subscriber_counters.
async fn markall_blocked_on_lock(app: &support::TestApp) -> bool {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity
          WHERE pid <> pg_backend_pid()
            AND wait_event_type = 'Lock'
            AND query ILIKE '%subscriber_counters%for update%'",
    )
    .fetch_one(&app.pool)
    .await
    .expect("pg_stat_activity probe");
    n > 0
}

#[tokio::test]
async fn mark_all_read_captures_its_watermark_after_taking_the_counters_lock() {
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

    // Hold the counters row lock so mark_all_read blocks at its FOR UPDATE
    // after it has begun and pinned its transaction now().
    let mut hold = app.pool.begin().await.expect("begin lock-holder txn");
    let _locked: Uuid = sqlx::query_scalar(
        "SELECT subscriber_id FROM subscriber_counters
          WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .fetch_one(&mut *hold)
    .await
    .expect("hold counters lock");

    // mark_all_read BEGINs, pinning now(), then blocks on the lock above.
    let base = app.base.clone();
    let headers = app.subscriber_headers(SUB);
    let client = app.client.clone();
    let handle = tokio::spawn(async move {
        client
            .post(format!("{base}/v1/inbox/read-all"))
            .headers(headers)
            .send()
            .await
    });

    // Wait until the handler blocks on the counters lock, so its BEGIN-pinned
    // now() is firmly in the past.
    let mut blocked = false;
    for _ in 0..120 {
        if markall_blocked_on_lock(&app).await {
            blocked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(blocked, "mark_all_read never blocked on the counters lock");
    // Margin so a BEGIN-pinned watermark is unambiguously older than the
    // release instant.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // The correct watermark must be >= this instant. DB clock, captured after
    // the gap, just before the lock is released.
    let t_release: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&app.pool)
        .await
        .expect("capture release instant");
    hold.commit().await.expect("release counters lock");

    let res = handle
        .await
        .expect("join mark-all task")
        .expect("mark-all response");
    assert_eq!(res.status(), 200, "mark_all_read completed");

    let watermark: DateTime<Utc> = sqlx::query_scalar(
        "SELECT read_watermark FROM subscriber_counters
          WHERE environment_id = $1 AND subscriber_id = $2",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .fetch_one(&app.pool)
    .await
    .expect("read watermark");

    assert!(
        watermark >= t_release,
        "mark_all_read installed a watermark captured BEFORE it held the counters lock \
         (got {watermark} < {t_release}): now() is pinned at BEGIN, so an item that commits \
         its +1 in the BEGIN->FOR UPDATE gap is zeroed while staying above the watermark — \
         permanent two-source drift. The watermark must be clock_timestamp() read under the lock."
    );
}

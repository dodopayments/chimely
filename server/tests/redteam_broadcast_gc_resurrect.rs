//! RED-TEAM regression guard (REAL handler): mark_all_read's broadcast_reads GC
//! must bind the watermark it just installed, never a fresh clock read. The fix
//! changed the GC bound from a re-evaluated clock to the returned `new_watermark`
//! so it "can never delete an exception row above the watermark and resurrect an
//! individually-read broadcast as unread" (commit 18ea409).
//!
//! The bug needs an exact interleaving: mark_all_read installs the watermark W
//! (clock under the counters lock), and an above-W exception must be visible to
//! the GC's DELETE. Within one transaction the GC runs microseconds after the
//! UPDATE, and the counters lock serializes every production exception insert,
//! so this state never arises by itself. We construct it deterministically:
//!
//!   1. Hold a table lock on `jobs` so the handler blocks at enqueue_timeline,
//!      which runs AFTER the watermark UPDATE (W installed) but BEFORE the GC
//!      DELETE.
//!   2. While it is blocked, commit a broadcast read with broadcast_created_at
//!      strictly after W. It commits before the (later) DELETE statement starts,
//!      so READ COMMITTED puts it in the DELETE's snapshot.
//!   3. Release the lock. The handler's DELETE now faces an above-watermark
//!      exception.
//!
//! With the fix (bound = W) that exception is kept (created_at > W). With a fresh
//! clock read (bound = clock at DELETE time, which is later than W) it is deleted
//! and the broadcast resurrects as unread. The test asserts it survives.

mod support;

use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

const SUB: &str = "usr_gc_resurrect";

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

/// True once some other backend is blocked waiting for a lock on the `jobs`
/// relation. Probing pg_locks by relation (not pg_stat_activity by query text)
/// keeps this correct if the enqueue SQL is ever rephrased.
///
/// REQUIRED INVARIANT this test depends on: the FIRST `jobs` write in
/// mark_all_read (enqueue_timeline) runs AFTER the watermark UPDATE and BEFORE
/// the broadcast_reads GC DELETE. The SHARE lock parks the handler at that
/// write, which is the window we inject the above-watermark exception into. If
/// the GC DELETE is ever moved ahead of enqueue_timeline, the handler would
/// park at a `jobs` write that runs AFTER the DELETE: the injected exception
/// would land too late for the DELETE's READ COMMITTED snapshot and survive
/// regardless of the bound, turning this guard into a vacuous pass. Keep the GC
/// DELETE after enqueue_timeline, or rewrite the pause point here.
async fn markall_blocked_on_jobs_lock(app: &support::TestApp) -> bool {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks
          WHERE relation = 'jobs'::regclass
            AND NOT granted
            AND pid <> pg_backend_pid()",
    )
    .fetch_one(&app.pool)
    .await
    .expect("pg_locks probe");
    n > 0
}

async fn broadcast_read_exists(app: &support::TestApp, sub: Uuid, broadcast: Uuid) -> bool {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM broadcast_reads
          WHERE environment_id = $1 AND subscriber_id = $2 AND broadcast_id = $3",
    )
    .bind(app.env.id)
    .bind(sub)
    .bind(broadcast)
    .fetch_one(&app.pool)
    .await
    .expect("broadcast_reads probe");
    n > 0
}

#[tokio::test]
async fn mark_all_read_gc_keeps_an_above_watermark_exception() {
    let app = support::spawn().await;

    // Create the subscriber (and its counters row, watermark = epoch).
    let res = app
        .client
        .put(format!("{}/v1/subscribers/{SUB}", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .expect("upsert subscriber");
    assert_eq!(res.status(), 200);
    let sub_id = internal_id(&app, SUB).await;

    // A broadcast read individually BEFORE the move: this exception is below
    // the watermark mark_all_read will install, so the GC must remove it. Its
    // removal proves the DELETE actually ran (the test is not vacuous).
    app.create_broadcast("below").await;
    let below_id = app
        .list_all_items(SUB, 10)
        .await
        .into_iter()
        .find(|i| i["source"] == "broadcast")
        .expect("broadcast in inbox")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/broadcasts/{below_id}/read"))
        .await;
    assert_eq!(res.status(), 204);
    let below_uuid = dronte::ids::parse_typeid(dronte::ids::BROADCAST, &below_id).unwrap();
    assert!(broadcast_read_exists(&app, sub_id, below_uuid).await);

    // Hold a SHARE lock on `jobs` so the handler's enqueue_timeline INSERT
    // blocks AFTER it has installed the watermark but BEFORE the GC DELETE.
    let mut hold = app.pool.begin().await.expect("begin lock-holder txn");
    sqlx::query("LOCK TABLE jobs IN SHARE MODE")
        .execute(&mut *hold)
        .await
        .expect("share-lock jobs");

    // Fire the REAL mark_all_read. It installs the watermark, then blocks at
    // the enqueue_timeline write (see the invariant on the probe below).
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

    let mut blocked = false;
    for _ in 0..120 {
        if markall_blocked_on_jobs_lock(&app).await {
            blocked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        blocked,
        "mark_all_read never blocked at enqueue_timeline (watermark already installed)"
    );
    // Margin so the new exception's broadcast_created_at is unambiguously after
    // the watermark instant the handler captured.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // An ABOVE-watermark broadcast read, committed while the handler is parked
    // between its watermark UPDATE and its GC DELETE. broadcast_created_at =
    // clock_timestamp() now, which is strictly after the watermark W. It commits
    // before the DELETE statement starts, so it lands in the DELETE's snapshot.
    let above_uuid = dronte::ids::new_uuid();
    let above_created_at: DateTime<Utc> = sqlx::query_scalar(
        "INSERT INTO broadcasts (environment_id, id, category, created_at)
         VALUES ($1, $2, 'above', clock_timestamp())
         RETURNING created_at",
    )
    .bind(app.env.id)
    .bind(above_uuid)
    .fetch_one(&app.pool)
    .await
    .expect("insert above-watermark broadcast");
    sqlx::query(
        "INSERT INTO broadcast_reads
             (environment_id, subscriber_id, broadcast_id, broadcast_created_at, read_at)
         VALUES ($1, $2, $3, $4, now())",
    )
    .bind(app.env.id)
    .bind(sub_id)
    .bind(above_uuid)
    .bind(above_created_at)
    .execute(&app.pool)
    .await
    .expect("insert above-watermark exception");

    // Release the lock. The handler runs its GC DELETE and commits.
    hold.commit().await.expect("release jobs lock");
    let res = handle
        .await
        .expect("join mark-all")
        .expect("mark-all response");
    assert_eq!(res.status(), 200, "mark_all_read completed");

    // The watermark must sit strictly below the above-watermark exception.
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
        watermark < above_created_at,
        "test precondition: watermark ({watermark}) must precede the above-watermark \
         exception ({above_created_at})"
    );

    // The GC ran: the below-watermark exception is gone (redundant).
    assert!(
        !broadcast_read_exists(&app, sub_id, below_uuid).await,
        "below-watermark exception should be GC'd"
    );
    // The fix: the ABOVE-watermark exception survives. A fresh-clock GC bound
    // would delete it and resurrect the broadcast as unread.
    assert!(
        broadcast_read_exists(&app, sub_id, above_uuid).await,
        "mark_all_read GC deleted an exception ABOVE the watermark, resurrecting an \
         individually-read broadcast as unread: the GC must bind the installed watermark, \
         not a fresh clock read"
    );

    // User-facing symptom: the above-watermark broadcast stays read in the list.
    let above_typeid = dronte::ids::typeid(dronte::ids::BROADCAST, above_uuid);
    let item_read = app
        .list_all_items(SUB, 50)
        .await
        .into_iter()
        .find(|i| i["id"] == above_typeid.as_str())
        .map(|i| i["read"] == true);
    assert_eq!(
        item_read,
        Some(true),
        "the above-watermark broadcast must still be read in the inbox list"
    );
}

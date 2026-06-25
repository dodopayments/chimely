//! mark_all_read's broadcast_reads GC must bind the watermark it just installed,
//! not a fresh clock read. A fresh clock read at DELETE time is later than the
//! installed watermark W and deletes an above-W exception, resurrecting an
//! individually-read broadcast as unread.
//!
//! The bug needs an exact interleaving: mark_all_read installs the watermark W
//! (clock under the counters lock), and an above-W exception must be visible to
//! the GC's DELETE. Within one transaction the GC runs microseconds after the
//! UPDATE, and the counters lock serializes every production exception insert, so
//! this state never arises by itself. The test constructs it deterministically.
//! A SHARE lock on `jobs` parks the handler at enqueue_timeline, which runs after
//! the watermark UPDATE but before the GC DELETE. While parked, an above-W
//! broadcast read commits before the DELETE statement starts, so READ COMMITTED
//! puts it in the DELETE's snapshot. The lock releases and the DELETE faces an
//! above-watermark exception that the correct bound (W) keeps.

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
/// relation. Probing pg_locks by relation, not pg_stat_activity by query text,
/// keeps this correct if the enqueue SQL is ever rephrased.
///
/// This test depends on the first `jobs` write in mark_all_read
/// (enqueue_timeline) running after the watermark UPDATE and before the
/// broadcast_reads GC DELETE. The SHARE lock parks the handler at that write,
/// the window the above-watermark exception is injected into. If the GC DELETE
/// ever moves ahead of enqueue_timeline, the handler parks at a `jobs` write
/// after the DELETE, the injected exception lands too late for the DELETE's READ
/// COMMITTED snapshot and survives regardless of the bound, and this guard
/// passes vacuously. Keep the GC DELETE after enqueue_timeline or rewrite the
/// pause point here.
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

    // Creates the subscriber and its counters row at watermark = epoch.
    let res = app
        .client
        .put(format!("{}/v1/subscribers/{SUB}", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .expect("upsert subscriber");
    assert_eq!(res.status(), 200);
    let sub_id = internal_id(&app, SUB).await;

    // A broadcast read below the watermark mark_all_read will install. The GC
    // must remove it, which proves the DELETE actually ran.
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

    // SHARE lock on `jobs` so the handler's enqueue_timeline INSERT blocks after
    // it installs the watermark but before the GC DELETE.
    let mut hold = app.pool.begin().await.expect("begin lock-holder txn");
    sqlx::query("LOCK TABLE jobs IN SHARE MODE")
        .execute(&mut *hold)
        .await
        .expect("share-lock jobs");

    // mark_all_read installs the watermark, then blocks at the enqueue_timeline
    // write. See the invariant on markall_blocked_on_jobs_lock.
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

    // An above-watermark broadcast read, committed while the handler is parked
    // between its watermark UPDATE and its GC DELETE. broadcast_created_at is
    // clock_timestamp() now, strictly after the watermark W. It commits before
    // the DELETE statement starts, so it lands in the DELETE's snapshot.
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

    // Releasing the lock lets the handler run its GC DELETE and commit.
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

    // The GC ran. The below-watermark exception is gone.
    assert!(
        !broadcast_read_exists(&app, sub_id, below_uuid).await,
        "below-watermark exception should be GC'd"
    );
    // The above-watermark exception survives. A fresh-clock GC bound would
    // delete it and resurrect the broadcast as unread.
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

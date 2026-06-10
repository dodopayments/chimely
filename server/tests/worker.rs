//! Task 3: the worker loop — fair SKIP LOCKED claims, deliver flow with
//! exactly-once counter bumps keyed on job deletion, progress_cursor
//! resumability (kill mid-deliver), counter_rebuild, delete-on-complete,
//! failure parking.

mod support;

use chrono::Utc;
use dronte::{jobs, worker};
use serde_json::json;
use uuid::Uuid;

async fn job_types(app: &support::TestApp, env: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT job_type FROM jobs WHERE environment_id = $1 ORDER BY run_at")
        .bind(env)
        .fetch_all(&app.pool)
        .await
        .unwrap()
}

/// Schedule a notification batch due (just) in the future and wait it out.
async fn create_due_scheduled(app: &support::TestApp, subscriber: &str, n: usize) {
    let deliver_at = Utc::now() + chrono::Duration::milliseconds(800);
    let recipients: Vec<String> = (0..n).map(|i| format!("{subscriber}_{i}")).collect();
    let res = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_ids": recipients, "category": "scheduled",
                    "deliver_at": deliver_at.to_rfc3339() }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
}

#[tokio::test]
async fn deliver_bumps_counters_in_the_job_deletion_txn_and_completes_by_delete() {
    let app = support::spawn().await;
    create_due_scheduled(&app, "usr_d", 3).await;

    let (unread, _, _) = app.counter_row("usr_d_0").await;
    assert_eq!(
        unread, 0,
        "scheduled rows are uncounted before the deliver job"
    );

    app.drain_jobs().await;

    for i in 0..3 {
        let (unread, unseen, _) = app.counter_row(&format!("usr_d_{i}")).await;
        assert_eq!((unread, unseen), (1, 1), "deliver bump for recipient {i}");
        let items = app.list_all_items(&format!("usr_d_{i}"), 10).await;
        assert_eq!(items.len(), 1, "item visible after deliver_at");
        assert_eq!(items[0]["read"], false);
    }
    // Jobs are deleted on completion — never status-flagged.
    assert_eq!(app.job_count(app.env.id).await, 0);
}

#[tokio::test]
async fn killing_the_worker_mid_deliver_is_replay_safe() {
    let app = support::spawn().await;
    create_due_scheduled(&app, "usr_k", 2).await;

    // Crash: one deliver chunk applied, then the txn aborts.
    assert!(
        worker::crash_mid_deliver(&app.pool, app.env.id)
            .await
            .unwrap()
    );
    let (unread, _, _) = app.counter_row("usr_k_0").await;
    assert_eq!(unread, 0, "aborted chunk must leave no effects");
    assert_eq!(
        job_types(&app, app.env.id).await,
        ["deliver"],
        "job survives the crash"
    );

    // The successor re-claims and applies exactly once.
    app.drain_jobs().await;
    for i in 0..2 {
        let (unread, _, _) = app.counter_row(&format!("usr_k_{i}")).await;
        assert_eq!(unread, 1, "exactly-once bump for recipient {i}");
    }
    assert_eq!(app.job_count(app.env.id).await, 0);
}

#[tokio::test]
async fn large_deliver_jobs_advance_progress_cursor_per_chunk() {
    let app = support::spawn().await;
    // Build a 600-row scheduled fan-out (> DELIVER_CHUNK) as 6 API batches,
    // then splice them into ONE deliver job to exercise chunking. The
    // deliver_at sits far in the future: the synthetic job below is due
    // immediately regardless, the bump does not gate on visibility, and a
    // near deadline 400s the later batches on slow CI runners.
    let deliver_at = Utc::now() + chrono::Duration::seconds(60);
    for batch in 0..6 {
        let recipients: Vec<String> = (0..100).map(|i| format!("usr_big_{batch}_{i}")).collect();
        let res = app
            .mgmt_post(
                "/v1/notifications",
                json!({ "subscriber_ids": recipients, "category": "big",
                        "deliver_at": deliver_at.to_rfc3339() }),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
    }
    let all_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM notifications WHERE environment_id = $1 ORDER BY id")
            .bind(app.env.id)
            .fetch_all(&app.pool)
            .await
            .unwrap();
    assert_eq!(all_ids.len(), 600);
    sqlx::query("DELETE FROM jobs WHERE environment_id = $1")
        .bind(app.env.id)
        .execute(&app.pool)
        .await
        .unwrap();
    let mut conn = app.pool.acquire().await.unwrap();
    jobs::enqueue(
        &mut conn,
        app.env.id,
        jobs::TYPE_DELIVER,
        json!({ "notification_ids": all_ids }),
        None,
    )
    .await
    .unwrap();
    drop(conn);

    // First claim: one chunk (500), cursor advanced, job still present.
    assert!(
        worker::process_one(&app.pool, app.pubsub.as_ref(), &app.cfg, app.env.id)
            .await
            .unwrap()
    );
    let cursor: serde_json::Value = sqlx::query_scalar(
        "SELECT progress_cursor FROM jobs WHERE environment_id = $1 AND job_type = 'deliver'",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(cursor["offset"], 500);

    // Second claim finishes the tail and deletes the job.
    assert!(
        worker::process_one(&app.pool, app.pubsub.as_ref(), &app.cfg, app.env.id)
            .await
            .unwrap()
    );
    let remaining: Vec<String> = job_types(&app, app.env.id).await;
    assert_eq!(remaining, ["hint"], "deliver gone; trailing hint enqueued");

    let total_unread: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(unread_direct_count), 0) FROM subscriber_counters WHERE environment_id = $1",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        total_unread, 600,
        "each row bumped exactly once across chunks"
    );
}

#[tokio::test]
async fn claims_round_robin_so_one_environments_flood_cannot_starve_another() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;

    // Flood env A with 50 due jobs; env B has 1 real-time job.
    let mut conn = app.pool.acquire().await.unwrap();
    for _ in 0..50 {
        jobs::enqueue(
            &mut conn,
            app.env.id,
            jobs::TYPE_COUNTER_REBUILD,
            json!({ "subscriber_id": Uuid::nil() }),
            None,
        )
        .await
        .unwrap();
    }
    jobs::enqueue(
        &mut conn,
        env_b.id,
        jobs::TYPE_COUNTER_REBUILD,
        json!({ "subscriber_id": Uuid::nil() }),
        None,
    )
    .await
    .unwrap();
    drop(conn);

    // ONE fair sweep: each environment with pending work gets exactly one
    // claim — env B's job is done while env A still has 49 queued.
    let processed = worker::sweep_once(&app.pool, app.pubsub.as_ref(), &app.cfg)
        .await
        .unwrap();
    assert_eq!(processed, 2);
    async fn rebuilds(pool: &sqlx::PgPool, env: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE environment_id = $1 AND job_type = 'counter_rebuild'",
        )
        .bind(env)
        .fetch_one(pool)
        .await
        .unwrap()
    }
    assert_eq!(
        rebuilds(&app.pool, env_b.id).await,
        0,
        "env B served in the first sweep"
    );
    assert_eq!(
        rebuilds(&app.pool, app.env.id).await,
        49,
        "the flood drains one per sweep"
    );
}

#[tokio::test]
async fn counter_rebuild_recounts_one_subscriber_mute_aware() {
    let app = support::spawn().await;
    app.create_notification("usr_r", "noisy").await;
    app.create_notification("usr_r", "noisy").await;
    app.create_notification("usr_r", "important").await;
    let (unread, _, _) = app.counter_row("usr_r").await;
    assert_eq!(unread, 3);

    // Mute 'noisy' — the PUT enqueues counter_rebuild; counters stay
    // mute-blind until the rebuild lands (documented eventual exactness).
    let res = app
        .client
        .put(format!("{}/v1/inbox/preferences", app.base))
        .headers(app.subscriber_headers("usr_r"))
        .json(&json!({ "preferences": [ { "category": "noisy", "channel": "in_app", "enabled": false } ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(
        job_types(&app, app.env.id)
            .await
            .contains(&"counter_rebuild".to_owned())
    );

    app.drain_jobs().await;
    let (unread, unseen, _) = app.counter_row("usr_r").await;
    assert_eq!(unread, 1, "rebuild recounted mute-aware");
    assert_eq!(unseen, 1);
    assert_eq!(
        app.job_count(app.env.id).await,
        0,
        "rebuild deleted on completion"
    );
}

#[tokio::test]
async fn failing_jobs_back_off_and_park_at_max_attempts() {
    let app = support::spawn().await;
    let mut conn = app.pool.acquire().await.unwrap();
    // counter_rebuild with a malformed payload always errors.
    jobs::enqueue(
        &mut conn,
        app.env.id,
        jobs::TYPE_COUNTER_REBUILD,
        json!({ "subscriber_id": "not-a-uuid" }),
        None,
    )
    .await
    .unwrap();
    drop(conn);
    sqlx::query("UPDATE jobs SET max_attempts = 2 WHERE environment_id = $1")
        .bind(app.env.id)
        .execute(&app.pool)
        .await
        .unwrap();

    // Attempt 1: error → attempts=1, backed off into the future.
    assert!(
        worker::process_one(&app.pool, app.pubsub.as_ref(), &app.cfg, app.env.id)
            .await
            .is_err()
    );
    let (attempts, last_error): (i32, Option<String>) =
        sqlx::query_as("SELECT attempts, last_error FROM jobs WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(attempts, 1);
    assert!(last_error.is_some());

    // Force it due again; attempt 2 exhausts max_attempts → parked at
    // 'infinity' for Phase 3 DLQ replay, NOT deleted, NOT claimable.
    sqlx::query("UPDATE jobs SET run_at = now() WHERE environment_id = $1")
        .bind(app.env.id)
        .execute(&app.pool)
        .await
        .unwrap();
    assert!(
        worker::process_one(&app.pool, app.pubsub.as_ref(), &app.cfg, app.env.id)
            .await
            .is_err()
    );
    let parked: bool =
        sqlx::query_scalar("SELECT run_at = 'infinity' FROM jobs WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert!(parked, "exhausted job parked at infinity");
    assert_eq!(
        worker::sweep_once(&app.pool, app.pubsub.as_ref(), &app.cfg)
            .await
            .unwrap(),
        0,
        "parked jobs are not claimable"
    );
}

/// C1(a) regression: a row can be VISIBLE (deliver_at passed) while still
/// UNCOUNTED (deliver job not yet processed). Reading it in that window must
/// not decrement a counter that was never incremented.
#[tokio::test]
async fn reading_a_visible_but_undelivered_notification_leaves_no_drift() {
    let app = support::spawn().await;
    let deliver_at = Utc::now() + chrono::Duration::milliseconds(800);
    let res = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_ids": ["usr_w_a", "usr_w_b"], "category": "window",
                    "deliver_at": deliver_at.to_rfc3339() }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    // Visible now, deliver job still pending.
    let items = app.list_all_items("usr_w_a", 10).await;
    assert_eq!(items.len(), 1);
    let id = items[0]["id"].as_str().unwrap().to_owned();
    let res = app
        .post_inbox("usr_w_a", &format!("/v1/inbox/notifications/{id}/read"))
        .await;
    assert_eq!(res.status(), 204);

    app.drain_jobs().await;

    // usr_w_a read theirs in the window: 0 unread. usr_w_b never read: 1.
    let (unread_a, unseen_a, _) = app.counter_row("usr_w_a").await;
    assert_eq!(
        (unread_a, unseen_a),
        (0, 1),
        "read-in-window must net to zero"
    );
    let (unread_b, unseen_b, _) = app.counter_row("usr_w_b").await;
    assert_eq!((unread_b, unseen_b), (1, 1));
    let (counts_a, _) = app.counts("usr_w_a").await;
    assert_eq!(counts_a, 0, "no negative-drift masking");
}

/// C1(b) regression: a counter_rebuild running inside the deliver window must
/// not count rows the deliver job will bump again afterwards.
#[tokio::test]
async fn rebuild_during_the_deliver_window_does_not_double_count() {
    let app = support::spawn().await;
    let deliver_at = Utc::now() + chrono::Duration::milliseconds(800);
    let res = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_id": "usr_rw", "category": "window",
                    "deliver_at": deliver_at.to_rfc3339() }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    // Force the rebuild to run FIRST: push the deliver job into the near
    // future (far enough that a loaded CI runner cannot make it due before
    // the rebuild claim below), enqueue a rebuild, process it, then let the
    // deliver job run.
    sqlx::query(
        "UPDATE jobs SET run_at = now() + interval '2 seconds'
                  WHERE environment_id = $1 AND job_type = 'deliver'",
    )
    .bind(app.env.id)
    .execute(&app.pool)
    .await
    .unwrap();
    let subscriber: Uuid = sqlx::query_scalar(
        "SELECT id FROM subscribers WHERE environment_id = $1 AND subscriber_id = 'usr_rw'",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    let mut conn = app.pool.acquire().await.unwrap();
    jobs::enqueue(
        &mut conn,
        app.env.id,
        jobs::TYPE_COUNTER_REBUILD,
        json!({ "subscriber_id": subscriber }),
        None,
    )
    .await
    .unwrap();
    drop(conn);
    // Process exactly ONE job: the rebuild is the only due job (the deliver
    // job is 300ms out), so a single claim runs it in isolation.
    assert!(
        worker::process_one(&app.pool, app.pubsub.as_ref(), &app.cfg, app.env.id)
            .await
            .unwrap()
    );

    let (unread, _, _) = app.counter_row("usr_rw").await;
    assert_eq!(
        unread, 0,
        "rebuild must not count rows still owned by a deliver job"
    );

    tokio::time::sleep(std::time::Duration::from_millis(2_200)).await;
    app.drain_jobs().await; // deliver

    let (unread, unseen, _) = app.counter_row("usr_rw").await;
    assert_eq!(
        (unread, unseen),
        (1, 1),
        "deliver is the single bookkeeper: exactly once"
    );
}

#[tokio::test]
async fn hint_jobs_publish_through_the_pubsub_plane() {
    let app = support::spawn().await; // Redis-less: LISTEN/NOTIFY path
    let mut rx = app.pubsub.subscribe();
    app.create_notification("usr_h", "x").await;
    app.drain_jobs().await;

    let hint = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
        .await
        .expect("hint within 3s")
        .expect("hint received");
    assert_eq!(hint.environment_id, app.env.id);
    assert_eq!(hint.reason, "notification");
    assert!(hint.subscriber_id.is_some());
    assert_eq!(app.job_count(app.env.id).await, 0);
}

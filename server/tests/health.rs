//! Task 7: /healthz, /readyz, and the Redis-degradation contract — Redis loss
//! may DELAY hints; it must never LOSE data, and it must never fail
//! readiness.

mod support;

use std::time::Duration;

#[tokio::test]
async fn health_and_readiness_respond_on_a_fresh_deploy() {
    let app = support::spawn().await;
    let res = app
        .client
        .get(format!("{}/healthz", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "ok");

    let res = app
        .client
        .get(format!("{}/readyz", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "Postgres reachable + migrations applied ⇒ ready"
    );
}

#[tokio::test]
async fn redis_loss_delays_hints_but_loses_nothing_and_keeps_readiness() {
    let app = support::spawn_with_redis().await;
    app.create_notification("usr_z", "seed").await;
    app.drain_jobs().await;

    // Kill Redis.
    let redis = app.redis.as_ref().expect("redis container");
    redis
        .stop_with_timeout(Some(1))
        .await
        .expect("stopping redis");

    // Creates still succeed — Postgres is the source of truth.
    app.create_notification("usr_z", "during.outage").await;
    let items = app.list_all_items("usr_z", 10).await;
    assert_eq!(items.len(), 2, "no data loss during the outage");
    let (unread, _) = app.counts("usr_z").await;
    assert_eq!(unread, 2);

    // Readiness is NOT Redis-gated.
    let res = app
        .client
        .get(format!("{}/readyz", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "Redis down must not fail readiness");

    // The hint survives as a job row (transactional outbox, not dual-write).
    assert!(
        app.job_count(app.env.id).await >= 1,
        "hint job must persist through the outage"
    );

    // Restore Redis and wait until the app's own client answers again. A
    // fixed sleep flakes under machine load: fred's reconnect backoff slot
    // can exceed the 5s command timeout, stacking job retries past the
    // assertion window below.
    redis.start().await.expect("restarting redis");
    for attempt in 0u32.. {
        match app
            .pubsub
            .try_acquire_debounce(
                &format!("recovery-probe-{attempt}"),
                Duration::from_millis(1),
            )
            .await
        {
            Ok(_) => break,
            Err(_) if attempt < 12 => tokio::time::sleep(Duration::from_millis(250)).await,
            Err(err) => panic!("hint plane did not recover: {err:#}"),
        }
    }
    sqlx::query("UPDATE jobs SET run_at = now() WHERE environment_id = $1")
        .bind(app.env.id)
        .execute(&app.pool)
        .await
        .unwrap();

    let mut rx = app.pubsub.subscribe();
    app.spawn_worker();
    // Generous window: a publish attempted before fred finishes reconnecting
    // hits the 5s command timeout and backs off via fail_job before the next
    // try, and under full-suite load the container restart itself is slow.
    let hint = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if let Ok(hint) = rx.recv().await
                && hint.environment_id == app.env.id
            {
                return hint;
            }
        }
    })
    .await
    .expect("delayed hint delivered after Redis recovery");
    assert_eq!(hint.reason, "notification");

    // Everything drained, nothing lost.
    let items = app.list_all_items("usr_z", 10).await;
    assert_eq!(items.len(), 2);
}

#[tokio::test]
async fn readiness_fails_when_postgres_is_unreachable() {
    let app = support::spawn().await;
    let res = app
        .client
        .get(format!("{}/readyz", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    app._pg_stop().await;
    let res = app
        .client
        .get(format!("{}/readyz", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 503, "Postgres down ⇒ not ready");
}

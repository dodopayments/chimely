//! Phase 3: graceful shutdown — readiness flips BEFORE the listener closes
//! (load balancers drain first), in-flight requests finish, SSE closes with
//! a retry directive (covered in sse.rs), workers stop claiming.

mod support;

use std::time::Duration;

use dronte::worker;
use serde_json::json;

#[tokio::test]
async fn readiness_flips_to_503_while_the_listener_still_serves() {
    let app = support::spawn().await;
    let res = app
        .client
        .get(format!("{}/readyz", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    // Phase 1 of the drain: readiness 503, everything else still up.
    app.draining_tx.send(true).unwrap();
    let res = app
        .client
        .get(format!("{}/readyz", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 503, "draining replica reports not-ready");
    let res = app
        .client
        .get(format!("{}/healthz", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "liveness unaffected by draining");
    let res = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_id": "usr_g", "category": "during-grace" }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "traffic still served during the grace");

    // Phase 2: the listener closes; new connections fail.
    app.shutdown_tx.send(true).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let err = app
        .client
        .get(format!("{}/healthz", app.base))
        .timeout(Duration::from_millis(500))
        .send()
        .await;
    assert!(err.is_err(), "listener must be closed after shutdown");
}

#[tokio::test]
async fn workers_stop_claiming_on_shutdown_and_leave_jobs_for_successors() {
    let app = support::spawn().await;

    // A running worker loop observes the flip and exits. Its own shutdown
    // switch: the HTTP listener must stay up for the assertions below.
    let (worker_stop_tx, worker_stop_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(worker::run(
        app.pool.clone(),
        app.pubsub.clone(),
        app.cfg.clone(),
        worker_stop_rx,
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    worker_stop_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("worker loop exits promptly on shutdown")
        .unwrap();

    // Work enqueued after the stop is untouched — the next replica's
    // workers (or a restart) claim it; nothing is lost or half-done.
    app.create_notification("usr_w", "after-stop").await;
    assert!(
        app.job_count(app.env.id).await >= 1,
        "outbox row persists for the successor"
    );
    app.drain_jobs().await;
    app.assert_consistent().await;
}

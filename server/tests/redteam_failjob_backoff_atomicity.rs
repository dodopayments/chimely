//! A job that just failed must not be re-claimable until its backoff is in
//! place. fail_job bumps `attempts` and pushes `run_at` into the future in ONE
//! atomic UPDATE, holding the row lock until commit. A two-statement shape that
//! commits the attempts bump first leaves a window where the still-due row
//! (run_at <= now() with attempts already incremented) is claimable before its
//! backoff lands.
//!
//! A sequential test cannot catch this: the existing
//! failing_jobs_back_off_with_jitter_then_park test passes against both shapes,
//! because once fail_job returns the row looks identical either way. The defect
//! lives only in the committed intermediate state, visible only to a concurrent
//! claimer.
//!
//! A non-consuming observer issues the production claim (FOR UPDATE SKIP LOCKED
//! ... run_at <= now()) on a tight loop and rolls back. The backoff is large
//! enough that a failed job's run_at never becomes due again during the test,
//! so a claimable row carrying attempts >= 1 can only be the two-statement
//! window. The atomic fix makes that state impossible to commit. The repeated
//! fail cycles give the observer many chances to catch the window if the
//! two-statement shape returns.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use chimely::worker;

const ROUNDS: usize = 200;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_job_is_never_reclaimable_before_its_backoff_lands() {
    // Backoff large enough never to elapse during the test. Every failed job's
    // run_at lands ~15-30s out, so a claim seeing a due row with attempts >= 1
    // can only be the two-statement window.
    let app = support::spawn_configured(false, |cfg| {
        cfg.retry_backoff_base = Duration::from_secs(30);
        cfg.retry_backoff_cap = Duration::from_secs(60);
    })
    .await;
    let env = app.env.id;

    // 'bogus' is an unknown job type, so process_one errors and routes through
    // fail_job. max_attempts is high so the job never parks.
    let job_id = chimely::ids::new_uuid();
    sqlx::query(
        "INSERT INTO jobs (environment_id, id, job_type, payload, run_at,
                           attempts, max_attempts)
         VALUES ($1, $2, 'bogus', '{}'::jsonb, now(), 0, 1000)",
    )
    .bind(env)
    .bind(job_id)
    .execute(&app.pool)
    .await
    .unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    // Highest `attempts` the observer found on a claimable row. Stays 0 iff no
    // failed-but-still-due row was ever exposed.
    let max_due_attempts = Arc::new(AtomicI32::new(0));

    let observer = tokio::spawn({
        let pool = app.pool.clone();
        let stop = stop.clone();
        let max_due_attempts = max_due_attempts.clone();
        async move {
            while !stop.load(Ordering::Relaxed) {
                let Ok(mut tx) = pool.begin().await else {
                    continue;
                };
                // Production claim predicate, non-consuming: reads `attempts`
                // and rolls back so the job is never advanced.
                let claimed: Option<(i32,)> = sqlx::query_as(
                    "SELECT attempts FROM jobs
                      WHERE environment_id = $1 AND run_at <= now()
                      ORDER BY run_at
                      LIMIT 1
                      FOR UPDATE SKIP LOCKED",
                )
                .bind(env)
                .fetch_optional(&mut *tx)
                .await
                .unwrap_or(None);
                if let Some((attempts,)) = claimed {
                    max_due_attempts.fetch_max(attempts, Ordering::Relaxed);
                }
                let _ = tx.rollback().await;
            }
        }
    });

    // Each round resets the job to a fresh due, un-failed state, then fails it
    // once via the real worker path.
    for _ in 0..ROUNDS {
        sqlx::query(
            "UPDATE jobs SET attempts = 0, run_at = now(), last_error = NULL
              WHERE environment_id = $1 AND id = $2",
        )
        .bind(env)
        .bind(job_id)
        .execute(&app.pool)
        .await
        .unwrap();
        // Under the fix, attempts++ and run_at += backoff commit together. The
        // two-statement shape commits the bump first, leaving (attempts = 1,
        // run_at = now()) re-claimable.
        let _ = worker::process_one(&app.pool, app.pubsub.as_ref(), &app.cfg, env).await;
    }

    stop.store(true, Ordering::Relaxed);
    observer.await.unwrap();

    let seen = max_due_attempts.load(Ordering::Relaxed);
    assert_eq!(
        seen, 0,
        "a concurrent claim observed a failed job (attempts = {seen}) still due \
         (run_at <= now()) before its backoff was installed: fail_job must bump \
         attempts and back off run_at in ONE atomic statement, never two"
    );
}

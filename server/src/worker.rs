//! The job worker: FOR UPDATE SKIP LOCKED claims, round-robin across
//! environments with pending work, DELETE on completion — never status-flag
//! in place (completed work leaves no row).
//!
//! Fairness: each sweep claims at most ONE job per environment with pending
//! work, then moves to the next environment — a broadcast burst from
//! 'dashboard-prod' cannot starve 'mobile-prod' real-time jobs. Large jobs
//! cooperate with the same rule: `deliver` processes one `progress_cursor`
//! chunk per claim and commits, so even a huge fan-out yields between chunks.
//!
//! Exactly-once side effects are keyed by job deletion: the deliver job's
//! counter bumps commit in the SAME transaction that deletes (or
//! cursor-advances) the job row. Hints are at-least-once by design (refetch
//! triggers, not transports).

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::jobs::{TYPE_COUNTER_REBUILD, TYPE_DELIVER, TYPE_HINT};
use crate::pubsub::{Hint, PubSub};
use crate::{ids, jobs};

/// Rows per deliver-chunk transaction: never one giant transaction, never N
/// tiny rows.
pub const DELIVER_CHUNK: usize = 500;

enum Outcome {
    /// Effects applied. The job row is deleted in this transaction.
    Done,
    /// A chunk was processed and the cursor advanced. The job stays claimable.
    Continue,
    /// Not processed now (debounce window). Runs again at the given time
    /// with the given payload, deferred rather than dropped.
    Defer {
        run_at: DateTime<Utc>,
        payload: Option<Value>,
    },
}

pub async fn run(
    pool: PgPool,
    pubsub: std::sync::Arc<dyn PubSub>,
    cfg: std::sync::Arc<Config>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        let processed = match sweep_once(&pool, pubsub.as_ref(), &cfg).await {
            Ok(n) => n,
            Err(err) => {
                tracing::error!(error = ?err, "worker sweep failed");
                0
            }
        };
        if processed == 0 {
            tokio::select! {
                _ = tokio::time::sleep(cfg.worker_poll_interval) => {}
                _ = shutdown.changed() => return,
            }
        }
    }
}

/// One fair pass: at most one job per environment with due work. Returns the
/// number of jobs touched (tests drive this directly).
pub async fn sweep_once(pool: &PgPool, pubsub: &dyn PubSub, cfg: &Config) -> anyhow::Result<u64> {
    let envs: Vec<Uuid> =
        sqlx::query_scalar!(r#"SELECT DISTINCT environment_id FROM jobs WHERE run_at <= now()"#)
            .fetch_all(pool)
            .await?;
    let mut processed = 0;
    for env in envs {
        match process_one(pool, pubsub, cfg, env).await {
            Ok(true) => processed += 1,
            Ok(false) => {}
            Err(err) => {
                tracing::error!(error = ?err, environment = %env, "job processing failed");
            }
        }
    }
    Ok(processed)
}

/// Claim and process a single job for one environment. The claim, the job's
/// effects, and its deletion share one transaction: a worker killed mid-job
/// rolls back wholesale and the job is simply re-claimed.
pub async fn process_one(
    pool: &PgPool,
    pubsub: &dyn PubSub,
    cfg: &Config,
    env: Uuid,
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let job = sqlx::query!(
        r#"SELECT environment_id, id, job_type, payload, attempts, max_attempts,
                  progress_cursor
             FROM jobs
            WHERE environment_id = $1 AND run_at <= now()
            ORDER BY run_at
            LIMIT 1
            FOR UPDATE SKIP LOCKED"#,
        env,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(job) = job else {
        return Ok(false);
    };

    let outcome = match job.job_type.as_str() {
        TYPE_HINT => {
            // Coalesce exact-duplicate pending hints first: a burst of N
            // creates for one subscriber enqueues N identical jobs, and one
            // publish (or one deferred trailing publish) covers them all —
            // debounce means at most one hint per subscriber per window,
            // never one publish per row. SKIP LOCKED: a duplicate claimed by
            // another worker is left alone (it will publish or coalesce on
            // its own), because a blocking DELETE here is an AB-BA deadlock
            // between two workers holding each other's duplicates.
            sqlx::query!(
                r#"DELETE FROM jobs
                    WHERE (environment_id, id) IN (
                        SELECT environment_id, id FROM jobs
                         WHERE environment_id = $1 AND job_type = 'hint'
                           AND id <> $2 AND payload = $3
                         FOR UPDATE SKIP LOCKED)"#,
                env,
                job.id,
                &job.payload,
            )
            .execute(&mut *tx)
            .await?;
            process_hint(pubsub, cfg, env, &job.payload).await
        }
        TYPE_DELIVER => {
            process_deliver(
                &mut tx,
                env,
                job.id,
                &job.payload,
                job.progress_cursor.as_ref(),
            )
            .await
        }
        TYPE_COUNTER_REBUILD => process_counter_rebuild(&mut tx, env, &job.payload).await,
        other => Err(anyhow::anyhow!("unknown job type: {other}")),
    };

    match outcome {
        Ok(Outcome::Done) => {
            sqlx::query!(
                "DELETE FROM jobs WHERE environment_id = $1 AND id = $2",
                env,
                job.id
            )
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok(true)
        }
        Ok(Outcome::Continue) => {
            tx.commit().await?;
            Ok(true)
        }
        Ok(Outcome::Defer { run_at, payload }) => {
            sqlx::query!(
                r#"UPDATE jobs SET run_at = $3, payload = COALESCE($4, payload)
                    WHERE environment_id = $1 AND id = $2"#,
                env,
                job.id,
                run_at,
                payload,
            )
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok(true)
        }
        Err(err) => {
            tx.rollback().await?;
            fail_job(pool, env, job.id, &err).await?;
            Err(err)
        }
    }
}

/// Failure bookkeeping happens OUTSIDE the rolled-back transaction. Exhausted
/// jobs are parked at run_at = 'infinity' (kept for the Phase 3 DLQ replay
/// tooling). Real retry/backoff policy is also Phase 3.
async fn fail_job(pool: &PgPool, env: Uuid, id: Uuid, err: &anyhow::Error) -> anyhow::Result<()> {
    sqlx::query!(
        r#"UPDATE jobs SET
               attempts = attempts + 1,
               last_error = $3,
               run_at = CASE
                   WHEN attempts + 1 >= max_attempts THEN 'infinity'::timestamptz
                   ELSE now() + make_interval(secs => 5.0 * (attempts + 1))
               END
         WHERE environment_id = $1 AND id = $2"#,
        env,
        id,
        format!("{err:#}"),
    )
    .execute(pool)
    .await?;
    Ok(())
}

// =============================================================================
// hint — debounced pub/sub publish
// =============================================================================

/// Publishes hints for the subscribers in the payload (`null` = env-wide, the
/// broadcast case). Debounce is per key: at most one published hint per
/// subscriber (or per environment for env-wide) per window. Suppressed keys
/// are deferred to the window's end, never dropped.
async fn process_hint(
    pubsub: &dyn PubSub,
    cfg: &Config,
    env: Uuid,
    payload: &Value,
) -> anyhow::Result<Outcome> {
    let reason = payload["reason"]
        .as_str()
        .unwrap_or("notification")
        .to_owned();
    let subscriber_ids: Option<Vec<Uuid>> = match &payload["subscriber_ids"] {
        Value::Null => None,
        v => Some(serde_json::from_value(v.clone())?),
    };

    let targets: Vec<Option<Uuid>> = match subscriber_ids {
        None => vec![None],
        Some(subs) => subs.into_iter().map(Some).collect(),
    };
    let mut deferred: Vec<Uuid> = Vec::new();
    let mut deferred_env_wide = false;
    for target in targets {
        let key = match target {
            Some(sub) => format!("{env}:{sub}"),
            None => format!("{env}:*"),
        };
        if pubsub.try_acquire_debounce(&key, cfg.hint_debounce).await? {
            pubsub
                .publish(&Hint {
                    environment_id: env,
                    subscriber_id: target,
                    reason: reason.clone(),
                })
                .await?;
        } else {
            match target {
                Some(sub) => deferred.push(sub),
                None => deferred_env_wide = true,
            }
        }
    }

    if deferred.is_empty() && !deferred_env_wide {
        return Ok(Outcome::Done);
    }
    Ok(Outcome::Defer {
        run_at: Utc::now() + cfg.hint_debounce,
        payload: Some(json!({
            "reason": reason,
            "subscriber_ids": if deferred_env_wide { Value::Null } else { json!(deferred) },
        })),
    })
}

// =============================================================================
// deliver — scheduled notifications coming due
// =============================================================================

/// One chunk per claim: conditional counter bumps for the chunk's rows, then
/// either advance `progress_cursor` (more chunks) or enqueue the hint and
/// signal deletion (job deletion is the exactly-once key). A crash before
/// commit rolls back bumps and cursor together, so replay is safe.
async fn process_deliver(
    tx: &mut sqlx::PgConnection,
    env: Uuid,
    job_id: Uuid,
    payload: &Value,
    cursor: Option<&Value>,
) -> anyhow::Result<Outcome> {
    let all_ids: Vec<Uuid> = serde_json::from_value(payload["notification_ids"].clone())?;
    let offset = cursor.and_then(|c| c["offset"].as_u64()).unwrap_or(0) as usize;
    let chunk: Vec<Uuid> = all_ids
        .iter()
        .skip(offset)
        .take(DELIVER_CHUNK)
        .copied()
        .collect();

    if !chunk.is_empty() {
        // Lock the affected counters rows FIRST, in their own statement and
        // in a stable order. The bump statement below then starts after the
        // locks are held and reads a fresh snapshot, so a concurrent
        // mark-read commit is never missed (EvalPlanQual rechecks re-read
        // the locked row but NOT the notifications subqueries).
        sqlx::query!(
            r#"SELECT c.subscriber_id FROM subscriber_counters c
                WHERE c.environment_id = $1
                  AND c.subscriber_id IN (
                      SELECT n.subscriber_id FROM notifications n
                       WHERE n.environment_id = $1 AND n.id = ANY($2))
                ORDER BY c.subscriber_id
                FOR UPDATE"#,
            env,
            &chunk,
        )
        .fetch_all(&mut *tx)
        .await?;
        // The conditional bump mirrors the immediate-insert rule, evaluated
        // at deliver time: a row already marked read (visible while the
        // worker lagged) or below a moved watermark must not be counted.
        // mark_notification_read skips its decrement for rows still owned by
        // this job, so this bump is the single bookkeeper for them.
        sqlx::query!(
            r#"UPDATE subscriber_counters c SET
                   unread_direct_count = c.unread_direct_count + (
                       SELECT count(*) FROM notifications n
                        WHERE n.environment_id = c.environment_id
                          AND n.subscriber_id  = c.subscriber_id
                          AND n.id = ANY($2)
                          AND n.read_at IS NULL
                          AND n.visible_at > c.read_watermark)::int,
                   unseen_direct_count = c.unseen_direct_count + (
                       SELECT count(*) FROM notifications n
                        WHERE n.environment_id = c.environment_id
                          AND n.subscriber_id  = c.subscriber_id
                          AND n.id = ANY($2)
                          AND n.visible_at > c.seen_watermark)::int,
                   updated_at = now()
             WHERE c.environment_id = $1
               AND c.subscriber_id IN (
                   SELECT n.subscriber_id FROM notifications n
                    WHERE n.environment_id = $1 AND n.id = ANY($2))"#,
            env,
            &chunk,
        )
        .execute(&mut *tx)
        .await?;
    }

    let new_offset = offset + chunk.len();
    if new_offset < all_ids.len() {
        sqlx::query!(
            r#"UPDATE jobs SET progress_cursor = $3 WHERE environment_id = $1 AND id = $2"#,
            env,
            job_id,
            json!({ "offset": new_offset }),
        )
        .execute(&mut *tx)
        .await?;
        return Ok(Outcome::Continue);
    }

    let subscribers: Vec<Uuid> = sqlx::query_scalar!(
        r#"SELECT DISTINCT subscriber_id FROM notifications
            WHERE environment_id = $1 AND id = ANY($2)"#,
        env,
        &all_ids,
    )
    .fetch_all(&mut *tx)
    .await?;
    jobs::enqueue_hint(tx, env, &subscribers, "notification").await?;
    Ok(Outcome::Done)
}

// =============================================================================
// counter_rebuild — exact recount of one subscriber
// =============================================================================

/// Mute-aware exact recount. Counters otherwise ignore category mutes, and
/// this job is the eventual-exactness path after a preference flip.
async fn process_counter_rebuild(
    tx: &mut sqlx::PgConnection,
    env: Uuid,
    payload: &Value,
) -> anyhow::Result<Outcome> {
    let subscriber: Uuid = serde_json::from_value(payload["subscriber_id"].clone())?;
    // Lock the counters row FIRST (same reasoning as the deliver bump: the
    // recount statement must see a snapshot taken after the lock).
    sqlx::query!(
        r#"SELECT 1 AS one FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        env,
        subscriber,
    )
    .fetch_optional(&mut *tx)
    .await?;
    // Visible rows still owned by a pending deliver job are UNCOUNTED (the
    // deliver bump is their single bookkeeper) and must be excluded from the
    // recount, or the deliver bump would add them a second time. Ownership
    // is positional: ids at index >= progress_cursor offset are unprocessed.
    sqlx::query!(
        r#"UPDATE subscriber_counters c SET
               unread_direct_count = (
                   SELECT count(*) FROM notifications n
                    WHERE n.environment_id = $1 AND n.subscriber_id = $2
                      AND n.visible_at <= now()
                      AND n.read_at IS NULL
                      AND n.visible_at > c.read_watermark
                      AND NOT EXISTS (SELECT 1 FROM preferences p
                            WHERE p.environment_id = $1 AND p.subscriber_id = $2
                              AND p.category = n.category AND p.channel = 'in_app'
                              AND p.enabled = false)
                      AND NOT EXISTS (SELECT 1 FROM jobs j
                            CROSS JOIN LATERAL jsonb_array_elements_text(
                                j.payload->'notification_ids') WITH ORDINALITY AS t(nid, idx)
                            WHERE j.environment_id = $1 AND j.job_type = 'deliver'
                              AND t.nid = n.id::text
                              AND (t.idx - 1) >=
                                  COALESCE((j.progress_cursor->>'offset')::bigint, 0)))::int,
               unseen_direct_count = (
                   SELECT count(*) FROM notifications n
                    WHERE n.environment_id = $1 AND n.subscriber_id = $2
                      AND n.visible_at <= now()
                      AND n.visible_at > c.seen_watermark
                      AND NOT EXISTS (SELECT 1 FROM preferences p
                            WHERE p.environment_id = $1 AND p.subscriber_id = $2
                              AND p.category = n.category AND p.channel = 'in_app'
                              AND p.enabled = false)
                      AND NOT EXISTS (SELECT 1 FROM jobs j
                            CROSS JOIN LATERAL jsonb_array_elements_text(
                                j.payload->'notification_ids') WITH ORDINALITY AS t(nid, idx)
                            WHERE j.environment_id = $1 AND j.job_type = 'deliver'
                              AND t.nid = n.id::text
                              AND (t.idx - 1) >=
                                  COALESCE((j.progress_cursor->>'offset')::bigint, 0)))::int,
               updated_at = now()
         WHERE c.environment_id = $1 AND c.subscriber_id = $2"#,
        env,
        subscriber,
    )
    .execute(&mut *tx)
    .await?;
    jobs::enqueue_hint(tx, env, &[subscriber], "read_state").await?;
    Ok(Outcome::Done)
}

/// Test hook: claim the next due job for `env` and apply ONE deliver chunk,
/// then abort instead of committing. Simulates a worker killed mid-deliver.
pub async fn crash_mid_deliver(pool: &PgPool, env: Uuid) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let job = sqlx::query!(
        r#"SELECT id, payload, progress_cursor FROM jobs
            WHERE environment_id = $1 AND job_type = 'deliver' AND run_at <= now()
            ORDER BY run_at LIMIT 1 FOR UPDATE SKIP LOCKED"#,
        env,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(job) = job else {
        return Ok(false);
    };
    process_deliver(
        &mut tx,
        env,
        job.id,
        &job.payload,
        job.progress_cursor.as_ref(),
    )
    .await?;
    tx.rollback().await?;
    Ok(true)
}

/// Test hook: a hint job id for assertions.
pub fn _job_typeid(id: Uuid) -> String {
    ids::typeid(ids::JOB, id)
}

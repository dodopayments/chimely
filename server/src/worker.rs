//! The job worker: FOR UPDATE SKIP LOCKED claims, round-robin across
//! environments with pending work, DELETE on completion — never status-flag
//! in place (completed work leaves no row).
//!
//! Fairness: each sweep claims at most ONE job per environment with pending
//! work, then moves to the next environment — a broadcast burst from
//! 'dashboard-prod' cannot starve 'mobile-prod' real-time jobs. Large jobs
//! cooperate with the same rule: `deliver` and `timeline` process one
//! `progress_cursor` chunk per claim and commit, so even a huge fan-out
//! yields between chunks.
//!
//! Exactly-once side effects are keyed by job deletion: the deliver job's
//! counter bumps and every timeline append commit in the SAME transaction
//! that deletes (or cursor-advances) the job row. Hints are at-least-once by
//! design (refetch triggers, not transports).
//!
//! Failed jobs retry with jittered exponential backoff; a job exhausting
//! max_attempts moves to dead_letters for replay (`dlq`).

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use tracing::Instrument as _;
use uuid::Uuid;

use crate::config::Config;
use crate::jobs::{TYPE_COUNTER_REBUILD, TYPE_DELIVER, TYPE_HINT, TYPE_TIMELINE};
use crate::pubsub::{Hint, PubSub};
use crate::{ids, jobs, telemetry, timeline};

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
///
/// Environment discovery is a loose index scan (recursive CTE over
/// jobs_claim_idx): one index probe per distinct environment. A plain
/// DISTINCT would scan the whole backlog on EVERY sweep, and with one claim
/// per environment per sweep that makes draining a deep single-environment
/// backlog quadratic.
pub async fn sweep_once(pool: &PgPool, pubsub: &dyn PubSub, cfg: &Config) -> anyhow::Result<u64> {
    let envs = sqlx::query!(
        r#"WITH RECURSIVE pending AS (
               (SELECT environment_id FROM jobs WHERE run_at <= now()
                 ORDER BY environment_id LIMIT 1)
               UNION ALL
               SELECT (SELECT j.environment_id FROM jobs j
                        WHERE j.environment_id > p.environment_id
                          AND j.run_at <= now()
                        ORDER BY j.environment_id LIMIT 1)
                 FROM pending p
                WHERE p.environment_id IS NOT NULL)
           SELECT p.environment_id AS "environment_id!", e.slug
             FROM pending p JOIN environments e ON e.id = p.environment_id"#
    )
    .fetch_all(pool)
    .await?;
    let mut processed = 0;
    for env in envs {
        match process_one(pool, pubsub, cfg, env.environment_id).await {
            Ok(true) => {
                processed += 1;
                metrics::counter!("dronte_jobs_processed_total", "environment" => env.slug)
                    .increment(1);
            }
            Ok(false) => {}
            Err(err) => {
                tracing::error!(error = ?err, environment = %env.environment_id, "job processing failed");
                metrics::counter!("dronte_jobs_failed_total", "environment" => env.slug)
                    .increment(1);
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
                  progress_cursor, run_at, created_at
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

    // Claim-to-due latency, the fairness signal: a starved environment shows
    // an unbounded wait here long before users notice late hints.
    let wait = (Utc::now() - job.run_at).num_milliseconds().max(0) as f64 / 1000.0;
    metrics::histogram!("dronte_job_wait_seconds", "job_type" => job.job_type.clone()).record(wait);

    // The span joins the trace that enqueued the job (traceparent rides in
    // the payload), so one trace covers ingest -> outbox -> worker -> hint.
    let span = tracing::info_span!(
        "job.process",
        job.id = %ids::typeid(ids::JOB, job.id),
        job.job_type = %job.job_type,
        environment.id = %env,
    );
    if let Some(traceparent) = job.payload.get("_traceparent").and_then(Value::as_str) {
        telemetry::set_remote_parent(&span, traceparent);
    }

    let outcome = async {
        match job.job_type.as_str() {
            TYPE_HINT => {
                process_hint(
                    &mut tx,
                    pubsub,
                    cfg,
                    env,
                    job.id,
                    &job.payload,
                    job.created_at,
                )
                .await
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
            TYPE_TIMELINE => {
                process_timeline(
                    &mut tx,
                    env,
                    job.id,
                    &job.payload,
                    job.progress_cursor.as_ref(),
                )
                .await
            }
            other => Err(anyhow::anyhow!("unknown job type: {other}")),
        }
    }
    .instrument(span)
    .await;

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
            fail_job(pool, cfg, env, job.id, &err).await?;
            Err(err)
        }
    }
}

/// Failure bookkeeping happens OUTSIDE the rolled-back transaction, but
/// inside ONE transaction of its own: the backoff lands in the same atomic
/// UPDATE that bumps `attempts`, and the row lock it takes is held until
/// the park decision commits. A concurrent claim (FOR UPDATE SKIP LOCKED)
/// therefore skips the row for the whole bookkeeping window, so a failed
/// job can never be re-claimed before its backoff is in place.
///
/// Backoff is exponential with equal jitter, computed in SQL from the
/// pre-increment attempt count: attempt n sleeps in `[exp/2, exp]` where
/// `exp = min(cap, base * 2^(n-1))`. The floor keeps a hot failure from
/// hammering; the jitter keeps a burst of failures from retrying in
/// lockstep. A job exhausting max_attempts moves to dead_letters instead
/// (a parked job is not a completed job, and it must not live in the hot
/// claim path).
async fn fail_job(
    pool: &PgPool,
    cfg: &Config,
    env: Uuid,
    id: Uuid,
    err: &anyhow::Error,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    // `attempts` on the right-hand side is the pre-increment value, so
    // power(2, attempts) is 2^(n-1) for attempt n.
    let row = sqlx::query!(
        r#"UPDATE jobs SET
               attempts = attempts + 1,
               last_error = $3,
               run_at = now() + make_interval(secs =>
                   least($5::float8, $4::float8 * power(2::float8, attempts::float8))
                       * (0.5 + random() * 0.5))
            WHERE environment_id = $1 AND id = $2
            RETURNING job_type, attempts, max_attempts"#,
        env,
        id,
        format!("{err:#}"),
        cfg.retry_backoff_base.as_secs_f64(),
        cfg.retry_backoff_cap.as_secs_f64(),
    )
    .fetch_optional(&mut *tx)
    .await?;
    // Raced away (another worker claimed and finished it): nothing to record.
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(());
    };

    if row.attempts >= row.max_attempts {
        sqlx::query!(
            r#"WITH parked AS (
                   DELETE FROM jobs
                    WHERE environment_id = $1 AND id = $2
                      AND attempts >= max_attempts
                    RETURNING environment_id, id, job_type, payload, attempts,
                              max_attempts, last_error, progress_cursor, created_at)
               INSERT INTO dead_letters
                      (environment_id, id, job_type, payload, attempts,
                       max_attempts, last_error, progress_cursor, created_at)
               SELECT environment_id, id, job_type, payload, attempts,
                      max_attempts, COALESCE(last_error, ''), progress_cursor,
                      created_at
                 FROM parked"#,
            env,
            id,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        metrics::counter!("dronte_jobs_parked_total", "job_type" => row.job_type.clone())
            .increment(1);
        tracing::error!(
            environment = %env, job = %id, job_type = %row.job_type,
            attempts = row.attempts, error = ?err,
            "job exhausted max_attempts; parked in dead_letters"
        );
        return Ok(());
    }

    tx.commit().await?;
    metrics::counter!("dronte_jobs_retried_total", "job_type" => row.job_type).increment(1);
    Ok(())
}

// =============================================================================
// hint — debounced pub/sub publish
// =============================================================================

/// Publishes hints for the subscribers in the payload (`null` = env-wide, the
/// broadcast case). Debounce is per key: at most one published hint per
/// subscriber (or per environment for env-wide) per window. Suppressed keys
/// are deferred to the window's end, never dropped.
///
/// `notification_ids` in the payload are the direct notifications this hint
/// announces. Their `delivered_hint` timeline rows are appended in this same
/// claim transaction, so the append commits exactly once: if and only if the
/// job row's deletion (or its deferred-payload rewrite) commits.
#[allow(clippy::too_many_arguments)]
async fn process_hint(
    tx: &mut sqlx::PgConnection,
    pubsub: &dyn PubSub,
    cfg: &Config,
    env: Uuid,
    job_id: Uuid,
    payload: &Value,
    job_created_at: DateTime<Utc>,
) -> anyhow::Result<Outcome> {
    let reason = payload["reason"]
        .as_str()
        .unwrap_or("notification")
        .to_owned();
    let subscriber_ids: Option<Vec<Uuid>> = match &payload["subscriber_ids"] {
        Value::Null => None,
        v => Some(serde_json::from_value(v.clone())?),
    };
    let mut notification_ids: Vec<Uuid> = match &payload["notification_ids"] {
        Value::Null => Vec::new(),
        v => serde_json::from_value(v.clone())?,
    };

    // Coalesce pending hints for the same targets and reason first: a burst
    // of N creates for one subscriber enqueues N hint jobs, and one publish
    // (or one deferred trailing publish) covers them all. Debounce means at
    // most one hint per subscriber per window, never one publish per row.
    // Their notification id sets merge into this job so no delivered_hint
    // row is lost. SKIP LOCKED: a duplicate claimed by another worker is
    // left alone (it will publish or coalesce on its own), because a
    // blocking DELETE here is an AB-BA deadlock between two workers holding
    // each other's duplicates.
    let match_key = json!({ "reason": &reason, "subscriber_ids": payload["subscriber_ids"] });
    let absorbed: Vec<Option<Value>> = sqlx::query_scalar!(
        r#"DELETE FROM jobs
            WHERE (environment_id, id) IN (
                SELECT environment_id, id FROM jobs
                 WHERE environment_id = $1 AND job_type = 'hint'
                   AND id <> $2
                   AND (payload - 'notification_ids' - '_traceparent') = $3
                 FOR UPDATE SKIP LOCKED)
            RETURNING payload->'notification_ids' AS ids"#,
        env,
        job_id,
        match_key,
    )
    .fetch_all(&mut *tx)
    .await?;
    for ids in absorbed.into_iter().flatten() {
        if !ids.is_null() {
            let more: Vec<Uuid> = serde_json::from_value(ids)?;
            notification_ids.extend(more);
        }
    }
    notification_ids.sort_unstable();
    notification_ids.dedup();

    let targets: Vec<Option<Uuid>> = match subscriber_ids {
        None => vec![None],
        Some(subs) => subs.into_iter().map(Some).collect(),
    };
    let mut published: Vec<Uuid> = Vec::new();
    let mut deferred: Vec<Uuid> = Vec::new();
    let mut deferred_env_wide = false;
    for target in targets {
        let key = match target {
            Some(sub) => format!("{env}:{sub}"),
            None => format!("{env}:*"),
        };
        if pubsub.try_acquire_debounce(&key, cfg.hint_debounce).await? {
            let started = std::time::Instant::now();
            pubsub
                .publish(&Hint {
                    environment_id: env,
                    subscriber_id: target,
                    reason: reason.clone(),
                })
                .await?;
            metrics::histogram!("dronte_hint_publish_duration_seconds")
                .record(started.elapsed().as_secs_f64());
            // Enqueue-to-publish lag: the end-to-end hint latency a
            // subscriber experiences (queue wait + debounce included).
            let lag = (Utc::now() - job_created_at).num_milliseconds().max(0) as f64 / 1000.0;
            metrics::histogram!("dronte_hint_delivery_lag_seconds").record(lag);
            if let Some(sub) = target {
                published.push(sub);
            }
        } else {
            match target {
                Some(sub) => deferred.push(sub),
                None => deferred_env_wide = true,
            }
        }
    }

    timeline::append_delivered(tx, env, &notification_ids, &published).await?;

    if deferred.is_empty() && !deferred_env_wide {
        return Ok(Outcome::Done);
    }
    // Only the still-unpublished ids ride in the deferred payload; the
    // published ones got their timeline rows above, in this transaction.
    let remaining = if deferred_env_wide {
        Value::Null
    } else {
        let ids = timeline::ids_for_subscribers(tx, env, &notification_ids, &deferred).await?;
        if ids.is_empty() {
            Value::Null
        } else {
            json!(ids)
        }
    };
    let mut deferred_payload = json!({
        "reason": reason,
        "subscriber_ids": if deferred_env_wide { Value::Null } else { json!(deferred) },
        "notification_ids": remaining,
    });
    if let Some(traceparent) = payload.get("_traceparent") {
        deferred_payload["_traceparent"] = traceparent.clone();
    }
    Ok(Outcome::Defer {
        run_at: Utc::now() + cfg.hint_debounce,
        payload: Some(deferred_payload),
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
    jobs::enqueue_hint(tx, env, &subscribers, "notification", &all_ids).await?;
    Ok(Outcome::Done)
}

// =============================================================================
// timeline: watermark-window status appends (chunked, resumable)
// =============================================================================

/// Appends `status` rows for the visible notifications inside a watermark
/// move's `(from, to]` window, one keyset chunk per claim. occurred_at is
/// the move time (`to`) carried in the payload, so a replayed chunk writes
/// byte-identical rows.
async fn process_timeline(
    tx: &mut sqlx::PgConnection,
    env: Uuid,
    job_id: Uuid,
    payload: &Value,
    cursor: Option<&Value>,
) -> anyhow::Result<Outcome> {
    let subscriber: Uuid = serde_json::from_value(payload["subscriber_id"].clone())?;
    let status = match payload["status"].as_str() {
        Some(s @ (timeline::STATUS_READ | timeline::STATUS_SEEN)) => s,
        other => return Err(anyhow::anyhow!("invalid timeline status: {other:?}")),
    };
    let from: DateTime<Utc> = serde_json::from_value(payload["from"].clone())?;
    let to: DateTime<Utc> = serde_json::from_value(payload["to"].clone())?;

    // The counters-row lock serializes this job's NOT EXISTS guard with the
    // per-item read path, which appends under the same lock.
    sqlx::query!(
        r#"SELECT 1 AS one FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        env,
        subscriber,
    )
    .fetch_optional(&mut *tx)
    .await?;

    let cursor = cursor.and_then(|c| {
        Some((
            serde_json::from_value(c["after_ts"].clone()).ok()?,
            serde_json::from_value(c["after_id"].clone()).ok()?,
        ))
    });
    match timeline::append_window_chunk(tx, env, subscriber, status, from, to, to, cursor).await? {
        None => Ok(Outcome::Done),
        Some((after_ts, after_id)) => {
            sqlx::query!(
                r#"UPDATE jobs SET progress_cursor = $3 WHERE environment_id = $1 AND id = $2"#,
                env,
                job_id,
                json!({ "after_ts": after_ts, "after_id": after_id }),
            )
            .execute(&mut *tx)
            .await?;
            Ok(Outcome::Continue)
        }
    }
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
                                CASE WHEN jsonb_typeof(j.payload->'notification_ids') = 'array'
                                     THEN j.payload->'notification_ids' END)
                                WITH ORDINALITY AS t(nid, idx)
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
                                CASE WHEN jsonb_typeof(j.payload->'notification_ids') = 'array'
                                     THEN j.payload->'notification_ids' END)
                                WITH ORDINALITY AS t(nid, idx)
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
    jobs::enqueue_hint(tx, env, &[subscriber], "read_state", &[]).await?;
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

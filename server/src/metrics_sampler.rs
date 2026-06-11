//! Periodically sampled gauges. Everything here is recomputed from Postgres
//! on every sample, never carried from in-process state, so the numbers stay
//! true across restarts and a stalled subsystem cannot freeze its own alarm
//! (`dronte_partitions_remaining` keeps decaying even when the maintenance
//! job is dead, and that decay IS the W4 alert).
//!
//! Gauges emitted:
//! * `dronte_queue_depth{environment,job_type}`: all pending job rows
//! * `dronte_queue_due{environment,job_type}`: rows with run_at <= now()
//! * `dronte_dead_letters{job_type}`: parked jobs awaiting replay
//! * `dronte_partitions_remaining{table}`: pre-created future partitions
//! * `dronte_counter_drift_unread` / `dronte_counter_drift_unseen`: summed
//!   |recount - maintained| over a sample of recently-active subscribers

use std::collections::HashSet;
use std::sync::Mutex;

use sqlx::PgPool;

use crate::config::Config;
use crate::partitions;

pub async fn run(
    pool: PgPool,
    cfg: std::sync::Arc<Config>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(cfg.metrics_sample_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown.changed() => return,
        }
        if *shutdown.borrow() {
            return;
        }
        if let Err(err) = sample(&pool, &cfg).await {
            tracing::warn!(error = ?err, "metrics sample failed");
        }
    }
}

/// One sampling pass. Tests drive this directly.
pub async fn sample(pool: &PgPool, cfg: &Config) -> anyhow::Result<()> {
    sample_queue_depth(pool).await?;
    sample_dead_letters(pool).await?;
    sample_partitions(pool).await?;
    let (unread_drift, unseen_drift) = counter_drift(pool, cfg.counter_drift_sample_size).await?;
    metrics::gauge!("dronte_counter_drift_unread").set(unread_drift as f64);
    metrics::gauge!("dronte_counter_drift_unseen").set(unseen_drift as f64);
    Ok(())
}

/// Series seen on earlier samples, so a drained queue or emptied DLQ drops
/// its gauge to 0 instead of freezing at the last nonzero value.
static QUEUE_SERIES: Mutex<Option<HashSet<(String, String)>>> = Mutex::new(None);
static DLQ_SERIES: Mutex<Option<HashSet<String>>> = Mutex::new(None);

async fn sample_queue_depth(pool: &PgPool) -> anyhow::Result<()> {
    let rows = sqlx::query!(
        r#"SELECT e.slug, j.job_type,
                  count(*) AS "total!",
                  count(*) FILTER (WHERE j.run_at <= now()) AS "due!"
             FROM jobs j JOIN environments e ON e.id = j.environment_id
            GROUP BY 1, 2"#
    )
    .fetch_all(pool)
    .await?;

    let mut seen = HashSet::new();
    for row in rows {
        metrics::gauge!("dronte_queue_depth",
            "environment" => row.slug.clone(), "job_type" => row.job_type.clone())
        .set(row.total as f64);
        metrics::gauge!("dronte_queue_due",
            "environment" => row.slug.clone(), "job_type" => row.job_type.clone())
        .set(row.due as f64);
        seen.insert((row.slug, row.job_type));
    }
    let mut previous = QUEUE_SERIES.lock().expect("queue series lock");
    if let Some(previous) = previous.as_ref() {
        for (slug, job_type) in previous.difference(&seen) {
            metrics::gauge!("dronte_queue_depth",
                "environment" => slug.clone(), "job_type" => job_type.clone())
            .set(0.0);
            metrics::gauge!("dronte_queue_due",
                "environment" => slug.clone(), "job_type" => job_type.clone())
            .set(0.0);
        }
    }
    *previous = Some(seen);
    Ok(())
}

async fn sample_dead_letters(pool: &PgPool) -> anyhow::Result<()> {
    let rows =
        sqlx::query!(r#"SELECT job_type, count(*) AS "count!" FROM dead_letters GROUP BY 1"#)
            .fetch_all(pool)
            .await?;
    let mut seen = HashSet::new();
    for row in rows {
        metrics::gauge!("dronte_dead_letters", "job_type" => row.job_type.clone())
            .set(row.count as f64);
        seen.insert(row.job_type);
    }
    let mut previous = DLQ_SERIES.lock().expect("dlq series lock");
    if let Some(previous) = previous.as_ref() {
        for job_type in previous.difference(&seen) {
            metrics::gauge!("dronte_dead_letters", "job_type" => job_type.clone()).set(0.0);
        }
    }
    *previous = Some(seen);
    Ok(())
}

async fn sample_partitions(pool: &PgPool) -> anyhow::Result<()> {
    let mut conn = pool.acquire().await?;
    // DB clock: the decay must follow the database's idea of the month.
    let now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *conn)
        .await?;
    for table in partitions::PARTITIONED_TABLES {
        let remaining = partitions::remaining_at(&mut conn, table, now).await?;
        metrics::gauge!("dronte_partitions_remaining", "table" => *table).set(remaining as f64);
    }
    Ok(())
}

/// Summed |maintained - recounted| over the `sample_size` most recently
/// active subscribers. One statement = one snapshot: every counter mutation
/// commits atomically with its source rows, so ANY nonzero value is a bug,
/// even under full write load.
///
/// Subscribers with a disabled in_app preference are skipped: maintained
/// counters are mute-blind by design and only converge to the mute-aware
/// value when a rebuild runs, so no recount formula is exact for them.
/// Rows still owned by a pending deliver job are excluded from the recount
/// (the deliver bump is their single bookkeeper).
pub async fn counter_drift(pool: &PgPool, sample_size: i64) -> anyhow::Result<(i64, i64)> {
    let row = sqlx::query!(
        r#"WITH sampled AS (
               SELECT c.environment_id, c.subscriber_id,
                      c.unread_direct_count, c.unseen_direct_count,
                      c.read_watermark, c.seen_watermark
                 FROM subscriber_counters c
                WHERE NOT EXISTS (SELECT 1 FROM preferences p
                      WHERE p.environment_id = c.environment_id
                        AND p.subscriber_id  = c.subscriber_id
                        AND p.channel = 'in_app' AND p.enabled = false)
                ORDER BY c.updated_at DESC
                LIMIT $1)
           SELECT
               COALESCE(sum(abs(s.unread_direct_count - r.unread)), 0)::bigint
                   AS "unread_drift!",
               COALESCE(sum(abs(s.unseen_direct_count - r.unseen)), 0)::bigint
                   AS "unseen_drift!"
             FROM sampled s
            CROSS JOIN LATERAL (
                SELECT
                    (SELECT count(*) FROM notifications n
                      WHERE n.environment_id = s.environment_id
                        AND n.subscriber_id  = s.subscriber_id
                        AND n.visible_at <= now()
                        AND n.read_at IS NULL
                        AND n.visible_at > s.read_watermark
                        AND NOT EXISTS (SELECT 1 FROM jobs j
                              CROSS JOIN LATERAL jsonb_array_elements_text(
                                  j.payload->'notification_ids') WITH ORDINALITY AS t(nid, idx)
                              WHERE j.environment_id = s.environment_id
                                AND j.job_type = 'deliver'
                                AND t.nid = n.id::text
                                AND (t.idx - 1) >=
                                    COALESCE((j.progress_cursor->>'offset')::bigint, 0)))::int
                        AS unread,
                    (SELECT count(*) FROM notifications n
                      WHERE n.environment_id = s.environment_id
                        AND n.subscriber_id  = s.subscriber_id
                        AND n.visible_at <= now()
                        AND n.visible_at > s.seen_watermark
                        AND NOT EXISTS (SELECT 1 FROM jobs j
                              CROSS JOIN LATERAL jsonb_array_elements_text(
                                  j.payload->'notification_ids') WITH ORDINALITY AS t(nid, idx)
                              WHERE j.environment_id = s.environment_id
                                AND j.job_type = 'deliver'
                                AND t.nid = n.id::text
                                AND (t.idx - 1) >=
                                    COALESCE((j.progress_cursor->>'offset')::bigint, 0)))::int
                        AS unseen
            ) r"#,
        sample_size,
    )
    .fetch_one(pool)
    .await?;
    Ok((row.unread_drift, row.unseen_drift))
}

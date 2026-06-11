//! Dead-letter queue tooling: list parked jobs, replay them.
//!
//! Replay moves the row back into `jobs` with a fresh attempt budget and
//! `run_at = now()`, keeping the original id, payload, created_at and
//! progress_cursor. A chunked job therefore resumes from its cursor, and the
//! per-job effects stay keyed exactly as during a normal run, so a replayed
//! job applies its side effects exactly-once-observably.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::ids;

#[derive(Debug)]
pub struct DeadLetter {
    pub environment_slug: String,
    pub id: Uuid,
    pub job_type: String,
    pub attempts: i32,
    pub last_error: String,
    pub parked_at: DateTime<Utc>,
}

impl DeadLetter {
    pub fn typeid(&self) -> String {
        ids::typeid(ids::JOB, self.id)
    }
}

pub async fn list(pool: &PgPool) -> anyhow::Result<Vec<DeadLetter>> {
    Ok(sqlx::query_as!(
        DeadLetter,
        r#"SELECT e.slug AS environment_slug, d.id, d.job_type, d.attempts,
                  d.last_error, d.parked_at
             FROM dead_letters d
             JOIN environments e ON e.id = d.environment_id
            ORDER BY d.parked_at"#,
    )
    .fetch_all(pool)
    .await?)
}

/// Replay one parked job by id, optionally pinned to one environment
/// (environment_id is part of every key; an unscoped id match would reach
/// across environments). Returns false if no such dead letter.
pub async fn replay(pool: &PgPool, id: Uuid, environment: Option<Uuid>) -> anyhow::Result<bool> {
    let moved = replay_where(pool, Some(id), environment).await?;
    Ok(moved > 0)
}

/// Replay every parked job, optionally only one environment's. Returns the
/// number moved.
pub async fn replay_all(pool: &PgPool, environment: Option<Uuid>) -> anyhow::Result<u64> {
    replay_where(pool, None, environment).await
}

/// The DELETE and the INSERT are one statement, so a replay can neither lose
/// the job nor duplicate it. attempts resets to 0 (a replay grants a fresh
/// budget); last_error stays for forensics until the job next succeeds.
async fn replay_where(
    pool: &PgPool,
    id: Option<Uuid>,
    environment: Option<Uuid>,
) -> anyhow::Result<u64> {
    let moved = sqlx::query!(
        r#"WITH revived AS (
               DELETE FROM dead_letters
                WHERE ($1::uuid IS NULL OR id = $1)
                  AND ($2::uuid IS NULL OR environment_id = $2)
                RETURNING environment_id, id, job_type, payload, max_attempts,
                          last_error, progress_cursor, created_at)
           INSERT INTO jobs (environment_id, id, job_type, payload, run_at,
                             attempts, max_attempts, last_error,
                             progress_cursor, created_at)
           SELECT environment_id, id, job_type, payload, now(), 0,
                  max_attempts, last_error, progress_cursor, created_at
             FROM revived"#,
        id,
        environment,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(moved)
}

/// Resolve an environment slug for the CLI's `--env` flag.
pub async fn environment_by_slug(pool: &PgPool, slug: &str) -> anyhow::Result<Option<Uuid>> {
    Ok(
        sqlx::query_scalar!("SELECT id FROM environments WHERE slug = $1", slug)
            .fetch_optional(pool)
            .await?,
    )
}

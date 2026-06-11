//! The per-notification status timeline: append-only rows in
//! notification_status_log, one row per (notification, status) transition.
//!
//! Append discipline (the table has no global unique constraint because it
//! is partitioned): every append commits in the same transaction as its
//! idempotency key, and the NOT EXISTS guard runs while the subscriber's
//! counters row is locked, so two writers can never observe the same status
//! as missing. No code path ever UPDATEs a timeline row.
//!
//! Watermark moves (read-all, seen-all) stay O(1) on the request path: they
//! enqueue a chunked `timeline` job covering the `(old, new]` watermark
//! window, and the job appends rows with `occurred_at` = the move time
//! carried in its payload. Broadcasts are never materialized per subscriber
//! and have no timeline.

use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const STATUS_CREATED: &str = "created";
pub const STATUS_DELIVERED_HINT: &str = "delivered_hint";
pub const STATUS_SEEN: &str = "seen";
pub const STATUS_READ: &str = "read";

/// Rows appended per timeline-job transaction.
pub const TIMELINE_CHUNK: i64 = 500;

/// Append `status` for every id that does not already have it,
/// `occurred_at = now()` (transaction-stable DB clock).
pub async fn append(
    conn: &mut sqlx::PgConnection,
    env: Uuid,
    notification_ids: &[Uuid],
    status: &str,
) -> sqlx::Result<()> {
    if notification_ids.is_empty() {
        return Ok(());
    }
    sqlx::query!(
        r#"INSERT INTO notification_status_log
               (environment_id, notification_id, status, occurred_at)
           SELECT $1, t.nid, $3, now()
             FROM unnest($2::uuid[]) AS t(nid)
            WHERE NOT EXISTS (
                SELECT 1 FROM notification_status_log l
                 WHERE l.environment_id = $1 AND l.notification_id = t.nid
                   AND l.status = $3)
           ON CONFLICT DO NOTHING"#,
        env,
        notification_ids,
        status,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Append `delivered_hint` for the subset of `notification_ids` whose
/// recipient is in `subscriber_ids` (the targets a hint was just published
/// for).
pub async fn append_delivered(
    conn: &mut sqlx::PgConnection,
    env: Uuid,
    notification_ids: &[Uuid],
    subscriber_ids: &[Uuid],
) -> sqlx::Result<()> {
    if notification_ids.is_empty() || subscriber_ids.is_empty() {
        return Ok(());
    }
    sqlx::query!(
        r#"INSERT INTO notification_status_log
               (environment_id, notification_id, status, occurred_at)
           SELECT $1, n.id, $4, now()
             FROM notifications n
            WHERE n.environment_id = $1 AND n.id = ANY($2)
              AND n.subscriber_id = ANY($3)
              AND NOT EXISTS (
                  SELECT 1 FROM notification_status_log l
                   WHERE l.environment_id = $1 AND l.notification_id = n.id
                     AND l.status = $4)
           ON CONFLICT DO NOTHING"#,
        env,
        notification_ids,
        subscriber_ids,
        STATUS_DELIVERED_HINT,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// The ids in `notification_ids` whose recipient is in `subscriber_ids`.
/// Used to carry only the still-undelivered ids in a deferred hint payload.
pub async fn ids_for_subscribers(
    conn: &mut sqlx::PgConnection,
    env: Uuid,
    notification_ids: &[Uuid],
    subscriber_ids: &[Uuid],
) -> sqlx::Result<Vec<Uuid>> {
    if notification_ids.is_empty() || subscriber_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_scalar!(
        r#"SELECT n.id FROM notifications n
            WHERE n.environment_id = $1 AND n.id = ANY($2)
              AND n.subscriber_id = ANY($3)"#,
        env,
        notification_ids,
        subscriber_ids,
    )
    .fetch_all(conn)
    .await
}

/// One chunk of a watermark-window timeline job: append `status` with
/// `occurred_at = at` (the watermark move time) for visible notifications in
/// `(from, to]`, keyset-resumable via `cursor`. Returns the new cursor when
/// more rows may remain, None when the window is exhausted.
///
/// The caller holds the subscriber's counters-row lock and commits the
/// cursor advance in the same transaction, so chunk replay after a crash is
/// idempotent by construction.
#[allow(clippy::too_many_arguments)]
pub async fn append_window_chunk(
    conn: &mut sqlx::PgConnection,
    env: Uuid,
    subscriber: Uuid,
    status: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    at: DateTime<Utc>,
    cursor: Option<(DateTime<Utc>, Uuid)>,
) -> sqlx::Result<Option<(DateTime<Utc>, Uuid)>> {
    // The window's own lower bound doubles as the initial keyset cursor:
    // rows at visible_at == from are excluded by the range predicate, so
    // (from, nil) is below every candidate row. chrono's MIN_UTC would
    // overflow Postgres' timestamptz range.
    let (cursor_ts, cursor_id) = cursor.unwrap_or((from, Uuid::nil()));
    let rows = sqlx::query!(
        r#"SELECT n.id, n.visible_at FROM notifications n
            WHERE n.environment_id = $1 AND n.subscriber_id = $2
              AND n.visible_at > $3 AND n.visible_at <= $4
              AND (n.visible_at, n.id) > ($5, $6)
            ORDER BY n.visible_at, n.id
            LIMIT $7"#,
        env,
        subscriber,
        from,
        to,
        cursor_ts,
        cursor_id,
        TIMELINE_CHUNK,
    )
    .fetch_all(&mut *conn)
    .await?;
    let Some(last) = rows.last() else {
        return Ok(None);
    };
    let next_cursor = (last.visible_at, last.id);

    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    sqlx::query!(
        r#"INSERT INTO notification_status_log
               (environment_id, notification_id, status, occurred_at)
           SELECT $1, t.nid, $3, $4
             FROM unnest($2::uuid[]) AS t(nid)
            WHERE NOT EXISTS (
                SELECT 1 FROM notification_status_log l
                 WHERE l.environment_id = $1 AND l.notification_id = t.nid
                   AND l.status = $3)
           ON CONFLICT DO NOTHING"#,
        env,
        &ids,
        status,
        at,
    )
    .execute(&mut *conn)
    .await?;

    if (rows.len() as i64) < TIMELINE_CHUNK {
        return Ok(None);
    }
    Ok(Some(next_cursor))
}

//! Outbox/job enqueue helpers. Always called with the transaction that owns
//! the triggering write — the transactional-outbox invariant (no
//! Postgres↔Redis dual writes anywhere) lives here by construction.

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::{ids, telemetry};

pub const TYPE_HINT: &str = "hint";
pub const TYPE_DELIVER: &str = "deliver";
pub const TYPE_COUNTER_REBUILD: &str = "counter_rebuild";
pub const TYPE_TIMELINE: &str = "timeline";

/// `run_at = None` ⇒ now().
pub async fn enqueue(
    tx: &mut sqlx::PgConnection,
    environment_id: Uuid,
    job_type: &str,
    mut payload: serde_json::Value,
    run_at: Option<DateTime<Utc>>,
) -> sqlx::Result<Uuid> {
    // The enqueuing trace rides along so the worker's span can join it
    // (ingest -> outbox -> worker -> hint as ONE trace).
    if let Some(traceparent) = telemetry::current_traceparent()
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("_traceparent".to_owned(), json!(traceparent));
    }
    let id = ids::new_uuid();
    sqlx::query!(
        r#"INSERT INTO jobs (environment_id, id, job_type, payload, run_at)
           VALUES ($1, $2, $3, $4, COALESCE($5, now()))"#,
        environment_id,
        id,
        job_type,
        payload,
        run_at,
    )
    .execute(tx)
    .await?;
    Ok(id)
}

/// Debounced pub/sub hint. `subscriber_ids` empty ⇒ environment-wide (a
/// broadcast: one job and one message regardless of subscriber count).
/// `notification_ids` are the direct notifications this hint announces; the
/// hint worker appends their `delivered_hint` timeline rows when it
/// publishes. Empty for read-state and broadcast hints.
pub async fn enqueue_hint(
    tx: &mut sqlx::PgConnection,
    environment_id: Uuid,
    subscriber_ids: &[Uuid],
    reason: &str,
    notification_ids: &[Uuid],
) -> sqlx::Result<Uuid> {
    let subscribers = if subscriber_ids.is_empty() {
        serde_json::Value::Null
    } else {
        json!(subscriber_ids)
    };
    let notifications = if notification_ids.is_empty() {
        serde_json::Value::Null
    } else {
        json!(notification_ids)
    };
    enqueue(
        tx,
        environment_id,
        TYPE_HINT,
        json!({
            "reason": reason,
            "subscriber_ids": subscribers,
            "notification_ids": notifications,
        }),
        None,
    )
    .await
}

/// Chunked watermark-window timeline job (see `timeline`): appends `status`
/// rows for visible notifications in `(from, to]` with `occurred_at = to`
/// (the watermark move time).
pub async fn enqueue_timeline(
    tx: &mut sqlx::PgConnection,
    environment_id: Uuid,
    subscriber_id: Uuid,
    status: &str,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
) -> sqlx::Result<Uuid> {
    enqueue(
        tx,
        environment_id,
        TYPE_TIMELINE,
        json!({
            "subscriber_id": subscriber_id,
            "status": status,
            "from": from,
            "to": to,
        }),
        None,
    )
    .await
}

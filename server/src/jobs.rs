//! Outbox/job enqueue helpers. Always called with the transaction that owns
//! the triggering write — the transactional-outbox invariant (no
//! Postgres↔Redis dual writes anywhere) lives here by construction.

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::ids;

pub const TYPE_HINT: &str = "hint";
pub const TYPE_DELIVER: &str = "deliver";
pub const TYPE_COUNTER_REBUILD: &str = "counter_rebuild";

/// `run_at = None` ⇒ now().
pub async fn enqueue(
    tx: &mut sqlx::PgConnection,
    environment_id: Uuid,
    job_type: &str,
    payload: serde_json::Value,
    run_at: Option<DateTime<Utc>>,
) -> sqlx::Result<Uuid> {
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
pub async fn enqueue_hint(
    tx: &mut sqlx::PgConnection,
    environment_id: Uuid,
    subscriber_ids: &[Uuid],
    reason: &str,
) -> sqlx::Result<Uuid> {
    let subscribers = if subscriber_ids.is_empty() {
        serde_json::Value::Null
    } else {
        json!(subscriber_ids)
    };
    enqueue(
        tx,
        environment_id,
        TYPE_HINT,
        json!({ "reason": reason, "subscriber_ids": subscribers }),
        None,
    )
    .await
}

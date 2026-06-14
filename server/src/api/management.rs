//! Management plane: create notifications/broadcasts, upsert subscribers.
//!
//! Both creates follow the transactional-outbox shape from specs/schema.sql:
//! resource rows + counter bumps + idempotency snapshot + outbox job commit
//! in ONE transaction. Idempotent replay returns the stored snapshot
//! byte-identically with HTTP 200 (first acceptance is 201) — both paths
//! serialize a `serde_json::Value` decoded from Postgres jsonb, so the bytes
//! cannot drift between the original response and a replay.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Months, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::ManagementAuth;
use crate::error::ApiError;
use crate::extract::ApiJson;
use crate::state::AppState;
use crate::{api, ids, jobs, ratelimit, timeline};

pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
/// `deliver_at` cap. Partition pre-creation headroom covers it.
pub const MAX_DELIVER_AT_MONTHS: u32 = 13;

#[derive(Debug, Deserialize)]
pub struct CreateNotificationsRequest {
    /// Single-recipient sugar for `subscriber_ids: [x]`.
    pub subscriber_id: Option<String>,
    pub subscriber_ids: Option<Vec<String>>,
    pub category: String,
    pub payload: Option<Value>,
    /// Client-supplied; server-generated and echoed if omitted.
    pub idempotency_key: Option<String>,
    /// Scheduled delivery; must be in the future, at most 13 months out.
    pub deliver_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    post,
    path = "/v1/notifications",
    tag = "management",
    operation_id = "createNotifications",
    summary = "Create direct notifications (1–100 recipients, fan-out on write)",
    description = r#"Creates one notification per recipient in a single transaction together
with counter bumps and the outbox job. The `idempotency_key` covers the
**whole request**: a retry never partially re-runs the batch.

`deliver_at` schedules delivery (max 13 months out): the notification
is durable immediately but invisible to the subscriber until then;
counters and real-time hints fire at `deliver_at`.

Recipients that don't exist yet are lazily created as subscribers.
Need more than 100 recipients? That's a broadcast.
"#,
    request_body = crate::api::contract::CreateNotificationsRequest,
    responses(
        (status = 201, description = "Created.", body = crate::api::contract::CreateNotificationsResponse),
        (status = 200, description = "Idempotent replay — original response echoed, nothing re-created.", body = crate::api::contract::CreateNotificationsResponse),
        (status = 400, description = "Validation error.", body = crate::api::contract::Error),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
        (status = 429, response = crate::api::contract::RateLimited),
    ),
    security(("ApiKeyBearer" = []))
)]
pub async fn create_notifications(
    State(state): State<AppState>,
    auth: ManagementAuth,
    ApiJson(req): ApiJson<CreateNotificationsRequest>,
) -> Result<Response, ApiError> {
    enforce_api_key_limit(&state, &auth).await?;
    let env = auth.environment_id;
    let recipients = validate_recipients(&req)?;
    let category = validate_category(&req.category)?;
    let payload = validate_payload(req.payload)?;
    let key = validate_idempotency_key(req.idempotency_key)?;

    // The snapshot lookup runs BEFORE the wall-clock validation below. A
    // replay must return the original response even if its deliver_at has
    // passed in the meantime.
    if let Some(snapshot) = fetch_snapshot(&state.pool, env, "notification", &key).await? {
        return Ok((StatusCode::OK, Json(snapshot)).into_response());
    }

    if let Some(deliver_at) = req.deliver_at {
        // DB clock, not app clock: partition headroom is computed from the
        // database's now(), and skew across a month boundary would let a
        // contract-legal deliver_at land outside the pre-created partitions.
        let now: DateTime<Utc> = sqlx::query_scalar!(r#"SELECT now() AS "now!""#)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::from)?;
        if deliver_at <= now {
            return Err(ApiError::bad_request("deliver_at must be in the future"));
        }
        if deliver_at > now + Months::new(MAX_DELIVER_AT_MONTHS) {
            return Err(ApiError::bad_request(
                "deliver_at must be at most 13 months out",
            ));
        }
    }

    match create_notifications_txn(
        &state,
        env,
        &recipients,
        category,
        &payload,
        req.deliver_at,
        &key,
    )
    .await
    {
        Ok(snapshot) => Ok((StatusCode::CREATED, Json(snapshot)).into_response()),
        // Concurrent retry beat us to the idempotency insert: acknowledged-and-dropped.
        Err(err) if is_idempotency_conflict(&err) => {
            let snapshot = fetch_snapshot(&state.pool, env, "notification", &key)
                .await?
                .ok_or_else(|| ApiError::from(anyhow::anyhow!("idempotency snapshot vanished")))?;
            Ok((StatusCode::OK, Json(snapshot)).into_response())
        }
        Err(err) => Err(err.into()),
    }
}

async fn create_notifications_txn(
    state: &AppState,
    env: Uuid,
    recipients: &[String],
    category: &str,
    payload: &Value,
    deliver_at: Option<DateTime<Utc>>,
    key: &str,
) -> sqlx::Result<Value> {
    let mut tx = state.pool.begin().await?;

    // Lazy subscriber upsert (+ counters rows for the conditional bump below).
    let fresh_ids: Vec<Uuid> = recipients.iter().map(|_| ids::new_uuid()).collect();
    let externals: Vec<String> = recipients.to_vec();
    sqlx::query!(
        r#"INSERT INTO subscribers (environment_id, id, subscriber_id)
           SELECT $1, t.id, t.ext FROM unnest($2::uuid[], $3::text[]) AS t(id, ext)
           ON CONFLICT (environment_id, subscriber_id) DO NOTHING"#,
        env,
        &fresh_ids,
        &externals,
    )
    .execute(&mut *tx)
    .await?;
    let rows = sqlx::query!(
        r#"SELECT id, subscriber_id FROM subscribers
            WHERE environment_id = $1 AND subscriber_id = ANY($2)"#,
        env,
        &externals,
    )
    .fetch_all(&mut *tx)
    .await?;
    let by_external: std::collections::HashMap<String, Uuid> =
        rows.into_iter().map(|r| (r.subscriber_id, r.id)).collect();
    let internal: Vec<Uuid> = recipients.iter().map(|r| by_external[r]).collect();
    sqlx::query!(
        r#"INSERT INTO subscriber_counters (environment_id, subscriber_id)
           SELECT $1, unnest($2::uuid[])
           ON CONFLICT (environment_id, subscriber_id) DO NOTHING"#,
        env,
        &internal,
    )
    .execute(&mut *tx)
    .await?;

    // Ordering timestamps are DB-clock-sourced: created_at/visible_at are
    // now() INSIDE the insert (now() is transaction-stable), never app time.
    let notif_ids: Vec<Uuid> = recipients.iter().map(|_| ids::new_uuid()).collect();
    sqlx::query!(
        r#"INSERT INTO notifications
               (environment_id, id, subscriber_id, category, payload,
                created_at, deliver_at, visible_at)
           SELECT $1, t.id, t.sub, $4, $5, now(), $6, COALESCE($6, now())
             FROM unnest($2::uuid[], $3::uuid[]) AS t(id, sub)"#,
        env,
        &notif_ids,
        &internal,
        category,
        payload,
        deliver_at,
    )
    .execute(&mut *tx)
    .await?;
    // 'created' is appended for scheduled notifications too: created means
    // accepted-and-durable, not visible.
    timeline::append(&mut tx, env, &notif_ids, timeline::STATUS_CREATED).await?;

    if deliver_at.is_none() {
        // Conditional increment — the guard against the mark-all-read race:
        // both paths write the counters row (row lock serializes them) and
        // the condition makes the serialization order irrelevant. now() =
        // this txn's visible_at. One row per subscriber (recipients dedup'd).
        // The mute guard keeps the counter in step with the list arm so an
        // item created into an already-muted category is not counted (the
        // list hides it). A later preference flip still recounts via
        // counter_rebuild, so muting an existing category stays exact too.
        sqlx::query!(
            r#"UPDATE subscriber_counters c SET
                   unread_direct_count = c.unread_direct_count + (now() > c.read_watermark
                       AND NOT EXISTS (SELECT 1 FROM preferences p
                             WHERE p.environment_id = c.environment_id
                               AND p.subscriber_id  = c.subscriber_id
                               AND p.category = $3 AND p.channel = 'in_app'
                               AND p.enabled = false))::int,
                   unseen_direct_count = c.unseen_direct_count + (now() > c.seen_watermark
                       AND NOT EXISTS (SELECT 1 FROM preferences p
                             WHERE p.environment_id = c.environment_id
                               AND p.subscriber_id  = c.subscriber_id
                               AND p.category = $3 AND p.channel = 'in_app'
                               AND p.enabled = false))::int,
                   updated_at = now()
             WHERE c.environment_id = $1 AND c.subscriber_id = ANY($2)"#,
            env,
            &internal,
            category,
        )
        .execute(&mut *tx)
        .await?;
        jobs::enqueue_hint(&mut tx, env, &internal, "notification", &notif_ids).await?;
    } else {
        // Scheduled: counters NOT bumped at create — the deliver job bumps
        // them in the same txn that deletes the job row (exactly-once key).
        jobs::enqueue(
            &mut tx,
            env,
            jobs::TYPE_DELIVER,
            json!({ "notification_ids": notif_ids }),
            deliver_at,
        )
        .await?;
    }

    let snapshot = json!({
        "idempotency_key": key,
        "notifications": recipients
            .iter()
            .zip(&notif_ids)
            .map(|(ext, id)| json!({
                "id": ids::typeid(ids::NOTIFICATION, *id),
                "subscriber_id": ext,
            }))
            .collect::<Vec<_>>(),
    });
    // RETURNING the jsonb (not echoing our local value) pins both the 201 and
    // every future replay to the same Postgres-normalized document.
    let snapshot = insert_snapshot(&mut tx, env, "notification", key, &snapshot).await?;
    tx.commit().await?;
    Ok(snapshot)
}

#[derive(Debug, Deserialize)]
pub struct CreateBroadcastRequest {
    pub category: String,
    pub payload: Option<Value>,
    pub idempotency_key: Option<String>,
}

#[utoipa::path(
    post,
    path = "/v1/broadcasts",
    tag = "management",
    operation_id = "createBroadcast",
    summary = "Create a broadcast (one row, fan-out on read)",
    description = r#"One row per announcement targeting the whole environment, regardless of
subscriber count. Visible to each subscriber only if the broadcast is
created at or after that subscriber's `created_at`.
"#,
    request_body = crate::api::contract::CreateBroadcastRequest,
    responses(
        (status = 201, description = "Created.", body = crate::api::contract::Broadcast),
        (status = 200, description = "Idempotent replay.", body = crate::api::contract::Broadcast),
        (status = 400, description = "Validation error.", body = crate::api::contract::Error),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
        (status = 429, response = crate::api::contract::RateLimited),
    ),
    security(("ApiKeyBearer" = []))
)]
pub async fn create_broadcast(
    State(state): State<AppState>,
    auth: ManagementAuth,
    ApiJson(req): ApiJson<CreateBroadcastRequest>,
) -> Result<Response, ApiError> {
    enforce_api_key_limit(&state, &auth).await?;
    let env = auth.environment_id;
    let category = validate_category(&req.category)?.to_owned();
    let payload = validate_payload(req.payload)?;
    let key = validate_idempotency_key(req.idempotency_key)?;
    let (status, snapshot) = create_broadcast_idempotent(&state, env, category, payload, key).await?;
    Ok((status, Json(snapshot)).into_response())
}

/// The single broadcast write-path, shared by the management plane and the
/// admin composer (specs/phase-4-admin.md: "composing is creating, same
/// idempotent management-plane semantics"). One row per announcement,
/// fan-out on read, NEVER materialized per subscriber, plus one env-wide hint
/// regardless of subscriber count. Returns 201 on first acceptance, 200 on
/// idempotent replay (byte-identical snapshot).
pub(crate) async fn create_broadcast_idempotent(
    state: &AppState,
    env: Uuid,
    category: String,
    payload: Value,
    key: String,
) -> Result<(StatusCode, Value), ApiError> {
    if let Some(snapshot) = fetch_snapshot(&state.pool, env, "broadcast", &key).await? {
        return Ok((StatusCode::OK, snapshot));
    }

    let result: sqlx::Result<Value> = async {
        let mut tx = state.pool.begin().await?;
        let id = ids::new_uuid();
        let row = sqlx::query!(
            r#"INSERT INTO broadcasts (environment_id, id, category, payload)
               VALUES ($1, $2, $3, $4) RETURNING created_at"#,
            env,
            id,
            &category,
            &payload,
        )
        .fetch_one(&mut *tx)
        .await?;
        jobs::enqueue_hint(&mut tx, env, &[], "broadcast", &[]).await?;
        let snapshot = json!({
            "id": ids::typeid(ids::BROADCAST, id),
            "category": category,
            "payload": payload,
            "created_at": api::format_ts(row.created_at),
            "idempotency_key": key,
        });
        let snapshot = insert_snapshot(&mut tx, env, "broadcast", &key, &snapshot).await?;
        tx.commit().await?;
        Ok(snapshot)
    }
    .await;

    match result {
        Ok(snapshot) => Ok((StatusCode::CREATED, snapshot)),
        Err(err) if is_idempotency_conflict(&err) => {
            let snapshot = fetch_snapshot(&state.pool, env, "broadcast", &key)
                .await?
                .ok_or_else(|| ApiError::from(anyhow::anyhow!("idempotency snapshot vanished")))?;
            Ok((StatusCode::OK, snapshot))
        }
        Err(err) => Err(err.into()),
    }
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct UpsertSubscriberRequest {
    // value_type String (not Option): the field is optional via `required`,
    // not nullable — matches the 3.0 contract shape.
    #[schema(value_type = String, format = DateTime, required = false)]
    /// Backdate on create only; ignored if the subscriber exists.
    pub created_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    put,
    path = "/v1/subscribers/{subscriber_id}",
    tag = "management",
    operation_id = "upsertSubscriber",
    summary = "Upsert a subscriber",
    description = r#"Subscribers are normally created lazily; this exists for imports.
`created_at` may be **backdated** on first create (ignored on update) —
it is the knob controlling which historical broadcasts an imported user
sees (`broadcast.created_at >= subscriber.created_at`).
"#,
    request_body = inline(UpsertSubscriberRequest),
    params(("subscriber_id" = String, Path, max_length = 255, description = "Customer-provided subscriber id (e.g. `usr_42`).")),
    responses(
        (status = 200, description = "Upserted.", body = crate::api::contract::Subscriber),
        // The handler rejects an out-of-range subscriber_id (empty or > 255
        // chars) with 400; declaring it keeps the annotation honest about the
        // status the handler returns and gives @dronte/client a 400 branch.
        (status = 400, description = "Validation error.", body = crate::api::contract::Error),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
    ),
    security(("ApiKeyBearer" = []))
)]
pub async fn upsert_subscriber(
    State(state): State<AppState>,
    auth: ManagementAuth,
    Path(subscriber_id): Path<String>,
    body: Option<ApiJson<UpsertSubscriberRequest>>,
) -> Result<Response, ApiError> {
    let env = auth.environment_id;
    if subscriber_id.is_empty() || subscriber_id.len() > 255 {
        return Err(ApiError::bad_request(
            "subscriber_id must be 1–255 characters",
        ));
    }
    let backdate = body.and_then(|ApiJson(b)| b.created_at);

    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    // created_at backdate applies on first create only (DO NOTHING on
    // conflict) — it is the knob controlling which historical broadcasts an
    // imported user sees.
    sqlx::query!(
        r#"INSERT INTO subscribers (environment_id, id, subscriber_id, created_at)
           VALUES ($1, $2, $3, COALESCE($4, now()))
           ON CONFLICT (environment_id, subscriber_id) DO NOTHING"#,
        env,
        ids::new_uuid(),
        &subscriber_id,
        backdate,
    )
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    let row = sqlx::query!(
        r#"SELECT id, subscriber_id, created_at FROM subscribers
            WHERE environment_id = $1 AND subscriber_id = $2"#,
        env,
        &subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    sqlx::query!(
        r#"INSERT INTO subscriber_counters (environment_id, subscriber_id)
           VALUES ($1, $2) ON CONFLICT (environment_id, subscriber_id) DO NOTHING"#,
        env,
        row.id,
    )
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;

    Ok(Json(json!({
        "subscriber_id": row.subscriber_id,
        "created_at": api::format_ts(row.created_at),
    }))
    .into_response())
}

#[utoipa::path(
    get,
    path = "/v1/notifications/{id}/timeline",
    tag = "management",
    operation_id = "getNotificationTimeline",
    summary = "Status timeline for one notification",
    description = r#"The append-only delivery timeline — the "did it send?" answer.
Statuses appear as they happen: `created` (accepted and durable),
`delivered_hint` (a real-time hint announcing it was published),
`seen` and `read` (subscriber actions; watermark moves apply them
asynchronously, so a just-clicked "mark all read" may take a moment
to appear here). Entries are ordered by `occurred_at`. Unknown future
statuses must be ignored by clients.

Broadcasts have no per-recipient timeline (they are never materialized
per subscriber).
"#,
    params(("id" = crate::api::contract::NotificationId, Path)),
    responses(
        (status = 200, description = "The timeline so far.", body = crate::api::contract::NotificationTimeline),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
        (status = 404, description = "Resource not found in this environment.", body = crate::api::contract::Error),
        (status = 429, response = crate::api::contract::RateLimited),
    ),
    security(("ApiKeyBearer" = []))
)]
pub async fn get_notification_timeline(
    State(state): State<AppState>,
    auth: ManagementAuth,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    enforce_api_key_limit(&state, &auth).await?;
    let env = auth.environment_id;
    let id = ids::parse_typeid(ids::NOTIFICATION, &id)
        .ok_or_else(|| ApiError::not_found("no such notification"))?;

    let subscriber = sqlx::query_scalar!(
        r#"SELECT s.subscriber_id FROM notifications n
             JOIN subscribers s ON s.environment_id = n.environment_id
                               AND s.id = n.subscriber_id
            WHERE n.environment_id = $1 AND n.id = $2"#,
        env,
        id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no such notification"))?;

    // min() per status is a defensive read-time dedupe; writers already
    // guarantee at most one row per (notification, status).
    let rows = sqlx::query!(
        r#"SELECT status, min(occurred_at) AS "occurred_at!"
             FROM notification_status_log
            WHERE environment_id = $1 AND notification_id = $2
            GROUP BY status
            ORDER BY 2, 1"#,
        env,
        id,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(json!({
        "id": ids::typeid(ids::NOTIFICATION, id),
        "subscriber_id": subscriber,
        "timeline": rows
            .into_iter()
            .map(|r| json!({ "status": r.status, "occurred_at": api::format_ts(r.occurred_at) }))
            .collect::<Vec<_>>(),
    }))
    .into_response())
}

// =============================================================================
// Shared validation + idempotency plumbing
// =============================================================================

/// Management-plane token bucket, one bucket per API key shared across every
/// replica (the Redis Lua bucket is the cross-replica source of truth).
async fn enforce_api_key_limit(state: &AppState, auth: &ManagementAuth) -> Result<(), ApiError> {
    ratelimit::enforce(
        state.ratelimit.as_ref(),
        &format!("key:{}", auth.api_key_id),
        state.cfg.api_key_rate_per_sec,
        state.cfg.api_key_rate_burst,
    )
    .await
}

fn validate_recipients(req: &CreateNotificationsRequest) -> Result<Vec<String>, ApiError> {
    let list = match (&req.subscriber_id, &req.subscriber_ids) {
        (Some(one), None) => vec![one.clone()],
        (None, Some(many)) => many.clone(),
        _ => {
            return Err(ApiError::bad_request(
                "exactly one of subscriber_id / subscriber_ids is required",
            ));
        }
    };
    if list.is_empty() || list.len() > 100 {
        return Err(ApiError::bad_request(
            "subscriber_ids must contain 1–100 recipients (use a broadcast for more)",
        ));
    }
    if list.iter().any(|s| s.is_empty() || s.len() > 255) {
        return Err(ApiError::bad_request(
            "subscriber ids must be 1–255 characters",
        ));
    }
    // One notification row per recipient; duplicates collapse.
    let mut seen = std::collections::HashSet::new();
    Ok(list
        .into_iter()
        .filter(|s| seen.insert(s.clone()))
        .collect())
}

pub(crate) fn validate_category(category: &str) -> Result<&str, ApiError> {
    if category.is_empty() || category.len() > 255 {
        return Err(ApiError::bad_request("category must be 1–255 characters"));
    }
    Ok(category)
}

pub(crate) fn validate_payload(payload: Option<Value>) -> Result<Value, ApiError> {
    let payload = payload.unwrap_or_else(|| json!({}));
    if !payload.is_object() {
        return Err(ApiError::bad_request("payload must be a JSON object"));
    }
    let serialized = serde_json::to_string(&payload).map_err(ApiError::from)?;
    if serialized.len() > MAX_PAYLOAD_BYTES {
        return Err(ApiError::bad_request("payload exceeds 16 KiB serialized"));
    }
    Ok(payload)
}

pub(crate) fn validate_idempotency_key(key: Option<String>) -> Result<String, ApiError> {
    match key {
        Some(key) if key.is_empty() || key.len() > 255 => Err(ApiError::bad_request(
            "idempotency_key must be 1–255 characters",
        )),
        Some(key) => Ok(key),
        // Server-generated and echoed.
        None => Ok(format!("idem_{}", ids::new_uuid().as_simple())),
    }
}

async fn fetch_snapshot(
    pool: &sqlx::PgPool,
    env: Uuid,
    scope: &str,
    key: &str,
) -> Result<Option<Value>, ApiError> {
    sqlx::query_scalar!(
        r#"SELECT response_snapshot FROM idempotency_keys
            WHERE environment_id = $1 AND scope = $2 AND idempotency_key = $3"#,
        env,
        scope,
        key,
    )
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)
}

async fn insert_snapshot(
    tx: &mut sqlx::PgConnection,
    env: Uuid,
    scope: &str,
    key: &str,
    snapshot: &Value,
) -> sqlx::Result<Value> {
    sqlx::query_scalar!(
        r#"INSERT INTO idempotency_keys
               (environment_id, scope, idempotency_key, response_snapshot)
           VALUES ($1, $2, $3, $4)
           RETURNING response_snapshot"#,
        env,
        scope,
        key,
        snapshot,
    )
    .fetch_one(tx)
    .await
}

fn is_idempotency_conflict(err: &sqlx::Error) -> bool {
    matches!(
        err,
        sqlx::Error::Database(db)
            if db.code().as_deref() == Some("23505")
                && db.constraint() == Some("idempotency_keys_pkey")
    )
}

//! Management plane: create notifications/broadcasts, upsert subscribers.
//!
//! Both creates use the transactional outbox: resource rows, counter bumps,
//! idempotency snapshot, and outbox job commit in ONE transaction. Idempotent
//! replay returns the stored snapshot byte-identically with HTTP 200 (first
//! acceptance is 201). Both paths serialize a `serde_json::Value` decoded from
//! Postgres jsonb, so the bytes cannot drift between the original response and
//! a replay.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Months, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::ManagementAuth;
use crate::error::ApiError;
use crate::extract::{ApiJson, ApiQuery};
use crate::state::AppState;
use crate::{api, ids, jobs, ratelimit, timeline, worker};

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
    /// Client-supplied. Server-generated and echoed if omitted.
    pub idempotency_key: Option<String>,
    /// Scheduled delivery. Must be in the future, at most 13 months out.
    pub deliver_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    post,
    path = "/v1/notifications",
    tag = "management",
    operation_id = "createNotifications",
    summary = "Create notifications",
    description = r#"Send a notification to up to 100 subscribers; one item is created per recipient. Use idempotency_key to make retries safe, and deliver_at to schedule delivery up to 13 months ahead (the item stays hidden from the subscriber until then). Unknown subscribers are created automatically. For more than 100 recipients, use a broadcast."#,
    request_body = crate::api::contract::CreateNotificationsRequest,
    responses(
        (status = 201, description = "Created.", body = crate::api::contract::CreateNotificationsResponse),
        (status = 200, description = "Idempotent replay — original response echoed, nothing re-created.", body = crate::api::contract::CreateNotificationsResponse),
        (
            status = 400,
            description = "Validation error.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "deliver_at must be in the future"}}),
        ),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid API key"}}),
        ),
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
        // Conditional increment guards against the mark-all-read race. Both
        // paths write the counters row (the row lock serializes them) and the
        // condition makes the serialization order irrelevant. now() is this
        // txn's visible_at. One row per subscriber (recipients dedup'd). The
        // mute guard keeps the counter in step with the list arm so an item
        // created into an already-muted category is not counted (the list
        // hides it). A later preference flip recounts via counter_rebuild, so
        // muting an existing category stays exact too.
        sqlx::query!(
            r#"UPDATE subscriber_counters c SET
                   unread_direct_count = c.unread_direct_count + (now() > c.read_watermark
                       AND now() > c.archive_watermark
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
        // Scheduled: counters NOT bumped at create. The deliver job bumps them
        // in the same txn that deletes the job row (exactly-once key).
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
    summary = "Create a broadcast",
    description = r#"Send one announcement to every subscriber in the environment. A subscriber sees it only if it was created at or after they were."#,
    request_body = crate::api::contract::CreateBroadcastRequest,
    responses(
        (status = 201, description = "Created.", body = crate::api::contract::Broadcast),
        (status = 200, description = "Idempotent replay.", body = crate::api::contract::Broadcast),
        (
            status = 400,
            description = "Validation error.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "category must be 1–255 characters"}}),
        ),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid API key"}}),
        ),
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
    let (status, snapshot) =
        create_broadcast_idempotent(&state, env, category, payload, key).await?;
    Ok((status, Json(snapshot)).into_response())
}

/// The single broadcast write-path, shared by the management plane and the
/// admin composer. Composing is creating, same idempotent management-plane
/// semantics. One row per announcement, fan-out on read, NEVER materialized
/// per subscriber, plus one env-wide hint regardless of subscriber count.
/// Returns 201 on first acceptance, 200 on idempotent replay (byte-identical
/// snapshot).
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
    // value_type String (not Option) so the field is optional via `required`,
    // not nullable. Matches the OpenAPI 3.0 shape.
    #[schema(value_type = String, format = DateTime, required = false)]
    /// Backdate on create only. Ignored if the subscriber exists.
    pub created_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    put,
    path = "/v1/subscribers/{subscriber_id}",
    tag = "management",
    operation_id = "upsertSubscriber",
    summary = "Upsert a subscriber",
    description = r#"Create or update a subscriber. Subscribers are normally created automatically; use this for imports. On first create, created_at may be backdated to control which past broadcasts the subscriber sees."#,
    request_body = inline(UpsertSubscriberRequest),
    params(("subscriber_id" = String, Path, max_length = 255, description = "Customer-provided subscriber id (e.g. `usr_42`).")),
    responses(
        (status = 200, description = "Upserted.", body = crate::api::contract::Subscriber),
        // 400 declared because the handler rejects an out-of-range
        // subscriber_id (empty or over 255 chars). Annotated status must match
        // returned status.
        (
            status = 400,
            description = "Validation error.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "subscriber_id must be 1–255 characters"}}),
        ),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid API key"}}),
        ),
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
    // conflict). It controls which historical broadcasts an imported user
    // sees (broadcast.created_at >= subscriber.created_at).
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
    summary = "Notification timeline",
    description = r#"The history of one notification: when it was created, delivered, seen, and read. Entries are ordered by time. Broadcasts have no per-subscriber timeline."#,
    params(("id" = crate::api::contract::NotificationId, Path)),
    responses(
        (status = 200, description = "The timeline so far.", body = crate::api::contract::NotificationTimeline),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid API key"}}),
        ),
        (
            status = 404,
            description = "Resource not found in this environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such notification"}}),
        ),
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

    // min() per status is a defensive read-time dedupe. Writers already
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
// Subscriber notification list and move
// =============================================================================

/// Page size cap of the list, and the id cap of one move.
pub const MAX_SUBSCRIBER_NOTIFICATION_PAGE: i64 = 100;

#[derive(Debug, Deserialize)]
pub struct SubscriberNotificationsQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StoredNotification {
    /// TypeID, `notif_…`.
    pub id: String,
    pub category: String,
    #[schema(value_type = Object)]
    pub payload: Value,
    pub visible_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StoredNotificationPage {
    pub items: Vec<StoredNotification>,
    /// Opaque keyset token. Null when the last page is reached.
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MoveNotificationsRequest {
    /// The subscriber that receives the notifications. 1-255 characters, no newline.
    pub to_subscriber_id: String,
    /// Up to 100 `notif_…` ids. Ids that the source does not hold are skipped.
    pub notification_ids: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MoveNotificationsResponse {
    pub moved: u64,
}

#[utoipa::path(
    get,
    path = "/v1/subscribers/{subscriber_id}/notifications",
    tag = "management",
    operation_id = "listSubscriberNotifications",
    summary = "List a subscriber's notifications",
    description = r#"Every stored direct notification of one subscriber, oldest first, archived and scheduled ones included. Use next_cursor for the next page. An unknown subscriber returns an empty page."#,
    params(
        ("subscriber_id" = String, Path, max_length = 255, description = "Customer-provided subscriber id."),
        ("cursor" = Option<String>, Query, description = "Opaque keyset cursor. Omit it for the first page."),
        ("limit" = Option<i64>, Query, minimum = 1, maximum = 100, description = "Page size. The default is 100."),
    ),
    responses(
        (status = 200, description = "A page of notifications.", body = StoredNotificationPage),
        (
            status = 400,
            description = "Malformed cursor or out-of-range limit.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "limit must be between 1 and 100"}}),
        ),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid API key"}}),
        ),
        (status = 429, response = crate::api::contract::RateLimited),
    ),
    security(("ApiKeyBearer" = []))
)]
pub async fn list_subscriber_notifications(
    State(state): State<AppState>,
    auth: ManagementAuth,
    Path(subscriber_id): Path<String>,
    ApiQuery(query): ApiQuery<SubscriberNotificationsQuery>,
) -> Result<Json<StoredNotificationPage>, ApiError> {
    enforce_api_key_limit(&state, &auth).await?;
    let env = auth.environment_id;
    validate_subscriber_id(&subscriber_id, "subscriber_id")?;
    let limit = query.limit.unwrap_or(MAX_SUBSCRIBER_NOTIFICATION_PAGE);
    if !(1..=MAX_SUBSCRIBER_NOTIFICATION_PAGE).contains(&limit) {
        return Err(ApiError::bad_request("limit must be between 1 and 100"));
    }
    let (after_ts, after_id) = match &query.cursor {
        None => (None, None),
        Some(c) => {
            let (ts, id) = api::inbox::decode_cursor(c)
                .ok_or_else(|| ApiError::bad_request("malformed cursor"))?;
            (Some(ts), Some(id))
        }
    };

    let internal = sqlx::query_scalar!(
        "SELECT id FROM subscribers WHERE environment_id = $1 AND subscriber_id = $2",
        env,
        &subscriber_id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let Some(internal) = internal else {
        return Ok(Json(StoredNotificationPage {
            items: Vec::new(),
            next_cursor: None,
        }));
    };

    let rows = sqlx::query!(
        r#"SELECT id, category, payload, visible_at FROM notifications
            WHERE environment_id = $1 AND subscriber_id = $2
              AND ($3::timestamptz IS NULL OR (visible_at, id) > ($3, $4::uuid))
            ORDER BY visible_at, id
            LIMIT $5"#,
        env,
        internal,
        after_ts,
        after_id,
        limit,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let next_cursor = (rows.len() as i64 == limit)
        .then(|| {
            rows.last()
                .map(|r| api::inbox::encode_cursor(r.visible_at, r.id))
        })
        .flatten();
    let items = rows
        .into_iter()
        .map(|r| StoredNotification {
            id: ids::typeid(ids::NOTIFICATION, r.id),
            category: r.category,
            payload: r.payload,
            visible_at: api::format_ts(r.visible_at),
        })
        .collect();
    Ok(Json(StoredNotificationPage { items, next_cursor }))
}

#[utoipa::path(
    post,
    path = "/v1/subscribers/{subscriber_id}/notifications/move",
    tag = "management",
    operation_id = "moveSubscriberNotifications",
    summary = "Move notifications to another subscriber",
    description = r#"Move up to 100 visible direct notifications to another subscriber. Each notification keeps its read and archive state. The category mutes of the source are copied to the target. Ids that the source does not hold, and scheduled notifications that are not yet visible, are skipped, so a repeated call is safe. The target subscriber is created if it does not exist."#,
    params(("subscriber_id" = String, Path, max_length = 255, description = "The source subscriber id.")),
    request_body = MoveNotificationsRequest,
    responses(
        (status = 200, description = "The number of moved notifications.", body = MoveNotificationsResponse),
        (
            status = 400,
            description = "Validation error.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "to_subscriber_id must differ from the source"}}),
        ),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid API key"}}),
        ),
        (status = 429, response = crate::api::contract::RateLimited),
    ),
    security(("ApiKeyBearer" = []))
)]
pub async fn move_subscriber_notifications(
    State(state): State<AppState>,
    auth: ManagementAuth,
    Path(from): Path<String>,
    ApiJson(req): ApiJson<MoveNotificationsRequest>,
) -> Result<Json<MoveNotificationsResponse>, ApiError> {
    enforce_api_key_limit(&state, &auth).await?;
    let env = auth.environment_id;
    validate_subscriber_id(&from, "subscriber_id")?;
    let to = req.to_subscriber_id;
    validate_subscriber_id(&to, "to_subscriber_id")?;
    if to == from {
        return Err(ApiError::bad_request(
            "to_subscriber_id must differ from the source",
        ));
    }
    if req.notification_ids.len() > MAX_SUBSCRIBER_NOTIFICATION_PAGE as usize {
        return Err(ApiError::bad_request(
            "notification_ids must contain at most 100 ids",
        ));
    }
    let notification_ids = req
        .notification_ids
        .iter()
        .map(|id| ids::parse_typeid(ids::NOTIFICATION, id))
        .collect::<Option<Vec<Uuid>>>()
        .ok_or_else(|| ApiError::bad_request("notification_ids must be notif_ ids"))?;
    if notification_ids.is_empty() {
        return Ok(Json(MoveNotificationsResponse { moved: 0 }));
    }
    let moved = move_notifications_txn(&state, env, &from, &to, &notification_ids).await?;
    Ok(Json(MoveNotificationsResponse { moved }))
}

async fn move_notifications_txn(
    state: &AppState,
    env: Uuid,
    from: &str,
    to: &str,
    notification_ids: &[Uuid],
) -> anyhow::Result<u64> {
    let mut tx = state.pool.begin().await?;
    let source = sqlx::query_scalar!(
        "SELECT id FROM subscribers WHERE environment_id = $1 AND subscriber_id = $2",
        env,
        from,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(source) = source else {
        return Ok(0);
    };

    // No backdate. Broadcast read state does not move, so an older target
    // created_at would show old broadcasts as new.
    sqlx::query!(
        r#"INSERT INTO subscribers (environment_id, id, subscriber_id)
           VALUES ($1, $2, $3)
           ON CONFLICT (environment_id, subscriber_id) DO NOTHING"#,
        env,
        ids::new_uuid(),
        to,
    )
    .execute(&mut *tx)
    .await?;
    let target = sqlx::query_scalar!(
        "SELECT id FROM subscribers WHERE environment_id = $1 AND subscriber_id = $2",
        env,
        to,
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        r#"INSERT INTO subscriber_counters (environment_id, subscriber_id)
           SELECT $1, unnest($2::uuid[])
           ON CONFLICT (environment_id, subscriber_id) DO NOTHING"#,
        env,
        &[source, target][..],
    )
    .execute(&mut *tx)
    .await?;

    // Ascending uuid order, so two opposite moves cannot deadlock.
    let counters = sqlx::query!(
        r#"SELECT subscriber_id, read_watermark, seen_watermark, archive_watermark
             FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = ANY($2)
            ORDER BY subscriber_id
            FOR UPDATE"#,
        env,
        &[source, target][..],
    )
    .fetch_all(&mut *tx)
    .await?;
    let src = counters
        .iter()
        .find(|c| c.subscriber_id == source)
        .ok_or_else(|| anyhow::anyhow!("source counters row missing"))?;
    let dst = counters
        .iter()
        .find(|c| c.subscriber_id == target)
        .ok_or_else(|| anyhow::anyhow!("target counters row missing"))?;

    // State becomes explicit per row. An override is set only at or below the
    // target watermark (migration 0004). Above it, NULL already means unread
    // or unarchived. Rows still owned by a pending deliver job are skipped.
    let moved = sqlx::query!(
        r#"UPDATE notifications n SET
               subscriber_id = $3,
               read_at = CASE
                   WHEN n.read_at IS NOT NULL
                        OR (n.unread_at IS NULL AND n.visible_at <= $5)
                   THEN COALESCE(n.read_at, $5) END,
               unread_at = CASE
                   WHEN n.read_at IS NULL
                        AND (n.unread_at IS NOT NULL OR n.visible_at > $5)
                        AND n.visible_at <= $6
                   THEN COALESCE(n.unread_at, now()) END,
               archived_at = CASE
                   WHEN n.archived_at IS NOT NULL
                        OR (n.unarchived_at IS NULL AND n.visible_at <= $7)
                   THEN COALESCE(n.archived_at, $7) END,
               unarchived_at = CASE
                   WHEN n.archived_at IS NULL
                        AND (n.unarchived_at IS NOT NULL OR n.visible_at > $7)
                        AND n.visible_at <= $8
                   THEN COALESCE(n.unarchived_at, now()) END
           WHERE n.environment_id = $1 AND n.subscriber_id = $2
             AND n.id = ANY($4)
             AND n.visible_at <= now()
             AND NOT EXISTS (SELECT 1 FROM jobs j
                   CROSS JOIN LATERAL jsonb_array_elements_text(
                       CASE WHEN jsonb_typeof(j.payload->'notification_ids') = 'array'
                            THEN j.payload->'notification_ids' END)
                       WITH ORDINALITY AS t(nid, idx)
                   WHERE j.environment_id = $1 AND j.job_type = 'deliver'
                     AND t.nid = n.id::text
                     AND (t.idx - 1) >=
                         COALESCE((j.progress_cursor->>'offset')::bigint, 0))
           RETURNING n.visible_at, (n.visible_at <= $9) AS "was_seen!""#,
        env,
        source,
        target,
        notification_ids,
        src.read_watermark,
        dst.read_watermark,
        src.archive_watermark,
        dst.archive_watermark,
        src.seen_watermark,
    )
    .fetch_all(&mut *tx)
    .await?;
    if moved.is_empty() {
        tx.commit().await?;
        return Ok(0);
    }

    if let Some(seen) = moved
        .iter()
        .filter(|r| r.was_seen)
        .map(|r| r.visible_at)
        .max()
    {
        sqlx::query!(
            r#"UPDATE subscriber_counters SET seen_watermark = GREATEST(seen_watermark, $3)
                WHERE environment_id = $1 AND subscriber_id = $2"#,
            env,
            target,
            seen,
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(
        r#"INSERT INTO preferences (environment_id, subscriber_id, category, channel, enabled)
           SELECT environment_id, $3, category, channel, enabled FROM preferences
            WHERE environment_id = $1 AND subscriber_id = $2
           ON CONFLICT DO NOTHING"#,
        env,
        source,
        target,
    )
    .execute(&mut *tx)
    .await?;
    worker::recount_subscriber(&mut tx, env, source).await?;
    worker::recount_subscriber(&mut tx, env, target).await?;
    jobs::enqueue_hint(&mut tx, env, &[source, target], "read_state", &[]).await?;
    tx.commit().await?;
    Ok(moved.len() as u64)
}

fn validate_subscriber_id(id: &str, field: &str) -> Result<(), ApiError> {
    if id.is_empty() || id.len() > 255 {
        return Err(ApiError::bad_request(format!(
            "{field} must be 1–255 characters"
        )));
    }
    if id.contains('\n') {
        return Err(ApiError::bad_request(format!(
            "{field} must not contain a newline"
        )));
    }
    Ok(())
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
    // One notification row per recipient. Duplicates collapse.
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

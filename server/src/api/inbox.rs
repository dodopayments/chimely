//! Subscriber plane: the merged two-source inbox.
//!
//! Each source arm is independently keyset-limited (newest first,
//! `(occurred_at, id)` tuple keyset, id tiebreaker makes it total) then merged.
//! The unread count is the maintained direct counter plus a tiny broadcasts
//! range count minus the subscriber's own exception rows above the watermark.
//! It is never O(rows).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::api::format_ts;
use crate::auth::SubscriberAuth;
use crate::error::ApiError;
use crate::extract::ApiQuery;
use crate::state::AppState;
use crate::{ids, jobs, ratelimit, timeline};

pub const DEFAULT_PAGE_SIZE: i64 = 20;
pub const MAX_PAGE_SIZE: i64 = 100;
pub const CACHE_CONTROL: &str = "private, max-age=0";

#[derive(Debug, Serialize)]
pub struct InboxItem {
    /// TypeID. The prefix already encodes the source.
    pub id: String,
    /// Routes mark-read to the right endpoint.
    pub source: &'static str,
    pub category: String,
    pub payload: Value,
    /// Ordering timestamp. `visible_at` for direct, `created_at` for broadcast.
    pub occurred_at: String,
    /// Per-item exception OR at-or-below the read watermark.
    pub read: bool,
    /// Per-item override OR at-or-below the archive watermark.
    pub archived: bool,
}

#[derive(Debug, Serialize)]
pub struct InboxPage {
    pub items: Vec<InboxItem>,
    /// Opaque keyset token. Null when the last page is reached.
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InboxCounts {
    pub unread: i32,
    pub unseen: i32,
}

#[derive(Debug, Deserialize)]
pub struct ListParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub filter: Option<String>,
}

/// Server-side list views. The default view excludes nothing (v1). The
/// unread view filters both arms by the same expression the `read` column
/// reports, so the view and the flag can never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxFilter {
    Default,
    Unread,
    Archived,
}

impl InboxFilter {
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        match raw {
            None => Some(Self::Default),
            Some("unread") => Some(Self::Unread),
            Some("archived") => Some(Self::Archived),
            Some(_) => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Unread => "unread",
            Self::Archived => "archived",
        }
    }
}

#[utoipa::path(
    get,
    path = "/v1/inbox/items",
    tag = "subscriber",
    operation_id = "listInboxItems",
    summary = "List inbox items",
    description = r#"The subscriber's inbox, newest first, combining direct notifications and broadcasts. Use cursor for pagination and If-None-Match for conditional fetches."#,
    params(
        ("cursor" = Option<String>, Query, description = "Opaque keyset cursor; omit for the first page."),
        ("limit" = Option<i32>, Query, minimum = 1, maximum = 100),
        ("filter" = Option<String>, Query, description = "View filter. Omit for the default view, or `unread` for unread items only."),
        ("If-None-Match" = Option<String>, Header),
    ),
    responses(
        (status = 200, description = "A page of inbox items.", body = crate::api::contract::InboxPage,
            headers(
                ("ETag" = String, description = "Strong validator for this page + read state."),
                ("Cache-Control" = String, description = "Always `private, max-age=0`."),
            )),
        (status = 304, description = "Not modified (If-None-Match matched)."),
        // The handler rejects an out-of-range `limit` and a malformed `cursor`,
        // and the query extractor rejects a non-integer `limit`, all with 400.
        (
            status = 400,
            description = "Malformed cursor or out-of-range limit.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "malformed cursor"}}),
        ),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
        (status = 429, response = crate::api::contract::RateLimited),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn list_items(
    State(state): State<AppState>,
    auth: SubscriberAuth,
    ApiQuery(params): ApiQuery<ListParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // One bucket per subscriber, shared across replicas (widget refetch
    // storms are the load this guards against).
    ratelimit::enforce(
        state.ratelimit.as_ref(),
        &format!("sub:{}:{}", auth.environment_id, auth.subscriber_id),
        state.cfg.subscriber_rate_per_sec,
        state.cfg.subscriber_rate_burst,
    )
    .await?;
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_SIZE);
    if !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err(ApiError::bad_request("limit must be between 1 and 100"));
    }
    let filter = InboxFilter::parse(params.filter.as_deref())
        .ok_or_else(|| ApiError::bad_request("unknown filter"))?;
    let (cursor_ts, cursor_id) = match &params.cursor {
        None => (DateTime::<Utc>::MAX_UTC, Uuid::max()),
        Some(c) => decode_cursor(c).ok_or_else(|| ApiError::bad_request("malformed cursor"))?,
    };

    let etag = compute_etag(&state, &auth, params.cursor.as_deref(), limit, filter).await?;
    let response_headers = [
        (header::ETAG, etag.clone()),
        (header::CACHE_CONTROL, CACHE_CONTROL.to_owned()),
    ];
    // Weak comparison: proxies that recompress responses rewrite ETags to
    // `W/"..."`, and RFC 9110 If-None-Match always compares weakly.
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
            })
        })
    {
        return Ok((StatusCode::NOT_MODIFIED, response_headers).into_response());
    }

    let rows = list_items_for(
        &state.pool,
        auth.environment_id,
        auth.subscriber_id,
        auth.subscriber_created_at,
        cursor_ts,
        cursor_id,
        limit,
        filter,
    )
    .await
    .map_err(ApiError::from)?;

    let next_cursor = (rows.len() as i64 == limit)
        .then(|| rows.last().map(|r| encode_cursor(r.occurred_at, r.id)))
        .flatten();
    let items: Vec<InboxItem> = rows.into_iter().map(InboxItem::from).collect();

    Ok((response_headers, Json(InboxPage { items, next_cursor })).into_response())
}

/// One row of the merged two-source inbox, with the source's native id type
/// and ordering timestamp intact so callers can keyset-paginate. The
/// subscriber plane and the admin subscriber-lookup both read through this one
/// function. A second implementation of the merge would be a bug.
pub struct MergedRow {
    pub source: &'static str,
    pub id: Uuid,
    pub category: String,
    pub payload: Value,
    pub occurred_at: DateTime<Utc>,
    pub read: bool,
    pub archived: bool,
}

impl From<MergedRow> for InboxItem {
    fn from(row: MergedRow) -> Self {
        let prefix = match row.source {
            "notification" => ids::NOTIFICATION,
            _ => ids::BROADCAST,
        };
        Self {
            id: ids::typeid(prefix, row.id),
            source: row.source,
            category: row.category,
            payload: row.payload,
            occurred_at: format_ts(row.occurred_at),
            read: row.read,
            archived: row.archived,
        }
    }
}

/// The canonical merged list query. Both arms are keyset range scans over
/// notifications_inbox_idx and broadcasts_window_idx. Category mutes are
/// read-time NOT EXISTS probes. `(cursor_ts, cursor_id)` is the last item of
/// the previous page, or `(MAX_UTC, Uuid::max())` for the first page.
pub async fn list_items_for<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    environment_id: Uuid,
    subscriber_id: Uuid,
    subscriber_created_at: DateTime<Utc>,
    cursor_ts: DateTime<Utc>,
    cursor_id: Uuid,
    limit: i64,
    filter: InboxFilter,
) -> sqlx::Result<Vec<MergedRow>> {
    let rows = sqlx::query!(
        r#"SELECT merged.source AS "source!", merged.id AS "id!",
                  merged.category AS "category!", merged.payload AS "payload!",
                  merged.occurred_at AS "occurred_at!", merged.read AS "read!",
                  merged.archived AS "archived!"
           FROM (
             (SELECT 'notification' AS source, n.id, n.category, n.payload,
                     n.visible_at AS occurred_at,
                     (n.read_at IS NOT NULL
                        OR (n.unread_at IS NULL AND n.visible_at <= c.read_watermark)) AS read,
                     (n.archived_at IS NOT NULL
                        OR (n.unarchived_at IS NULL
                            AND n.visible_at <= c.archive_watermark)) AS archived
                FROM notifications n
                JOIN subscriber_counters c
                  ON c.environment_id = n.environment_id
                 AND c.subscriber_id  = n.subscriber_id
               WHERE n.environment_id = $1 AND n.subscriber_id = $2
                 AND n.visible_at <= now()
                 AND (n.visible_at, n.id) < ($3, $4)
                 AND ($7 <> 'unread' OR (n.read_at IS NULL
                        AND (n.unread_at IS NOT NULL OR n.visible_at > c.read_watermark)))
                 AND CASE WHEN $7 = 'archived'
                          THEN (n.archived_at IS NOT NULL
                                OR (n.unarchived_at IS NULL
                                    AND n.visible_at <= c.archive_watermark))
                          ELSE NOT (n.archived_at IS NOT NULL
                                OR (n.unarchived_at IS NULL
                                    AND n.visible_at <= c.archive_watermark))
                     END
                 AND NOT EXISTS (SELECT 1 FROM preferences p
                       WHERE p.environment_id = n.environment_id
                         AND p.subscriber_id  = n.subscriber_id
                         AND p.category = n.category AND p.channel = 'in_app'
                         AND p.enabled = false)
               ORDER BY n.visible_at DESC, n.id DESC LIMIT $5)
             UNION ALL
             (SELECT 'broadcast', b.id, b.category, b.payload, b.created_at,
                     COALESCE(br.read, b.created_at <= c.read_watermark),
                     COALESCE(ba.archived, b.created_at <= c.archive_watermark)
                FROM broadcasts b
                JOIN subscriber_counters c
                  ON c.environment_id = b.environment_id AND c.subscriber_id = $2
                LEFT JOIN broadcast_reads br
                  ON br.environment_id = b.environment_id
                 AND br.subscriber_id  = $2
                 AND br.broadcast_id   = b.id
                LEFT JOIN broadcast_archives ba
                  ON ba.environment_id = b.environment_id
                 AND ba.subscriber_id  = $2
                 AND ba.broadcast_id   = b.id
               WHERE b.environment_id = $1
                 AND b.created_at >= $6
                 AND (b.created_at, b.id) < ($3, $4)
                 AND ($7 <> 'unread'
                        OR NOT COALESCE(br.read, b.created_at <= c.read_watermark))
                 AND CASE WHEN $7 = 'archived'
                          THEN COALESCE(ba.archived, b.created_at <= c.archive_watermark)
                          ELSE NOT COALESCE(ba.archived, b.created_at <= c.archive_watermark)
                     END
                 AND NOT EXISTS (SELECT 1 FROM preferences p
                       WHERE p.environment_id = b.environment_id
                         AND p.subscriber_id  = $2
                         AND p.category = b.category AND p.channel = 'in_app'
                         AND p.enabled = false)
               ORDER BY b.created_at DESC, b.id DESC LIMIT $5)
           ) merged
           ORDER BY occurred_at DESC, id DESC
           LIMIT $5"#,
        environment_id,
        subscriber_id,
        cursor_ts,
        cursor_id,
        limit,
        subscriber_created_at,
        filter.as_str(),
    )
    .fetch_all(executor)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| MergedRow {
            source: if r.source == "notification" {
                "notification"
            } else {
                "broadcast"
            },
            id: r.id,
            category: r.category,
            payload: r.payload,
            occurred_at: r.occurred_at,
            read: r.read,
            archived: r.archived,
        })
        .collect())
}

#[utoipa::path(
    get,
    path = "/v1/inbox/counts",
    tag = "subscriber",
    operation_id = "getInboxCounts",
    summary = "Inbox counts",
    description = r#"The subscriber's unread and unseen counts. unread reflects items not yet read; unseen drives the bell badge."#,
    responses(
        (status = 200, description = "Current counts.", body = crate::api::contract::InboxCounts),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn get_counts(
    State(state): State<AppState>,
    auth: SubscriberAuth,
) -> Result<Json<InboxCounts>, ApiError> {
    let mut conn = state.pool.acquire().await.map_err(ApiError::from)?;
    let counts = fetch_counts(&mut conn, &auth).await?;
    Ok(Json(counts))
}

/// The unread/unseen count: maintained direct counters plus two live
/// broadcast terms. Term one is evaluated exactly as the list's broadcast arm
/// (visible, above the watermark, not explicitly read, not muted). Term two
/// counts explicit unread overrides at or below the watermark and is bounded
/// by exception-row cardinality. Together they agree with the visible list at
/// all times while staying O(above-watermark + exceptions), never O(rows).
/// Seen has no exceptions term.
///
/// Category mutes are applied to the broadcast term here, unlike the maintained
/// direct counters. The broadcast term is recomputed on every read with no
/// stored counter to drift, so making it mute-aware keeps the two-source
/// invariant exact for broadcasts.
pub async fn fetch_counts(
    conn: &mut sqlx::PgConnection,
    auth: &SubscriberAuth,
) -> Result<InboxCounts, ApiError> {
    fetch_counts_for(
        conn,
        auth.environment_id,
        auth.subscriber_id,
        auth.subscriber_created_at,
    )
    .await
}

/// The canonical unread/unseen count, by explicit identity. The subscriber
/// plane and the admin subscriber-lookup share this one implementation so the
/// admin view can never disagree with what the subscriber sees.
pub async fn fetch_counts_for(
    conn: &mut sqlx::PgConnection,
    environment_id: Uuid,
    subscriber_id: Uuid,
    subscriber_created_at: DateTime<Utc>,
) -> Result<InboxCounts, ApiError> {
    let row = sqlx::query!(
        r#"SELECT
               greatest(0, c.unread_direct_count
                 + (SELECT count(*) FROM broadcasts b
                     WHERE b.environment_id = $1
                       AND b.created_at >= $3
                       AND b.created_at >  c.read_watermark
                       AND NOT EXISTS (SELECT 1 FROM broadcast_reads br
                             WHERE br.environment_id = b.environment_id
                               AND br.subscriber_id  = $2
                               AND br.broadcast_id   = b.id
                               AND br.read)
                       AND NOT COALESCE((SELECT ba.archived FROM broadcast_archives ba
                             WHERE ba.environment_id = b.environment_id
                               AND ba.subscriber_id  = $2
                               AND ba.broadcast_id   = b.id),
                             b.created_at <= c.archive_watermark)
                       AND NOT EXISTS (SELECT 1 FROM preferences p
                             WHERE p.environment_id = b.environment_id
                               AND p.subscriber_id  = $2
                               AND p.category = b.category AND p.channel = 'in_app'
                               AND p.enabled = false))
                 + (SELECT count(*) FROM broadcast_reads br
                     JOIN broadcasts b
                       ON b.environment_id = br.environment_id AND b.id = br.broadcast_id
                     WHERE br.environment_id = $1
                       AND br.subscriber_id  = $2
                       AND NOT br.read
                       AND br.broadcast_created_at <= c.read_watermark
                       AND b.created_at >= $3
                       AND NOT COALESCE((SELECT ba.archived FROM broadcast_archives ba
                             WHERE ba.environment_id = b.environment_id
                               AND ba.subscriber_id  = $2
                               AND ba.broadcast_id   = b.id),
                             b.created_at <= c.archive_watermark)
                       AND NOT EXISTS (SELECT 1 FROM preferences p
                             WHERE p.environment_id = b.environment_id
                               AND p.subscriber_id  = $2
                               AND p.category = b.category AND p.channel = 'in_app'
                               AND p.enabled = false)))::int AS "unread!",
               greatest(0, c.unseen_direct_count
                 + (SELECT count(*) FROM broadcasts b
                     WHERE b.environment_id = $1
                       AND b.created_at >= $3
                       AND b.created_at >  c.seen_watermark
                       AND NOT EXISTS (SELECT 1 FROM preferences p
                             WHERE p.environment_id = b.environment_id
                               AND p.subscriber_id  = $2
                               AND p.category = b.category AND p.channel = 'in_app'
                               AND p.enabled = false)))::int AS "unseen!"
           FROM subscriber_counters c
           WHERE c.environment_id = $1 AND c.subscriber_id = $2"#,
        environment_id,
        subscriber_id,
        subscriber_created_at,
    )
    .fetch_one(conn)
    .await
    .map_err(ApiError::from)?;
    Ok(InboxCounts {
        unread: row.unread,
        unseen: row.unseen,
    })
}

#[utoipa::path(
    post,
    path = "/v1/inbox/notifications/{id}/read",
    tag = "subscriber",
    operation_id = "markNotificationRead",
    summary = "Mark notification read",
    description = "Mark one direct notification as read. Idempotent.",
    params(("id" = crate::api::contract::NotificationId, Path)),
    responses(
        (status = 204, description = "Read (now or already)."),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
        (
            status = 404,
            description = "Resource not found in this environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such notification"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn mark_notification_read(
    State(state): State<AppState>,
    auth: SubscriberAuth,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = ids::parse_typeid(ids::NOTIFICATION, &id)
        .ok_or_else(|| ApiError::not_found("no such notification"))?;

    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    // Lock the counters row FIRST, in its own statement. Every later
    // statement in this transaction then starts after the lock is held and
    // reads a fresh snapshot, which closes the READ COMMITTED stale-subquery
    // race against a concurrent deliver job (EvalPlanQual rechecks re-read
    // the locked row but NOT other tables).
    sqlx::query!(
        r#"SELECT 1 AS one FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    // Scheduled rows (visible_at > now()) are excluded from all subscriber
    // queries. An invisible item cannot be marked read.
    let row = sqlx::query!(
        r#"SELECT visible_at, read_at, unread_at, archived_at, unarchived_at, category
            FROM notifications
            WHERE environment_id = $1 AND id = $2 AND subscriber_id = $3
              AND visible_at <= now()
            FOR UPDATE"#,
        auth.environment_id,
        id,
        auth.subscriber_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no such notification"))?;

    if row.read_at.is_none() {
        sqlx::query!(
            r#"UPDATE notifications SET read_at = now(), unread_at = NULL
                WHERE environment_id = $1 AND id = $2 AND visible_at = $3"#,
            auth.environment_id,
            id,
            row.visible_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        // A visible row still owned by a pending deliver job is uncounted:
        // its counter increment has not happened yet, so decrementing now
        // would drift the counter by -1 forever (the deliver bump skips rows
        // with read_at set). The deliver job is the only counter for those
        // rows. The counters lock held here blocks the job from completing
        // concurrently.
        let pending = sqlx::query_scalar!(
            r#"SELECT EXISTS (
                   SELECT 1 FROM jobs j
                   CROSS JOIN LATERAL jsonb_array_elements_text(
                       CASE WHEN jsonb_typeof(j.payload->'notification_ids') = 'array'
                            THEN j.payload->'notification_ids' END)
                       WITH ORDINALITY AS t(nid, idx)
                   WHERE j.environment_id = $1 AND j.job_type = 'deliver'
                     AND t.nid = ($2::uuid)::text
                     AND (t.idx - 1) >= COALESCE((j.progress_cursor->>'offset')::bigint, 0)
               ) AS "pending!""#,
            auth.environment_id,
            id,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        // Decrement ONLY IF the row was counted: unread above the watermark,
        // or an explicit unread override below it (otherwise
        // double-decrement). The mute guard pairs with the mute-aware
        // increment: a muted row was never counted, so marking it read must
        // not steal a count from an unmuted item.
        // updated_at bump: EVERY read-state mutation is an ETag input.
        sqlx::query!(
            r#"UPDATE subscriber_counters c SET
                   unread_direct_count = greatest(0,
                       c.unread_direct_count - (($3 > c.read_watermark OR $6) AND NOT $4
                           AND NOT ($7 OR (NOT $8 AND $3 <= c.archive_watermark))
                           AND NOT EXISTS (SELECT 1 FROM preferences p
                                 WHERE p.environment_id = c.environment_id
                                   AND p.subscriber_id  = c.subscriber_id
                                   AND p.category = $5 AND p.channel = 'in_app'
                                   AND p.enabled = false))::int),
                   updated_at = now()
             WHERE c.environment_id = $1 AND c.subscriber_id = $2"#,
            auth.environment_id,
            auth.subscriber_id,
            row.visible_at,
            pending,
            row.category,
            row.unread_at.is_some(),
            row.archived_at.is_some(),
            row.unarchived_at.is_some(),
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        // The 'read' timeline row commits with the read_at flip. The guard
        // inside append runs under the counters lock taken above, so the
        // watermark timeline job can never double-append it.
        timeline::append(&mut tx, auth.environment_id, &[id], timeline::STATUS_READ)
            .await
            .map_err(ApiError::from)?;
        jobs::enqueue_hint(
            &mut tx,
            auth.environment_id,
            &[auth.subscriber_id],
            "read_state",
            &[],
        )
        .await
        .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/v1/inbox/notifications/{id}/unread",
    tag = "subscriber",
    operation_id = "markNotificationUnread",
    summary = "Mark notification unread",
    description = "Mark one direct notification as unread. The override survives the read watermark. Idempotent.",
    params(("id" = crate::api::contract::NotificationId, Path)),
    responses(
        (status = 204, description = "Unread (now or already)."),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
        (
            status = 404,
            description = "Resource not found in this environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such notification"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn mark_notification_unread(
    State(state): State<AppState>,
    auth: SubscriberAuth,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = ids::parse_typeid(ids::NOTIFICATION, &id)
        .ok_or_else(|| ApiError::not_found("no such notification"))?;

    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    // Counters lock first, same discipline as mark_notification_read. The
    // watermark read under the lock decides whether an override is needed.
    let watermark = sqlx::query_scalar!(
        r#"SELECT read_watermark FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    let row = sqlx::query!(
        r#"SELECT visible_at, read_at, unread_at, archived_at, unarchived_at, category
            FROM notifications
            WHERE environment_id = $1 AND id = $2 AND subscriber_id = $3
              AND visible_at <= now()
            FOR UPDATE"#,
        auth.environment_id,
        id,
        auth.subscriber_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no such notification"))?;

    let effectively_read =
        row.read_at.is_some() || (row.unread_at.is_none() && row.visible_at <= watermark);
    if effectively_read {
        // Above the watermark, clearing read_at alone means unread. At or
        // below it, the override column keeps the item unread despite the
        // watermark.
        let needs_override = row.visible_at <= watermark;
        sqlx::query!(
            r#"UPDATE notifications
                  SET read_at = NULL,
                      unread_at = CASE WHEN $4 THEN now() ELSE NULL END
                WHERE environment_id = $1 AND id = $2 AND visible_at = $3"#,
            auth.environment_id,
            id,
            row.visible_at,
            needs_override,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        // A row still owned by a pending deliver job stays uncounted here:
        // the deliver bump reads the new expression (unread_at survives) and
        // counts it exactly once. The counters lock blocks that job from
        // completing concurrently.
        let pending = sqlx::query_scalar!(
            r#"SELECT EXISTS (
                   SELECT 1 FROM jobs j
                   CROSS JOIN LATERAL jsonb_array_elements_text(
                       CASE WHEN jsonb_typeof(j.payload->'notification_ids') = 'array'
                            THEN j.payload->'notification_ids' END)
                       WITH ORDINALITY AS t(nid, idx)
                   WHERE j.environment_id = $1 AND j.job_type = 'deliver'
                     AND t.nid = ($2::uuid)::text
                     AND (t.idx - 1) >= COALESCE((j.progress_cursor->>'offset')::bigint, 0)
               ) AS "pending!""#,
            auth.environment_id,
            id,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        // Increment mirrors the mark-read decrement: only rows the counter
        // would have counted (not pending, not muted) gain the +1.
        sqlx::query!(
            r#"UPDATE subscriber_counters c SET
                   unread_direct_count = c.unread_direct_count + (NOT $3
                       AND NOT ($5 OR (NOT $6 AND $7 <= c.archive_watermark))
                       AND NOT EXISTS (SELECT 1 FROM preferences p
                             WHERE p.environment_id = c.environment_id
                               AND p.subscriber_id  = c.subscriber_id
                               AND p.category = $4 AND p.channel = 'in_app'
                               AND p.enabled = false))::int,
                   updated_at = now()
             WHERE c.environment_id = $1 AND c.subscriber_id = $2"#,
            auth.environment_id,
            auth.subscriber_id,
            pending,
            row.category,
            row.archived_at.is_some(),
            row.unarchived_at.is_some(),
            row.visible_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        // No timeline row: notification_status_log is append-once per
        // (notification, status), so a read/unread/read cycle cannot be
        // represented there. Unread transitions are not lifecycle events.
        jobs::enqueue_hint(
            &mut tx,
            auth.environment_id,
            &[auth.subscriber_id],
            "read_state",
            &[],
        )
        .await
        .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/v1/inbox/broadcasts/{id}/read",
    tag = "subscriber",
    operation_id = "markBroadcastRead",
    summary = "Mark broadcast read",
    description = "Mark one broadcast as read for this subscriber. Idempotent.",
    params(("id" = crate::api::contract::BroadcastId, Path)),
    responses(
        (status = 204, description = "Read (now or already)."),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
        (
            status = 404,
            description = "Resource not found in this environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such broadcast"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn mark_broadcast_read(
    State(state): State<AppState>,
    auth: SubscriberAuth,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = ids::parse_typeid(ids::BROADCAST, &id)
        .ok_or_else(|| ApiError::not_found("no such broadcast"))?;

    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    // Lock the counters row first: serializes against mark-all-read so the
    // exception insert can never race past the watermark GC.
    let watermark = sqlx::query_scalar!(
        r#"SELECT read_watermark FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    let broadcast_created_at = sqlx::query_scalar!(
        r#"SELECT created_at FROM broadcasts WHERE environment_id = $1 AND id = $2"#,
        auth.environment_id,
        id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::from)?
    // The visibility rule doubles as the existence rule: a broadcast from
    // before the subscriber existed is not in their inbox.
    .filter(|created_at| *created_at >= auth.subscriber_created_at)
    .ok_or_else(|| ApiError::not_found("no such broadcast"))?;

    // Above the watermark an explicit read row decides. At or below it the
    // watermark already reads the item, so the only work is deleting a
    // possible unread override.
    let changed = if broadcast_created_at > watermark {
        sqlx::query!(
            r#"INSERT INTO broadcast_reads
                   (environment_id, subscriber_id, broadcast_id, broadcast_created_at, read)
               VALUES ($1, $2, $3, $4, true)
               ON CONFLICT (environment_id, subscriber_id, broadcast_id)
               DO UPDATE SET read = true, read_at = now()
               WHERE broadcast_reads.read = false"#,
            auth.environment_id,
            auth.subscriber_id,
            id,
            broadcast_created_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?
        .rows_affected()
    } else {
        sqlx::query!(
            r#"DELETE FROM broadcast_reads
                WHERE environment_id = $1 AND subscriber_id = $2
                  AND broadcast_id = $3 AND read = false"#,
            auth.environment_id,
            auth.subscriber_id,
            id,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?
        .rows_affected()
    };
    if changed > 0 {
        // No maintained counter changes, but updated_at must move: it is
        // the change-detection input for the list ETag.
        sqlx::query!(
            r#"UPDATE subscriber_counters SET updated_at = now()
                WHERE environment_id = $1 AND subscriber_id = $2"#,
            auth.environment_id,
            auth.subscriber_id,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        jobs::enqueue_hint(
            &mut tx,
            auth.environment_id,
            &[auth.subscriber_id],
            "read_state",
            &[],
        )
        .await
        .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/v1/inbox/broadcasts/{id}/unread",
    tag = "subscriber",
    operation_id = "markBroadcastUnread",
    summary = "Mark broadcast unread",
    description = "Mark one broadcast as unread for this subscriber. The override survives the read watermark. Idempotent.",
    params(("id" = crate::api::contract::BroadcastId, Path)),
    responses(
        (status = 204, description = "Unread (now or already)."),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
        (
            status = 404,
            description = "Resource not found in this environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such broadcast"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn mark_broadcast_unread(
    State(state): State<AppState>,
    auth: SubscriberAuth,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = ids::parse_typeid(ids::BROADCAST, &id)
        .ok_or_else(|| ApiError::not_found("no such broadcast"))?;

    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    // Counters lock first: serializes against mark-all-read so an unread
    // override can never race the watermark GC.
    let watermark = sqlx::query_scalar!(
        r#"SELECT read_watermark FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    let broadcast_created_at = sqlx::query_scalar!(
        r#"SELECT created_at FROM broadcasts WHERE environment_id = $1 AND id = $2"#,
        auth.environment_id,
        id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::from)?
    .filter(|created_at| *created_at >= auth.subscriber_created_at)
    .ok_or_else(|| ApiError::not_found("no such broadcast"))?;

    // Above the watermark, absence of a read row already means unread, so
    // deleting the row is the whole operation. At or below it, an explicit
    // unread override outranks the watermark.
    let changed = if broadcast_created_at > watermark {
        sqlx::query!(
            r#"DELETE FROM broadcast_reads
                WHERE environment_id = $1 AND subscriber_id = $2
                  AND broadcast_id = $3 AND read = true"#,
            auth.environment_id,
            auth.subscriber_id,
            id,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?
        .rows_affected()
    } else {
        sqlx::query!(
            r#"INSERT INTO broadcast_reads
                   (environment_id, subscriber_id, broadcast_id, broadcast_created_at, read)
               VALUES ($1, $2, $3, $4, false)
               ON CONFLICT (environment_id, subscriber_id, broadcast_id)
               DO UPDATE SET read = false, read_at = now()
               WHERE broadcast_reads.read = true"#,
            auth.environment_id,
            auth.subscriber_id,
            id,
            broadcast_created_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?
        .rows_affected()
    };
    if changed > 0 {
        sqlx::query!(
            r#"UPDATE subscriber_counters SET updated_at = now()
                WHERE environment_id = $1 AND subscriber_id = $2"#,
            auth.environment_id,
            auth.subscriber_id,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        jobs::enqueue_hint(
            &mut tx,
            auth.environment_id,
            &[auth.subscriber_id],
            "read_state",
            &[],
        )
        .await
        .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/v1/inbox/notifications/{id}/archive",
    tag = "subscriber",
    operation_id = "archiveNotification",
    summary = "Archive notification",
    description = "Archive one direct notification. Archiving never changes read state. Idempotent.",
    params(("id" = crate::api::contract::NotificationId, Path)),
    responses(
        (status = 204, description = "Archived (now or already)."),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
        (
            status = 404,
            description = "Resource not found in this environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such notification"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn archive_notification(
    State(state): State<AppState>,
    auth: SubscriberAuth,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = ids::parse_typeid(ids::NOTIFICATION, &id)
        .ok_or_else(|| ApiError::not_found("no such notification"))?;

    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    let watermark = sqlx::query_scalar!(
        r#"SELECT archive_watermark FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    let row = sqlx::query!(
        r#"SELECT visible_at, read_at, unread_at, archived_at, unarchived_at, category
            FROM notifications
            WHERE environment_id = $1 AND id = $2 AND subscriber_id = $3
              AND visible_at <= now()
            FOR UPDATE"#,
        auth.environment_id,
        id,
        auth.subscriber_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no such notification"))?;

    let effectively_archived =
        row.archived_at.is_some() || (row.unarchived_at.is_none() && row.visible_at <= watermark);
    if !effectively_archived {
        sqlx::query!(
            r#"UPDATE notifications SET archived_at = now(), unarchived_at = NULL
                WHERE environment_id = $1 AND id = $2 AND visible_at = $3"#,
            auth.environment_id,
            id,
            row.visible_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        let pending = pending_deliver(&mut tx, auth.environment_id, id).await?;
        // Archiving an unread item removes it from the count (unread means
        // unread AND not archived). Read, pending, and muted items were not
        // counted, so they must not decrement.
        sqlx::query!(
            r#"UPDATE subscriber_counters c SET
                   unread_direct_count = greatest(0,
                       c.unread_direct_count - ($3 AND ($4 OR $5 > c.read_watermark) AND NOT $6
                           AND NOT EXISTS (SELECT 1 FROM preferences p
                                 WHERE p.environment_id = c.environment_id
                                   AND p.subscriber_id  = c.subscriber_id
                                   AND p.category = $7 AND p.channel = 'in_app'
                                   AND p.enabled = false))::int),
                   updated_at = now()
             WHERE c.environment_id = $1 AND c.subscriber_id = $2"#,
            auth.environment_id,
            auth.subscriber_id,
            row.read_at.is_none(),
            row.unread_at.is_some(),
            row.visible_at,
            pending,
            row.category,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        jobs::enqueue_hint(
            &mut tx,
            auth.environment_id,
            &[auth.subscriber_id],
            "archive_state",
            &[],
        )
        .await
        .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/v1/inbox/notifications/{id}/unarchive",
    tag = "subscriber",
    operation_id = "unarchiveNotification",
    summary = "Unarchive notification",
    description = "Return one direct notification to the inbox. Read state is untouched. Idempotent.",
    params(("id" = crate::api::contract::NotificationId, Path)),
    responses(
        (status = 204, description = "Unarchived (now or already)."),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
        (
            status = 404,
            description = "Resource not found in this environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such notification"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn unarchive_notification(
    State(state): State<AppState>,
    auth: SubscriberAuth,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = ids::parse_typeid(ids::NOTIFICATION, &id)
        .ok_or_else(|| ApiError::not_found("no such notification"))?;

    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    let watermark = sqlx::query_scalar!(
        r#"SELECT archive_watermark FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    let row = sqlx::query!(
        r#"SELECT visible_at, read_at, unread_at, archived_at, unarchived_at, category
            FROM notifications
            WHERE environment_id = $1 AND id = $2 AND subscriber_id = $3
              AND visible_at <= now()
            FOR UPDATE"#,
        auth.environment_id,
        id,
        auth.subscriber_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no such notification"))?;

    let effectively_archived =
        row.archived_at.is_some() || (row.unarchived_at.is_none() && row.visible_at <= watermark);
    if effectively_archived {
        // At or below the archive watermark the override column keeps the
        // item unarchived despite the watermark.
        let needs_override = row.visible_at <= watermark;
        sqlx::query!(
            r#"UPDATE notifications
                  SET archived_at = NULL,
                      unarchived_at = CASE WHEN $4 THEN now() ELSE NULL END
                WHERE environment_id = $1 AND id = $2 AND visible_at = $3"#,
            auth.environment_id,
            id,
            row.visible_at,
            needs_override,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        let pending = pending_deliver(&mut tx, auth.environment_id, id).await?;
        // An unread item re-enters the count. Read state never changed while
        // archived, so unarchiving below the read watermark comes back read.
        sqlx::query!(
            r#"UPDATE subscriber_counters c SET
                   unread_direct_count = c.unread_direct_count + ($3
                       AND ($4 OR $5 > c.read_watermark) AND NOT $6
                       AND NOT EXISTS (SELECT 1 FROM preferences p
                             WHERE p.environment_id = c.environment_id
                               AND p.subscriber_id  = c.subscriber_id
                               AND p.category = $7 AND p.channel = 'in_app'
                               AND p.enabled = false))::int,
                   updated_at = now()
             WHERE c.environment_id = $1 AND c.subscriber_id = $2"#,
            auth.environment_id,
            auth.subscriber_id,
            row.read_at.is_none(),
            row.unread_at.is_some(),
            row.visible_at,
            pending,
            row.category,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        jobs::enqueue_hint(
            &mut tx,
            auth.environment_id,
            &[auth.subscriber_id],
            "archive_state",
            &[],
        )
        .await
        .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/v1/inbox/broadcasts/{id}/archive",
    tag = "subscriber",
    operation_id = "archiveBroadcast",
    summary = "Archive broadcast",
    description = "Archive one broadcast for this subscriber. Idempotent.",
    params(("id" = crate::api::contract::BroadcastId, Path)),
    responses(
        (status = 204, description = "Archived (now or already)."),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
        (
            status = 404,
            description = "Resource not found in this environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such broadcast"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn archive_broadcast(
    State(state): State<AppState>,
    auth: SubscriberAuth,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = ids::parse_typeid(ids::BROADCAST, &id)
        .ok_or_else(|| ApiError::not_found("no such broadcast"))?;

    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    let watermark = sqlx::query_scalar!(
        r#"SELECT archive_watermark FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    let broadcast_created_at = sqlx::query_scalar!(
        r#"SELECT created_at FROM broadcasts WHERE environment_id = $1 AND id = $2"#,
        auth.environment_id,
        id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::from)?
    .filter(|created_at| *created_at >= auth.subscriber_created_at)
    .ok_or_else(|| ApiError::not_found("no such broadcast"))?;

    let changed = if broadcast_created_at > watermark {
        sqlx::query!(
            r#"INSERT INTO broadcast_archives
                   (environment_id, subscriber_id, broadcast_id, broadcast_created_at, archived)
               VALUES ($1, $2, $3, $4, true)
               ON CONFLICT (environment_id, subscriber_id, broadcast_id)
               DO UPDATE SET archived = true, updated_at = now()
               WHERE broadcast_archives.archived = false"#,
            auth.environment_id,
            auth.subscriber_id,
            id,
            broadcast_created_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?
        .rows_affected()
    } else {
        sqlx::query!(
            r#"DELETE FROM broadcast_archives
                WHERE environment_id = $1 AND subscriber_id = $2
                  AND broadcast_id = $3 AND archived = false"#,
            auth.environment_id,
            auth.subscriber_id,
            id,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?
        .rows_affected()
    };
    if changed > 0 {
        sqlx::query!(
            r#"UPDATE subscriber_counters SET updated_at = now()
                WHERE environment_id = $1 AND subscriber_id = $2"#,
            auth.environment_id,
            auth.subscriber_id,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        jobs::enqueue_hint(
            &mut tx,
            auth.environment_id,
            &[auth.subscriber_id],
            "archive_state",
            &[],
        )
        .await
        .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/v1/inbox/broadcasts/{id}/unarchive",
    tag = "subscriber",
    operation_id = "unarchiveBroadcast",
    summary = "Unarchive broadcast",
    description = "Return one broadcast to this subscriber's inbox. The override survives the archive watermark. Idempotent.",
    params(("id" = crate::api::contract::BroadcastId, Path)),
    responses(
        (status = 204, description = "Unarchived (now or already)."),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
        (
            status = 404,
            description = "Resource not found in this environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such broadcast"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn unarchive_broadcast(
    State(state): State<AppState>,
    auth: SubscriberAuth,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = ids::parse_typeid(ids::BROADCAST, &id)
        .ok_or_else(|| ApiError::not_found("no such broadcast"))?;

    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    let watermark = sqlx::query_scalar!(
        r#"SELECT archive_watermark FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    let broadcast_created_at = sqlx::query_scalar!(
        r#"SELECT created_at FROM broadcasts WHERE environment_id = $1 AND id = $2"#,
        auth.environment_id,
        id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::from)?
    .filter(|created_at| *created_at >= auth.subscriber_created_at)
    .ok_or_else(|| ApiError::not_found("no such broadcast"))?;

    let changed = if broadcast_created_at > watermark {
        sqlx::query!(
            r#"DELETE FROM broadcast_archives
                WHERE environment_id = $1 AND subscriber_id = $2
                  AND broadcast_id = $3 AND archived = true"#,
            auth.environment_id,
            auth.subscriber_id,
            id,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?
        .rows_affected()
    } else {
        sqlx::query!(
            r#"INSERT INTO broadcast_archives
                   (environment_id, subscriber_id, broadcast_id, broadcast_created_at, archived)
               VALUES ($1, $2, $3, $4, false)
               ON CONFLICT (environment_id, subscriber_id, broadcast_id)
               DO UPDATE SET archived = false, updated_at = now()
               WHERE broadcast_archives.archived = true"#,
            auth.environment_id,
            auth.subscriber_id,
            id,
            broadcast_created_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?
        .rows_affected()
    };
    if changed > 0 {
        sqlx::query!(
            r#"UPDATE subscriber_counters SET updated_at = now()
                WHERE environment_id = $1 AND subscriber_id = $2"#,
            auth.environment_id,
            auth.subscriber_id,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        jobs::enqueue_hint(
            &mut tx,
            auth.environment_id,
            &[auth.subscriber_id],
            "archive_state",
            &[],
        )
        .await
        .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/v1/inbox/archive-all",
    tag = "subscriber",
    operation_id = "archiveAll",
    summary = "Archive all",
    description = r#"Archive every item, direct and broadcast, for this subscriber. Read state is untouched."#,
    responses(
        (status = 200, description = "Watermark moved.", body = crate::api::contract::InboxCounts),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn archive_all(
    State(state): State<AppState>,
    auth: SubscriberAuth,
) -> Result<Json<InboxCounts>, ApiError> {
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    sqlx::query!(
        r#"SELECT 1 AS one FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    // Archive-all is a one-row watermark upsert, never a bulk UPDATE over
    // inbox rows. clock_timestamp() under the lock for the same reason as
    // mark_all_read: an insert committing in the BEGIN->FOR UPDATE gap must
    // land at or below the watermark (archived and uncounted), never above a
    // stale one while the counter zeroes.
    let new_watermark = sqlx::query_scalar!(
        r#"UPDATE subscriber_counters SET
               archive_watermark = clock_timestamp(),
               unread_direct_count = 0,
               updated_at = clock_timestamp()
         WHERE environment_id = $1 AND subscriber_id = $2
         RETURNING archive_watermark"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    // Override GCs, both bounded by explicit-exception cardinality (partial
    // index for direct). Bind the installed watermark, never a fresh clock,
    // or an override above the watermark could be destroyed.
    sqlx::query!(
        r#"UPDATE notifications SET archived_at = NULL, unarchived_at = NULL
            WHERE environment_id = $1 AND subscriber_id = $2
              AND (archived_at IS NOT NULL OR unarchived_at IS NOT NULL)
              AND visible_at <= $3"#,
        auth.environment_id,
        auth.subscriber_id,
        new_watermark,
    )
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    sqlx::query!(
        r#"DELETE FROM broadcast_archives
            WHERE environment_id = $1 AND subscriber_id = $2
              AND broadcast_created_at <= $3"#,
        auth.environment_id,
        auth.subscriber_id,
        new_watermark,
    )
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    jobs::enqueue_hint(
        &mut tx,
        auth.environment_id,
        &[auth.subscriber_id],
        "archive_state",
        &[],
    )
    .await
    .map_err(ApiError::from)?;
    let counts = fetch_counts(&mut tx, &auth).await?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(Json(counts))
}

#[utoipa::path(
    post,
    path = "/v1/inbox/archive-read",
    tag = "subscriber",
    operation_id = "archiveRead",
    summary = "Archive read items",
    description = r#"Archive every currently read item, direct and broadcast. Runs asynchronously as a resumable job; completion is signaled by an SSE hint and ETag movement."#,
    responses(
        (status = 202, description = "Accepted. The job runs asynchronously."),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn archive_read(
    State(state): State<AppState>,
    auth: SubscriberAuth,
) -> Result<StatusCode, ApiError> {
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    // The lock orders as_of against concurrent watermark moves. as_of
    // freezes the item horizon: items arriving later are untouched.
    sqlx::query!(
        r#"SELECT 1 AS one FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    let as_of = sqlx::query_scalar!(r#"SELECT clock_timestamp() AS "now!""#)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::from)?;
    // Transactional outbox: the job commits with this transaction.
    jobs::enqueue_archive_read(&mut tx, auth.environment_id, auth.subscriber_id, as_of)
        .await
        .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::ACCEPTED)
}

/// Whether a deliver job still owns this notification (its counter bump has
/// not run). Shared guard for every per-item counter mutation.
async fn pending_deliver(
    tx: &mut sqlx::PgConnection,
    environment_id: Uuid,
    id: Uuid,
) -> Result<bool, ApiError> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM jobs j
               CROSS JOIN LATERAL jsonb_array_elements_text(
                   CASE WHEN jsonb_typeof(j.payload->'notification_ids') = 'array'
                        THEN j.payload->'notification_ids' END)
                   WITH ORDINALITY AS t(nid, idx)
               WHERE j.environment_id = $1 AND j.job_type = 'deliver'
                 AND t.nid = ($2::uuid)::text
                 AND (t.idx - 1) >= COALESCE((j.progress_cursor->>'offset')::bigint, 0)
           ) AS "pending!""#,
        environment_id,
        id,
    )
    .fetch_one(tx)
    .await
    .map_err(ApiError::from)
}

#[utoipa::path(
    post,
    path = "/v1/inbox/read-all",
    tag = "subscriber",
    operation_id = "markAllRead",
    summary = "Mark all read",
    description = r#"Mark every item, direct and broadcast, as read for this subscriber."#,
    responses(
        (status = 200, description = "Watermark moved.", body = crate::api::contract::InboxCounts),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn mark_all_read(
    State(state): State<AppState>,
    auth: SubscriberAuth,
) -> Result<Json<InboxCounts>, ApiError> {
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    // Lock first. The old watermark bounds the timeline job's window below.
    let old_watermark = sqlx::query_scalar!(
        r#"SELECT read_watermark FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    // Mark-all-read is a one-row watermark upsert, never a bulk UPDATE over
    // notification rows (MVCC bloat on the hottest write path).
    //
    // clock_timestamp() (not now()) is evaluated while this statement holds the
    // counters row lock taken above, so the watermark is the instant the move
    // actually takes effect, not the transaction's BEGIN. now() is pinned at
    // BEGIN, before the FOR UPDATE, so a direct insert or deliver bump that
    // committed its +1 in the BEGIN->FOR UPDATE gap would land above a
    // now()-watermark while unread_direct_count = 0 zeroed it. That is unread
    // in the list but uncounted, a permanent two-source drift. With
    // clock_timestamp() read under the lock, any such row's visible_at precedes
    // this instant, so it is covered by the watermark (read) not clobbered.
    let new_watermark = sqlx::query_scalar!(
        r#"UPDATE subscriber_counters SET
               read_watermark = clock_timestamp(),
               unread_direct_count = 0,
               updated_at = clock_timestamp()
         WHERE environment_id = $1 AND subscriber_id = $2
         RETURNING read_watermark"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    if new_watermark > old_watermark {
        // Timeline rows for the (old, new] window append asynchronously in
        // chunks. The request path stays O(1).
        jobs::enqueue_timeline(
            &mut tx,
            auth.environment_id,
            auth.subscriber_id,
            timeline::STATUS_READ,
            old_watermark,
            new_watermark,
        )
        .await
        .map_err(ApiError::from)?;
    }
    // GC exception rows at or below the new watermark. They are redundant.
    // Bind the watermark just installed, not now()/clock_timestamp() again. It
    // must match the watermark exactly so this never deletes an exception row
    // above the watermark, which would resurrect an individually-read broadcast
    // as unread.
    sqlx::query!(
        r#"DELETE FROM broadcast_reads
            WHERE environment_id = $1 AND subscriber_id = $2
              AND broadcast_created_at <= $3"#,
        auth.environment_id,
        auth.subscriber_id,
        new_watermark,
    )
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    // Direct unread overrides die too: the user just read everything. The
    // scan is bounded by explicit exceptions (partial index), the same
    // cardinality class as the broadcast GC above, so this is not a bulk
    // UPDATE over the inbox.
    sqlx::query!(
        r#"UPDATE notifications SET unread_at = NULL
            WHERE environment_id = $1 AND subscriber_id = $2
              AND unread_at IS NOT NULL AND visible_at <= $3"#,
        auth.environment_id,
        auth.subscriber_id,
        new_watermark,
    )
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    jobs::enqueue_hint(
        &mut tx,
        auth.environment_id,
        &[auth.subscriber_id],
        "read_state",
        &[],
    )
    .await
    .map_err(ApiError::from)?;
    let counts = fetch_counts(&mut tx, &auth).await?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(Json(counts))
}

#[utoipa::path(
    post,
    path = "/v1/inbox/seen-all",
    tag = "subscriber",
    operation_id = "markAllSeen",
    summary = "Mark all seen",
    description = "Clear the unseen count (the bell badge) without changing read state. Called when the inbox opens.",
    responses(
        (status = 200, description = "Watermark moved.", body = crate::api::contract::InboxCounts),
        (
            status = 401,
            description = "Missing/invalid API key or subscriber hash.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid subscriber hash"}}),
        ),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn mark_all_seen(
    State(state): State<AppState>,
    auth: SubscriberAuth,
) -> Result<Json<InboxCounts>, ApiError> {
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    let old_watermark = sqlx::query_scalar!(
        r#"SELECT seen_watermark FROM subscriber_counters
            WHERE environment_id = $1 AND subscriber_id = $2 FOR UPDATE"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    // Seen state is watermark-ONLY: no per-item seen, no exceptions table.
    // clock_timestamp() read under the lock taken above, not now() pinned at
    // BEGIN: the symmetric fix to mark_all_read. A create or deliver bump that
    // commits its unseen +1 in the BEGIN->FOR UPDATE gap would otherwise be
    // zeroed while sitting above a stale seen_watermark.
    let new_watermark = sqlx::query_scalar!(
        r#"UPDATE subscriber_counters SET
               seen_watermark = clock_timestamp(),
               unseen_direct_count = 0,
               updated_at = clock_timestamp()
         WHERE environment_id = $1 AND subscriber_id = $2
         RETURNING seen_watermark"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    if new_watermark > old_watermark {
        jobs::enqueue_timeline(
            &mut tx,
            auth.environment_id,
            auth.subscriber_id,
            timeline::STATUS_SEEN,
            old_watermark,
            new_watermark,
        )
        .await
        .map_err(ApiError::from)?;
    }
    jobs::enqueue_hint(
        &mut tx,
        auth.environment_id,
        &[auth.subscriber_id],
        "read_state",
        &[],
    )
    .await
    .map_err(ApiError::from)?;
    let counts = fetch_counts(&mut tx, &auth).await?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(Json(counts))
}

// =============================================================================
// Cursor + ETag plumbing
// =============================================================================

pub fn encode_cursor(ts: DateTime<Utc>, id: Uuid) -> String {
    URL_SAFE_NO_PAD.encode(format!("{}:{}", ts.timestamp_micros(), id.as_simple()))
}

pub fn decode_cursor(cursor: &str) -> Option<(DateTime<Utc>, Uuid)> {
    let raw = URL_SAFE_NO_PAD.decode(cursor).ok()?;
    let raw = String::from_utf8(raw).ok()?;
    let (micros, id) = raw.split_once(':')?;
    Some((
        DateTime::from_timestamp_micros(micros.parse().ok()?)?,
        id.parse().ok()?,
    ))
}

/// Strong validator over: request cursor, counters.updated_at (bumped by
/// EVERY read-state mutation), latest visible direct item, latest broadcast,
/// max(preferences.updated_at). Each input is one index-only lookup, and
/// anything that can change a page moves at least one of them. The "latest
/// direct" input is the latest VISIBLE item, so a scheduled notification
/// crossing `now()` moves it without any write.
async fn compute_etag(
    state: &AppState,
    auth: &SubscriberAuth,
    cursor: Option<&str>,
    limit: i64,
    filter: InboxFilter,
) -> Result<String, ApiError> {
    let row = sqlx::query!(
        r#"SELECT
               c.updated_at AS "counters_updated_at!",
               (SELECT max(p.updated_at) FROM preferences p
                 WHERE p.environment_id = $1 AND p.subscriber_id = $2) AS prefs_updated_at,
               (SELECT n.visible_at FROM notifications n
                 WHERE n.environment_id = $1 AND n.subscriber_id = $2
                   AND n.visible_at <= now()
                 ORDER BY n.visible_at DESC, n.id DESC LIMIT 1) AS latest_direct_at,
               (SELECT n.id FROM notifications n
                 WHERE n.environment_id = $1 AND n.subscriber_id = $2
                   AND n.visible_at <= now()
                 ORDER BY n.visible_at DESC, n.id DESC LIMIT 1) AS latest_direct_id,
               (SELECT b.created_at FROM broadcasts b
                 WHERE b.environment_id = $1
                 ORDER BY b.created_at DESC, b.id DESC LIMIT 1) AS latest_broadcast_at,
               (SELECT b.id FROM broadcasts b
                 WHERE b.environment_id = $1
                 ORDER BY b.created_at DESC, b.id DESC LIMIT 1) AS latest_broadcast_id
           FROM subscriber_counters c
           WHERE c.environment_id = $1 AND c.subscriber_id = $2"#,
        auth.environment_id,
        auth.subscriber_id,
    )
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let mut hasher = Sha256::new();
    hasher.update(b"chimely-inbox-v2|");
    hasher.update(cursor.unwrap_or("").as_bytes());
    hasher.update(
        format!(
            "|{limit}|{}|{}",
            filter.as_str(),
            row.counters_updated_at.timestamp_micros()
        )
        .as_bytes(),
    );
    hasher.update(
        format!(
            "|{:?}|{:?}/{:?}|{:?}/{:?}",
            row.prefs_updated_at.map(|t| t.timestamp_micros()),
            row.latest_direct_at.map(|t| t.timestamp_micros()),
            row.latest_direct_id,
            row.latest_broadcast_at.map(|t| t.timestamp_micros()),
            row.latest_broadcast_id,
        )
        .as_bytes(),
    );
    Ok(format!("\"{}\"", hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips() {
        let ts = Utc::now();
        let id = crate::ids::new_uuid();
        let enc = encode_cursor(ts, id);
        let (ts2, id2) = decode_cursor(&enc).unwrap();
        assert_eq!(ts2.timestamp_micros(), ts.timestamp_micros());
        assert_eq!(id2, id);
    }

    #[test]
    fn malformed_cursors_are_rejected() {
        assert!(decode_cursor("not-base64!").is_none());
        assert!(decode_cursor("").is_none());
        let bogus = URL_SAFE_NO_PAD.encode("hello world");
        assert!(decode_cursor(&bogus).is_none());
    }
}

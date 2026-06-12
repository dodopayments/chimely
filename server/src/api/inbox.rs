//! Subscriber plane: the merged two-source inbox.
//!
//! Implements the canonical queries from specs/schema.sql in their specified
//! shape: each source arm is independently keyset-limited (newest first,
//! `(occurred_at, id)` tuple keyset, id tiebreaker makes it total), then
//! merged; the unread count is the maintained direct counter plus a tiny
//! broadcasts range count minus the subscriber's own exception rows above the
//! watermark — O(1)-ish, never O(rows).

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
    /// Routes mark-read to the right endpoint; the SDK handles this.
    pub source: &'static str,
    pub category: String,
    pub payload: Value,
    /// The ordering timestamp — `visible_at` for direct, `created_at` for
    /// broadcast.
    pub occurred_at: String,
    /// Computed — per-item exception OR at-or-below the read watermark.
    pub read: bool,
}

#[derive(Debug, Serialize)]
pub struct InboxPage {
    pub items: Vec<InboxItem>,
    /// Opaque keyset token; null when the last page is reached.
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
}

#[utoipa::path(
    get,
    path = "/v1/inbox/items",
    tag = "subscriber",
    operation_id = "listInboxItems",
    summary = "Merged inbox list (direct + broadcast), keyset-paginated",
    description = r#"Newest first, ordered by `(occurred_at, id)` descending across both
sources. `cursor` is the opaque keyset token from the previous page's
`next_cursor`. Category mutes are applied server-side.

Supports `ETag` / `If-None-Match`, so post-reconnect refetches
(deploy thundering herd) are mostly 304s. The validator is a strong
hash over: the request cursor, `subscriber_counters.updated_at`
(bumped by EVERY read-state mutation — see the counter invariants in
schema.sql), the subscriber's latest direct item `(visible_at, id)`,
the environment's latest broadcast `(created_at, id)`, and
`max(preferences.updated_at)` for the subscriber. Each input is one
index-only lookup; anything that can change a page moves at least
one of them.

Responses are `Cache-Control: private, max-age=0` — inbox pages are
per-user data and must never be cached by shared proxies.
"#,
    params(
        ("cursor" = Option<String>, Query, description = "Opaque keyset cursor; omit for the first page."),
        ("limit" = Option<i32>, Query, minimum = 1, maximum = 100),
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
        // The frozen 3.0 spec (specs/openapi.yaml) under-declares this, so the
        // contract CI job sanctions-strips it before oasdiff (see ci.yml); the
        // generated/served spec and @dronte/client stay honest about it.
        (status = 400, description = "Malformed cursor or out-of-range limit.", body = crate::api::contract::Error),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
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
    let (cursor_ts, cursor_id) = match &params.cursor {
        None => (DateTime::<Utc>::MAX_UTC, Uuid::max()),
        Some(c) => decode_cursor(c).ok_or_else(|| ApiError::bad_request("malformed cursor"))?,
    };

    let etag = compute_etag(&state, &auth, params.cursor.as_deref(), limit).await?;
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

    // The canonical merged list query (specs/schema.sql header). Both arms
    // are keyset range scans over notifications_inbox_idx /
    // broadcasts_window_idx; category mutes are read-time NOT EXISTS probes.
    let rows = sqlx::query!(
        r#"SELECT merged.source AS "source!", merged.id AS "id!",
                  merged.category AS "category!", merged.payload AS "payload!",
                  merged.occurred_at AS "occurred_at!", merged.read AS "read!"
           FROM (
             (SELECT 'notification' AS source, n.id, n.category, n.payload,
                     n.visible_at AS occurred_at,
                     (n.read_at IS NOT NULL OR n.visible_at <= c.read_watermark) AS read
                FROM notifications n
                JOIN subscriber_counters c
                  ON c.environment_id = n.environment_id
                 AND c.subscriber_id  = n.subscriber_id
               WHERE n.environment_id = $1 AND n.subscriber_id = $2
                 AND n.visible_at <= now()
                 AND (n.visible_at, n.id) < ($3, $4)
                 AND NOT EXISTS (SELECT 1 FROM preferences p
                       WHERE p.environment_id = n.environment_id
                         AND p.subscriber_id  = n.subscriber_id
                         AND p.category = n.category AND p.channel = 'in_app'
                         AND p.enabled = false)
               ORDER BY n.visible_at DESC, n.id DESC LIMIT $5)
             UNION ALL
             (SELECT 'broadcast', b.id, b.category, b.payload, b.created_at,
                     (br.broadcast_id IS NOT NULL OR b.created_at <= c.read_watermark)
                FROM broadcasts b
                JOIN subscriber_counters c
                  ON c.environment_id = b.environment_id AND c.subscriber_id = $2
                LEFT JOIN broadcast_reads br
                  ON br.environment_id = b.environment_id
                 AND br.subscriber_id  = $2
                 AND br.broadcast_id   = b.id
               WHERE b.environment_id = $1
                 AND b.created_at >= $6
                 AND (b.created_at, b.id) < ($3, $4)
                 AND NOT EXISTS (SELECT 1 FROM preferences p
                       WHERE p.environment_id = b.environment_id
                         AND p.subscriber_id  = $2
                         AND p.category = b.category AND p.channel = 'in_app'
                         AND p.enabled = false)
               ORDER BY b.created_at DESC, b.id DESC LIMIT $5)
           ) merged
           ORDER BY occurred_at DESC, id DESC
           LIMIT $5"#,
        auth.environment_id,
        auth.subscriber_id,
        cursor_ts,
        cursor_id,
        limit,
        auth.subscriber_created_at,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let next_cursor = (rows.len() as i64 == limit)
        .then(|| rows.last().map(|r| encode_cursor(r.occurred_at, r.id)))
        .flatten();
    let items: Vec<InboxItem> = rows
        .into_iter()
        .map(|r| {
            let (source, prefix) = match r.source.as_str() {
                "notification" => ("notification", ids::NOTIFICATION),
                _ => ("broadcast", ids::BROADCAST),
            };
            InboxItem {
                id: ids::typeid(prefix, r.id),
                source,
                category: r.category,
                payload: r.payload,
                occurred_at: format_ts(r.occurred_at),
                read: r.read,
            }
        })
        .collect();

    Ok((response_headers, Json(InboxPage { items, next_cursor })).into_response())
}

#[utoipa::path(
    get,
    path = "/v1/inbox/counts",
    tag = "subscriber",
    operation_id = "getInboxCounts",
    summary = "Unread and unseen counts",
    description = r#"`unread` drives list styling; `unseen` drives the bell badge (cleared
by mark-all-seen when the inbox opens). Served from maintained counters
(Redis cache, Postgres authoritative) — O(1), not O(rows).
"#,
    responses(
        (status = 200, description = "Current counts.", body = crate::api::contract::InboxCounts),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
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

/// The unread/unseen count: maintained direct counters + a live broadcast
/// term. The broadcast term is evaluated EXACTLY as the list's broadcast arm
/// (visible, above the watermark, no read exception, not muted) so the count
/// agrees with the visible list at all times. It is a count over the
/// broadcasts table (rows = announcements, not subscribers), so the per-row
/// `NOT EXISTS` probes stay cheap. Seen has no exceptions term.
///
/// Category mutes ARE applied to the broadcast term here, unlike the
/// maintained direct counters: the broadcast term is recomputed on every read
/// (no stored counter to drift), so making it mute-aware is free and keeps the
/// two-source invariant ("list, count, read state agree") exact for broadcasts
/// instead of relying on a counter_rebuild that can never reconcile a live,
/// unstored term.
///
/// DESIGN NOTE, Redis count cache epoch (risk M2, required this phase even
/// though caching itself is deferred). When these computed totals are cached
/// in Redis, the key MUST be
/// `dronte:counts:{environment_id}:{epoch}:{subscriber_id}` where `epoch` is
/// an environment-level counter (`INCR dronte:counts-epoch:{env}` or its
/// Postgres-backed equivalent) bumped on every broadcast create. A broadcast
/// changes EVERY subscriber's unread count at once. Bumping the epoch
/// invalidates the whole environment's cached counts in O(1) without a key
/// scan, and the old-epoch keys age out via TTL. Per-subscriber mutations
/// (read/seen/deliver) invalidate just their own key. Redis stays the cache
/// plane: a lost epoch key only causes recomputes from Postgres, never a
/// stale count.
pub async fn fetch_counts(
    conn: &mut sqlx::PgConnection,
    auth: &SubscriberAuth,
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
                               AND br.broadcast_id   = b.id)
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
        auth.environment_id,
        auth.subscriber_id,
        auth.subscriber_created_at,
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
    summary = "Mark one direct notification read",
    description = "Idempotent. Sets `read_at`; decrements the unread counter only if it was unread.",
    params(("id" = crate::api::contract::NotificationId, Path)),
    responses(
        (status = 204, description = "Read (now or already)."),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
        (status = 404, description = "Resource not found in this environment.", body = crate::api::contract::Error),
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
    // Scheduled rows (visible_at > now()) are excluded from ALL subscriber
    // queries — an invisible item cannot be marked read.
    let row = sqlx::query!(
        r#"SELECT visible_at, read_at, category FROM notifications
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
            r#"UPDATE notifications SET read_at = now()
                WHERE environment_id = $1 AND id = $2 AND visible_at = $3"#,
            auth.environment_id,
            id,
            row.visible_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        // A visible row still owned by a pending deliver job is UNCOUNTED:
        // its counter increment has not happened yet, so decrementing now
        // would drift the counter by -1 forever (the deliver bump skips rows
        // with read_at set). The deliver job is the only counter for those
        // rows, and we hold the counters lock, so the job cannot complete
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
        // Decrement ONLY IF the row was unread above the watermark AND
        // already counted (otherwise double-decrement). The mute guard pairs
        // with the mute-aware increment: a muted row was never counted, so
        // marking it read must not steal a count from an unmuted item.
        // updated_at bump: EVERY read-state mutation is an ETag input.
        sqlx::query!(
            r#"UPDATE subscriber_counters c SET
                   unread_direct_count = greatest(0,
                       c.unread_direct_count - ($3 > c.read_watermark AND NOT $4
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
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        // The 'read' timeline row commits with the read_at flip; the guard
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
    path = "/v1/inbox/broadcasts/{id}/read",
    tag = "subscriber",
    operation_id = "markBroadcastRead",
    summary = "Mark one broadcast read (for this subscriber)",
    description = "Idempotent. Inserts a `broadcast_reads` exception row; a no-op if the broadcast is already below the read watermark.",
    params(("id" = crate::api::contract::BroadcastId, Path)),
    responses(
        (status = 204, description = "Read (now or already)."),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
        (status = 404, description = "Resource not found in this environment.", body = crate::api::contract::Error),
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

    // No-op if already below the read watermark (the watermark covers it;
    // an exception row would be GC fodder).
    if broadcast_created_at > watermark {
        let inserted = sqlx::query!(
            r#"INSERT INTO broadcast_reads
                   (environment_id, subscriber_id, broadcast_id, broadcast_created_at)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (environment_id, subscriber_id, broadcast_id) DO NOTHING"#,
            auth.environment_id,
            auth.subscriber_id,
            id,
            broadcast_created_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?
        .rows_affected();
        if inserted > 0 {
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
    }
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/v1/inbox/read-all",
    tag = "subscriber",
    operation_id = "markAllRead",
    summary = "Mark everything read (watermark move)",
    description = r#"Moves the per-subscriber read watermark to now — a one-row update, not
a bulk UPDATE. Covers both sources; individually-read exception rows
below the new watermark are garbage-collected.
"#,
    responses(
        (status = 200, description = "Watermark moved.", body = crate::api::contract::InboxCounts),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn mark_all_read(
    State(state): State<AppState>,
    auth: SubscriberAuth,
) -> Result<Json<InboxCounts>, ApiError> {
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    // Lock first; the old watermark bounds the timeline job's window below.
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
    // clock_timestamp() (NOT now()) is evaluated while this statement holds the
    // counters row lock taken above, so the watermark is the instant the move
    // actually takes effect, not the transaction's BEGIN. now() is pinned at
    // BEGIN, before the FOR UPDATE, so a direct insert or deliver bump that
    // committed its +1 in the BEGIN->FOR UPDATE gap would land ABOVE a
    // now()-watermark while unread_direct_count = 0 zeroed it — unread in the
    // list but uncounted, permanent two-source drift. With clock_timestamp()
    // read under the lock, any such row's visible_at precedes this instant, so
    // it is correctly covered by the watermark (read) instead of clobbered.
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
        // chunks; the request path stays O(1).
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
    // GC exception rows at or below the new watermark — they are redundant.
    // Bind the watermark we just installed (not now()/clock_timestamp() again):
    // it must match the watermark exactly so this never deletes an exception
    // row ABOVE the watermark, which would resurrect an individually-read
    // broadcast as unread.
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
    summary = "Mark everything seen (badge clear; watermark move)",
    description = "Called by the SDK when the inbox opens. Moves the seen watermark; read state is untouched.",
    responses(
        (status = 200, description = "Watermark moved.", body = crate::api::contract::InboxCounts),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
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
    hasher.update(b"dronte-inbox-v1|");
    hasher.update(cursor.unwrap_or("").as_bytes());
    hasher.update(format!("|{limit}|{}", row.counters_updated_at.timestamp_micros()).as_bytes());
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

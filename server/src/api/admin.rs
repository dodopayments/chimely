//! Admin plane: the embedded `/admin` dashboard's API (specs/phase-4-admin.md).
//!
//! Single-org by construction: no organizations, no admin users, no roles.
//! One static credential gates the whole plane (see `auth::AdminAuth`).
//! Every query is either scoped to one `environment_id` or is an explicit,
//! documented cross-environment admin path (the DLQ browser) — the schema's
//! shard-readiness invariant #3 exception.
//!
//! Admin reads REUSE the canonical queries (`inbox::list_items_for`,
//! `inbox::fetch_counts_for`) and admin writes REUSE the canonical write-path
//! (`management::create_broadcast_idempotent`, `dlq::replay`). A second
//! implementation of the two-source merge or the broadcast write would be a
//! bug by definition — two implementations WILL disagree.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::api::format_ts;
use crate::api::inbox::{self, InboxCounts};
use crate::api::management;
use crate::auth::AdminAuth;
use crate::error::ApiError;
use crate::extract::{ApiJson, ApiQuery};
use crate::state::AppState;
use crate::{dlq, ids};

const ADMIN_NOTIFICATION_PAGE: i64 = 50;
const ADMIN_NOTIFICATION_MAX_PAGE: i64 = 200;
const ADMIN_INBOX_PREVIEW: i64 = 20;
/// Vite emits content-hashed asset filenames, so the bundle is immutable.
const ASSET_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

// =============================================================================
// Embedded SPA (rust-embed). The single-binary deploy story: `docker run`
// ships the dashboard, no CDN and no separate static host.
// =============================================================================

#[derive(rust_embed::RustEmbed)]
#[folder = "admin/dist"]
struct AdminAssets;

/// Serve the embedded SPA. Gated by `AdminAuth` like every admin route, so a
/// bare `/admin` navigation 401s with `WWW-Authenticate: Basic` and the
/// browser prompts; thereafter it attaches the credential to every asset and
/// API request on this origin. Unknown non-file paths fall back to
/// `index.html` so client-side routing (TanStack Router) works on refresh.
pub async fn serve_spa(_auth: AdminAuth, uri: Uri) -> Response {
    let path = uri
        .path()
        .trim_start_matches("/admin")
        .trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = AdminAssets::get(path) {
        let cache = if path.starts_with("assets/") {
            ASSET_CACHE_CONTROL
        } else {
            "no-cache"
        };
        return (
            [
                (CONTENT_TYPE, file.metadata.mimetype()),
                (CACHE_CONTROL, cache),
            ],
            file.data.into_owned(),
        )
            .into_response();
    }

    // A path that looks like a file but is missing is a real 404; anything
    // else is a client-side route and gets the SPA shell.
    if path.contains('.') {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    match AdminAssets::get("index.html") {
        Some(index) => (
            [
                (CONTENT_TYPE, "text/html; charset=utf-8"),
                (CACHE_CONTROL, "no-cache"),
            ],
            index.data.into_owned(),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "admin UI not built (run `pnpm --filter dronte-admin build`)",
        )
            .into_response(),
    }
}

// =============================================================================
// Environments
// =============================================================================

#[derive(Serialize, ToSchema)]
pub struct AdminEnvironment {
    /// TypeID, `env_…`.
    pub id: String,
    pub slug: String,
    pub name: String,
    pub require_subscriber_hash: bool,
    pub created_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct AdminEnvironmentDetail {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub require_subscriber_hash: bool,
    /// The dedicated subscriber HMAC secret. Plaintext by design (the
    /// customer backend computes hashes with it) and operator-only.
    pub subscriber_hmac_secret: String,
    /// True while a rotation overlap is open (the previous secret still
    /// verifies live `<Inbox />` sessions).
    pub has_previous_secret: bool,
    pub subscriber_hmac_rotated_at: Option<String>,
    pub created_at: String,
}

#[derive(Deserialize, ToSchema)]
pub struct AdminCreateEnvironmentRequest {
    pub slug: String,
    pub name: String,
    /// Production default true; set false for a dev/quickstart environment.
    #[serde(default = "default_true")]
    pub require_subscriber_hash: bool,
}

fn default_true() -> bool {
    true
}

#[utoipa::path(
    get,
    path = "/admin/api/environments",
    tag = "admin",
    operation_id = "adminListEnvironments",
    summary = "List environments",
    responses((status = 200, description = "Environments.", body = Vec<AdminEnvironment>),
              (status = 401, description = "Admin authentication required.")),
    security(("AdminToken" = []))
)]
pub async fn list_environments(
    _auth: AdminAuth,
    State(state): State<AppState>,
) -> Result<Json<Vec<AdminEnvironment>>, ApiError> {
    let rows = sqlx::query!(
        r#"SELECT id, slug, name, require_subscriber_hash, created_at
             FROM environments ORDER BY created_at"#,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(
        rows.into_iter()
            .map(|r| AdminEnvironment {
                id: ids::typeid(ids::ENVIRONMENT, r.id),
                slug: r.slug,
                name: r.name,
                require_subscriber_hash: r.require_subscriber_hash,
                created_at: format_ts(r.created_at),
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/api/environments",
    tag = "admin",
    operation_id = "adminCreateEnvironment",
    summary = "Create an environment",
    request_body = AdminCreateEnvironmentRequest,
    responses((status = 201, description = "Created.", body = AdminEnvironmentDetail),
              (status = 400, description = "Validation error or slug already exists.", body = crate::api::contract::Error),
              (status = 401, description = "Admin authentication required.")),
    security(("AdminToken" = []))
)]
pub async fn create_environment(
    _auth: AdminAuth,
    State(state): State<AppState>,
    ApiJson(req): ApiJson<AdminCreateEnvironmentRequest>,
) -> Result<Response, ApiError> {
    let slug = req.slug.trim();
    if slug.is_empty() || slug.len() > 255 {
        return Err(ApiError::bad_request("slug must be 1-255 characters"));
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ApiError::bad_request(
            "slug may contain only letters, digits, '-' and '_'",
        ));
    }
    let name = req.name.trim();
    if name.is_empty() || name.len() > 255 {
        return Err(ApiError::bad_request("name must be 1-255 characters"));
    }

    let id = ids::new_uuid();
    let hmac_secret = format!("shmac_{}", ids::new_uuid().as_simple());
    let created = sqlx::query!(
        r#"INSERT INTO environments
               (id, slug, name, subscriber_hmac_secret, require_subscriber_hash)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (slug) DO NOTHING
           RETURNING created_at"#,
        id,
        slug,
        name,
        hmac_secret,
        req.require_subscriber_hash,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let Some(created) = created else {
        return Err(ApiError::bad_request("environment slug already exists"));
    };

    Ok((
        StatusCode::CREATED,
        Json(AdminEnvironmentDetail {
            id: ids::typeid(ids::ENVIRONMENT, id),
            slug: slug.to_owned(),
            name: name.to_owned(),
            require_subscriber_hash: req.require_subscriber_hash,
            subscriber_hmac_secret: hmac_secret,
            has_previous_secret: false,
            subscriber_hmac_rotated_at: None,
            created_at: format_ts(created.created_at),
        }),
    )
        .into_response())
}

#[utoipa::path(
    get,
    path = "/admin/api/environments/{env_id}",
    tag = "admin",
    operation_id = "adminGetEnvironment",
    summary = "Environment detail (includes the subscriber HMAC secret)",
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    responses((status = 200, description = "Environment.", body = AdminEnvironmentDetail),
              (status = 401, description = "Admin authentication required."),
              (status = 404, description = "No such environment.", body = crate::api::contract::Error)),
    security(("AdminToken" = []))
)]
pub async fn get_environment(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
) -> Result<Json<AdminEnvironmentDetail>, ApiError> {
    let env = parse_env_id(&env_id)?;
    let row = sqlx::query!(
        r#"SELECT id, slug, name, require_subscriber_hash, subscriber_hmac_secret,
                  subscriber_hmac_secret_previous, subscriber_hmac_rotated_at, created_at
             FROM environments WHERE id = $1"#,
        env,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no such environment"))?;

    Ok(Json(AdminEnvironmentDetail {
        id: ids::typeid(ids::ENVIRONMENT, row.id),
        slug: row.slug,
        name: row.name,
        require_subscriber_hash: row.require_subscriber_hash,
        subscriber_hmac_secret: row.subscriber_hmac_secret,
        has_previous_secret: row.subscriber_hmac_secret_previous.is_some(),
        subscriber_hmac_rotated_at: row.subscriber_hmac_rotated_at.map(format_ts),
        created_at: format_ts(row.created_at),
    }))
}

// =============================================================================
// Subscriber HMAC rotation (two-slot overlap)
// =============================================================================

#[derive(Serialize, ToSchema)]
pub struct AdminHmacRotation {
    /// The new current secret. Update the customer backend with it; the
    /// previous secret keeps verifying until the rotation is completed.
    pub subscriber_hmac_secret: String,
    pub has_previous_secret: bool,
    pub subscriber_hmac_rotated_at: Option<String>,
}

#[utoipa::path(
    post,
    path = "/admin/api/environments/{env_id}/hmac/rotate",
    tag = "admin",
    operation_id = "adminRotateHmac",
    summary = "Begin a subscriber-HMAC rotation (current → previous)",
    description = r#"Generates a new current secret and moves the existing one into the
previous slot. During the overlap BOTH secrets verify live `<Inbox />`
sessions (auth checks current then previous), so rotation is
zero-downtime. Complete the rotation to clear the previous slot once
every customer backend has switched.
"#,
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    responses((status = 200, description = "Rotated.", body = AdminHmacRotation),
              (status = 401, description = "Admin authentication required."),
              (status = 404, description = "No such environment.", body = crate::api::contract::Error)),
    security(("AdminToken" = []))
)]
pub async fn rotate_hmac(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
) -> Result<Json<AdminHmacRotation>, ApiError> {
    let env = parse_env_id(&env_id)?;
    let new_secret = format!("shmac_{}", ids::new_uuid().as_simple());
    let row = sqlx::query!(
        r#"UPDATE environments SET
               subscriber_hmac_secret = $2,
               subscriber_hmac_secret_previous = subscriber_hmac_secret,
               subscriber_hmac_rotated_at = now(),
               updated_at = now()
           WHERE id = $1
           RETURNING subscriber_hmac_secret, subscriber_hmac_secret_previous,
                     subscriber_hmac_rotated_at"#,
        env,
        new_secret,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no such environment"))?;

    Ok(Json(AdminHmacRotation {
        subscriber_hmac_secret: row.subscriber_hmac_secret,
        has_previous_secret: row.subscriber_hmac_secret_previous.is_some(),
        subscriber_hmac_rotated_at: row.subscriber_hmac_rotated_at.map(format_ts),
    }))
}

#[utoipa::path(
    post,
    path = "/admin/api/environments/{env_id}/hmac/rotate/complete",
    tag = "admin",
    operation_id = "adminCompleteHmacRotation",
    summary = "Complete a rotation (clear the previous secret slot)",
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    responses((status = 204, description = "Previous secret cleared."),
              (status = 401, description = "Admin authentication required."),
              (status = 404, description = "No such environment.", body = crate::api::contract::Error)),
    security(("AdminToken" = []))
)]
pub async fn complete_hmac_rotation(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let env = parse_env_id(&env_id)?;
    let affected = sqlx::query!(
        r#"UPDATE environments SET subscriber_hmac_secret_previous = NULL, updated_at = now()
           WHERE id = $1"#,
        env,
    )
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?
    .rows_affected();
    if affected == 0 {
        return Err(ApiError::not_found("no such environment"));
    }
    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// API keys
// =============================================================================

#[derive(Serialize, ToSchema)]
pub struct AdminApiKey {
    /// TypeID, `key_…`.
    pub id: String,
    pub name: String,
    /// Display prefix for recognition; the full key is never retrievable.
    pub key_prefix: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AdminApiKeyCreated {
    pub id: String,
    pub name: String,
    pub key_prefix: String,
    /// The plaintext key, shown EXACTLY once. Only the sha256 hash is stored.
    pub key: String,
    pub created_at: String,
}

#[derive(Deserialize, ToSchema)]
pub struct AdminCreateApiKeyRequest {
    pub name: String,
}

#[utoipa::path(
    get,
    path = "/admin/api/environments/{env_id}/api-keys",
    tag = "admin",
    operation_id = "adminListApiKeys",
    summary = "List API keys for an environment (prefix only, never the key)",
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    responses((status = 200, description = "Keys.", body = Vec<AdminApiKey>),
              (status = 401, description = "Admin authentication required."),
              (status = 404, description = "No such environment.", body = crate::api::contract::Error)),
    security(("AdminToken" = []))
)]
pub async fn list_api_keys(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
) -> Result<Json<Vec<AdminApiKey>>, ApiError> {
    let env = parse_env_id(&env_id)?;
    ensure_environment(&state, env).await?;
    let rows = sqlx::query!(
        r#"SELECT id, name, key_prefix, created_at, last_used_at, revoked_at
             FROM api_keys WHERE environment_id = $1 ORDER BY created_at"#,
        env,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(
        rows.into_iter()
            .map(|r| AdminApiKey {
                id: ids::typeid(ids::API_KEY, r.id),
                name: r.name,
                key_prefix: r.key_prefix,
                created_at: format_ts(r.created_at),
                last_used_at: r.last_used_at.map(format_ts),
                revoked_at: r.revoked_at.map(format_ts),
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/api/environments/{env_id}/api-keys",
    tag = "admin",
    operation_id = "adminCreateApiKey",
    summary = "Create an API key (plaintext returned once)",
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    request_body = AdminCreateApiKeyRequest,
    responses((status = 201, description = "Created; `key` is shown once.", body = AdminApiKeyCreated),
              (status = 400, description = "Validation error.", body = crate::api::contract::Error),
              (status = 401, description = "Admin authentication required."),
              (status = 404, description = "No such environment.", body = crate::api::contract::Error)),
    security(("AdminToken" = []))
)]
pub async fn create_api_key(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
    ApiJson(req): ApiJson<AdminCreateApiKeyRequest>,
) -> Result<Response, ApiError> {
    let env = parse_env_id(&env_id)?;
    ensure_environment(&state, env).await?;
    let name = req.name.trim();
    if name.is_empty() || name.len() > 255 {
        return Err(ApiError::bad_request("name must be 1-255 characters"));
    }

    let id = ids::new_uuid();
    // 256 bits of randomness in the key body; the prefix is for display only.
    let key = format!("drnt_live_{}", ids::new_uuid().as_simple());
    let key_hash: Vec<u8> = Sha256::digest(key.as_bytes()).to_vec();
    let key_prefix = &key[..key.floor_char_boundary(14)];

    let created = sqlx::query!(
        r#"INSERT INTO api_keys (environment_id, id, name, key_hash, key_prefix)
           VALUES ($1, $2, $3, $4, $5)
           RETURNING created_at"#,
        env,
        id,
        name,
        key_hash,
        key_prefix,
    )
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(AdminApiKeyCreated {
            id: ids::typeid(ids::API_KEY, id),
            name: name.to_owned(),
            key_prefix: key_prefix.to_owned(),
            key,
            created_at: format_ts(created.created_at),
        }),
    )
        .into_response())
}

#[utoipa::path(
    post,
    path = "/admin/api/environments/{env_id}/api-keys/{key_id}/revoke",
    tag = "admin",
    operation_id = "adminRevokeApiKey",
    summary = "Revoke an API key (soft; row kept for audit)",
    params(
        ("env_id" = String, Path, description = "Environment TypeID (env_…)."),
        ("key_id" = String, Path, description = "API key TypeID (key_…).")
    ),
    responses((status = 204, description = "Revoked (now or already)."),
              (status = 401, description = "Admin authentication required."),
              (status = 404, description = "No such API key.", body = crate::api::contract::Error)),
    security(("AdminToken" = []))
)]
pub async fn revoke_api_key(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Path((env_id, key_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let env = parse_env_id(&env_id)?;
    let key = ids::parse_typeid(ids::API_KEY, &key_id)
        .ok_or_else(|| ApiError::not_found("no such API key"))?;
    // Soft revoke: keep the row for audit, set revoked_at if not already set.
    let affected = sqlx::query!(
        r#"UPDATE api_keys SET revoked_at = COALESCE(revoked_at, now())
           WHERE environment_id = $1 AND id = $2"#,
        env,
        key,
    )
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?
    .rows_affected();
    if affected == 0 {
        return Err(ApiError::not_found("no such API key"));
    }
    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Notification / status-timeline browser
// =============================================================================

#[derive(Deserialize, IntoParams)]
pub struct AdminNotificationFilter {
    /// Customer-provided subscriber id.
    pub subscriber_id: Option<String>,
    pub category: Option<String>,
    /// Lower bound on `visible_at` (inclusive), RFC 3339.
    pub after: Option<DateTime<Utc>>,
    /// Upper bound on `visible_at` (exclusive), RFC 3339.
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    /// Opaque keyset cursor from the previous page's `next_cursor`.
    pub cursor: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AdminNotification {
    /// TypeID, `notif_…`.
    pub id: String,
    pub subscriber_id: String,
    pub category: String,
    pub payload: Value,
    pub created_at: String,
    pub deliver_at: Option<String>,
    pub visible_at: String,
    pub read_at: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AdminNotificationPage {
    pub items: Vec<AdminNotification>,
    pub next_cursor: Option<String>,
}

#[utoipa::path(
    get,
    path = "/admin/api/environments/{env_id}/notifications",
    tag = "admin",
    operation_id = "adminListNotifications",
    summary = "Browse direct notifications (filter by subscriber, category, time)",
    params(
        ("env_id" = String, Path, description = "Environment TypeID (env_…)."),
        AdminNotificationFilter
    ),
    responses((status = 200, description = "A page of notifications.", body = AdminNotificationPage),
              (status = 400, description = "Malformed cursor or out-of-range limit.", body = crate::api::contract::Error),
              (status = 401, description = "Admin authentication required."),
              (status = 404, description = "No such environment.", body = crate::api::contract::Error)),
    security(("AdminToken" = []))
)]
pub async fn list_notifications(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
    ApiQuery(filter): ApiQuery<AdminNotificationFilter>,
) -> Result<Json<AdminNotificationPage>, ApiError> {
    let env = parse_env_id(&env_id)?;
    ensure_environment(&state, env).await?;
    let limit = filter.limit.unwrap_or(ADMIN_NOTIFICATION_PAGE);
    if !(1..=ADMIN_NOTIFICATION_MAX_PAGE).contains(&limit) {
        return Err(ApiError::bad_request("limit must be between 1 and 200"));
    }
    let (cursor_ts, cursor_id) = match &filter.cursor {
        None => (DateTime::<Utc>::MAX_UTC, Uuid::max()),
        Some(c) => {
            inbox::decode_cursor(c).ok_or_else(|| ApiError::bad_request("malformed cursor"))?
        }
    };

    // Cross-partition, env-scoped admin scan (not a hot path). Keyset on
    // (visible_at, id) descending, mirroring the inbox ordering spine.
    let rows = sqlx::query!(
        r#"SELECT n.id, s.subscriber_id AS "subscriber_id!", n.category, n.payload,
                  n.created_at, n.deliver_at, n.visible_at, n.read_at
             FROM notifications n
             JOIN subscribers s ON s.environment_id = n.environment_id
                               AND s.id = n.subscriber_id
            WHERE n.environment_id = $1
              AND ($2::text IS NULL OR s.subscriber_id = $2)
              AND ($3::text IS NULL OR n.category = $3)
              AND ($4::timestamptz IS NULL OR n.visible_at >= $4)
              AND ($5::timestamptz IS NULL OR n.visible_at < $5)
              AND (n.visible_at, n.id) < ($6, $7)
            ORDER BY n.visible_at DESC, n.id DESC
            LIMIT $8"#,
        env,
        filter.subscriber_id.as_deref(),
        filter.category.as_deref(),
        filter.after,
        filter.before,
        cursor_ts,
        cursor_id,
        limit,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let next_cursor = (rows.len() as i64 == limit)
        .then(|| {
            rows.last()
                .map(|r| inbox::encode_cursor(r.visible_at, r.id))
        })
        .flatten();
    let items = rows
        .into_iter()
        .map(|r| AdminNotification {
            id: ids::typeid(ids::NOTIFICATION, r.id),
            subscriber_id: r.subscriber_id,
            category: r.category,
            payload: r.payload,
            created_at: format_ts(r.created_at),
            deliver_at: r.deliver_at.map(format_ts),
            visible_at: format_ts(r.visible_at),
            read_at: r.read_at.map(format_ts),
        })
        .collect();

    Ok(Json(AdminNotificationPage { items, next_cursor }))
}

#[derive(Serialize, ToSchema)]
pub struct AdminTimelineEntry {
    pub status: String,
    pub occurred_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct AdminNotificationTimeline {
    pub id: String,
    pub subscriber_id: String,
    pub timeline: Vec<AdminTimelineEntry>,
}

#[utoipa::path(
    get,
    path = "/admin/api/environments/{env_id}/notifications/{notif_id}/timeline",
    tag = "admin",
    operation_id = "adminNotificationTimeline",
    summary = "Status timeline for one notification (the \"did it send?\" answer)",
    params(
        ("env_id" = String, Path, description = "Environment TypeID (env_…)."),
        ("notif_id" = String, Path, description = "Notification TypeID (notif_…).")
    ),
    responses((status = 200, description = "The timeline so far.", body = AdminNotificationTimeline),
              (status = 401, description = "Admin authentication required."),
              (status = 404, description = "No such notification.", body = crate::api::contract::Error)),
    security(("AdminToken" = []))
)]
pub async fn notification_timeline(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Path((env_id, notif_id)): Path<(String, String)>,
) -> Result<Json<AdminNotificationTimeline>, ApiError> {
    let env = parse_env_id(&env_id)?;
    let id = ids::parse_typeid(ids::NOTIFICATION, &notif_id)
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

    // Same read as the management-plane timeline: min() per status is a
    // defensive read-time dedupe (writers guarantee one row per status).
    let rows = sqlx::query!(
        r#"SELECT status, min(occurred_at) AS "occurred_at!"
             FROM notification_status_log
            WHERE environment_id = $1 AND notification_id = $2
            GROUP BY status ORDER BY 2, 1"#,
        env,
        id,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(AdminNotificationTimeline {
        id: ids::typeid(ids::NOTIFICATION, id),
        subscriber_id: subscriber,
        timeline: rows
            .into_iter()
            .map(|r| AdminTimelineEntry {
                status: r.status,
                occurred_at: format_ts(r.occurred_at),
            })
            .collect(),
    }))
}

// =============================================================================
// Broadcast composer (reuses the canonical write-path)
// =============================================================================

#[derive(Deserialize, ToSchema)]
pub struct AdminCreateBroadcastRequest {
    pub category: String,
    pub payload: Option<Value>,
    pub idempotency_key: Option<String>,
}

#[utoipa::path(
    post,
    path = "/admin/api/environments/{env_id}/broadcasts",
    tag = "admin",
    operation_id = "adminCreateBroadcast",
    summary = "Compose a broadcast (one row, fan-out on read)",
    description = "Composing is creating: the same idempotent management-plane write-path. One row per announcement, never materialized per subscriber.",
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    request_body = AdminCreateBroadcastRequest,
    responses((status = 201, description = "Created.", body = crate::api::contract::Broadcast),
              (status = 200, description = "Idempotent replay.", body = crate::api::contract::Broadcast),
              (status = 400, description = "Validation error.", body = crate::api::contract::Error),
              (status = 401, description = "Admin authentication required."),
              (status = 404, description = "No such environment.", body = crate::api::contract::Error)),
    security(("AdminToken" = []))
)]
pub async fn create_broadcast(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
    ApiJson(req): ApiJson<AdminCreateBroadcastRequest>,
) -> Result<Response, ApiError> {
    let env = parse_env_id(&env_id)?;
    ensure_environment(&state, env).await?;
    let category = management::validate_category(&req.category)?.to_owned();
    let payload = management::validate_payload(req.payload)?;
    let key = management::validate_idempotency_key(req.idempotency_key)?;
    let (status, snapshot) =
        management::create_broadcast_idempotent(&state, env, category, payload, key).await?;
    Ok((status, Json(snapshot)).into_response())
}

// =============================================================================
// Subscriber lookup (reuses the canonical merge + count queries)
// =============================================================================

#[derive(Serialize, ToSchema)]
pub struct AdminPreference {
    pub category: String,
    pub channel: String,
    pub enabled: bool,
}

#[derive(Serialize, ToSchema)]
pub struct AdminInboxItem {
    pub id: String,
    pub source: String,
    pub category: String,
    pub payload: Value,
    pub occurred_at: String,
    pub read: bool,
}

#[derive(Serialize, ToSchema)]
pub struct AdminSubscriberView {
    pub subscriber_id: String,
    /// Governs broadcast visibility (`broadcast.created_at >= this`).
    pub created_at: String,
    pub counters: AdminCounts,
    pub read_watermark: String,
    pub seen_watermark: String,
    pub preferences: Vec<AdminPreference>,
    /// The subscriber's recent merged inbox, from the SAME canonical query
    /// the subscriber plane serves.
    pub inbox: Vec<AdminInboxItem>,
}

#[derive(Serialize, ToSchema)]
pub struct AdminCounts {
    pub unread: i32,
    pub unseen: i32,
}

impl From<InboxCounts> for AdminCounts {
    fn from(c: InboxCounts) -> Self {
        Self {
            unread: c.unread,
            unseen: c.unseen,
        }
    }
}

#[utoipa::path(
    get,
    path = "/admin/api/environments/{env_id}/subscribers/{subscriber_id}",
    tag = "admin",
    operation_id = "adminGetSubscriber",
    summary = "Subscriber lookup: counters, watermarks, preferences, merged inbox",
    params(
        ("env_id" = String, Path, description = "Environment TypeID (env_…)."),
        ("subscriber_id" = String, Path, description = "Customer-provided subscriber id.")
    ),
    responses((status = 200, description = "Subscriber view.", body = AdminSubscriberView),
              (status = 401, description = "Admin authentication required."),
              (status = 404, description = "No such subscriber.", body = crate::api::contract::Error)),
    security(("AdminToken" = []))
)]
pub async fn get_subscriber(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Path((env_id, subscriber_id)): Path<(String, String)>,
) -> Result<Json<AdminSubscriberView>, ApiError> {
    let env = parse_env_id(&env_id)?;
    let identity = sqlx::query!(
        r#"SELECT s.id, s.created_at,
                  c.read_watermark AS "read_watermark!",
                  c.seen_watermark AS "seen_watermark!"
             FROM subscribers s
             JOIN subscriber_counters c
               ON c.environment_id = s.environment_id AND c.subscriber_id = s.id
            WHERE s.environment_id = $1 AND s.subscriber_id = $2"#,
        env,
        subscriber_id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no such subscriber"))?;

    let mut conn = state.pool.acquire().await.map_err(ApiError::from)?;
    let counts = inbox::fetch_counts_for(&mut conn, env, identity.id, identity.created_at).await?;
    let inbox_rows = inbox::list_items_for(
        &mut *conn,
        env,
        identity.id,
        identity.created_at,
        DateTime::<Utc>::MAX_UTC,
        Uuid::max(),
        ADMIN_INBOX_PREVIEW,
    )
    .await
    .map_err(ApiError::from)?;
    drop(conn);

    let preferences = sqlx::query!(
        r#"SELECT category, channel, enabled FROM preferences
            WHERE environment_id = $1 AND subscriber_id = $2
            ORDER BY category, channel"#,
        env,
        identity.id,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(AdminSubscriberView {
        subscriber_id,
        created_at: format_ts(identity.created_at),
        counters: counts.into(),
        read_watermark: format_ts(identity.read_watermark),
        seen_watermark: format_ts(identity.seen_watermark),
        preferences: preferences
            .into_iter()
            .map(|p| AdminPreference {
                category: p.category,
                channel: p.channel,
                enabled: p.enabled,
            })
            .collect(),
        inbox: inbox_rows
            .into_iter()
            .map(|r| AdminInboxItem {
                id: ids::typeid(
                    if r.source == "notification" {
                        ids::NOTIFICATION
                    } else {
                        ids::BROADCAST
                    },
                    r.id,
                ),
                source: r.source.to_owned(),
                category: r.category,
                payload: r.payload,
                occurred_at: format_ts(r.occurred_at),
                read: r.read,
            })
            .collect(),
    }))
}

// =============================================================================
// DLQ browser + replay (the documented cross-environment admin path)
// =============================================================================

#[derive(Serialize, ToSchema)]
pub struct AdminDeadLetter {
    /// TypeID, `job_…` (stable across park/replay).
    pub id: String,
    pub environment_slug: String,
    pub job_type: String,
    pub attempts: i32,
    pub last_error: String,
    pub parked_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct AdminReplayResult {
    /// Number of parked jobs moved back into the claim path.
    pub replayed: i64,
}

#[utoipa::path(
    get,
    path = "/admin/api/dlq",
    tag = "admin",
    operation_id = "adminListDeadLetters",
    summary = "List parked jobs across environments",
    responses((status = 200, description = "Parked jobs.", body = Vec<AdminDeadLetter>),
              (status = 401, description = "Admin authentication required.")),
    security(("AdminToken" = []))
)]
pub async fn list_dlq(
    _auth: AdminAuth,
    State(state): State<AppState>,
) -> Result<Json<Vec<AdminDeadLetter>>, ApiError> {
    let letters = dlq::list(&state.pool).await.map_err(ApiError::from)?;
    Ok(Json(
        letters
            .into_iter()
            .map(|l| AdminDeadLetter {
                id: l.typeid(),
                environment_slug: l.environment_slug,
                job_type: l.job_type,
                attempts: l.attempts,
                last_error: l.last_error,
                parked_at: format_ts(l.parked_at),
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/api/dlq/{job_id}/replay",
    tag = "admin",
    operation_id = "adminReplayDeadLetter",
    summary = "Replay one parked job (re-enters the normal claim path)",
    description = "Moves the parked row back into `jobs` with a fresh attempt budget; the normal worker loop (SKIP LOCKED, per-environment fairness, delete-on-completion) runs it. Never executed inline.",
    params(("job_id" = String, Path, description = "Job TypeID (job_…).")),
    responses((status = 200, description = "Replayed.", body = AdminReplayResult),
              (status = 401, description = "Admin authentication required."),
              (status = 404, description = "No such parked job.", body = crate::api::contract::Error)),
    security(("AdminToken" = []))
)]
pub async fn replay_dead_letter(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<AdminReplayResult>, ApiError> {
    let id = ids::parse_typeid(ids::JOB, &job_id)
        .ok_or_else(|| ApiError::not_found("no such parked job"))?;
    // Cross-environment admin path: job ids are globally unique UUIDv7s.
    let replayed = dlq::replay(&state.pool, id, None)
        .await
        .map_err(ApiError::from)?;
    if !replayed {
        return Err(ApiError::not_found("no such parked job"));
    }
    Ok(Json(AdminReplayResult { replayed: 1 }))
}

#[utoipa::path(
    post,
    path = "/admin/api/dlq/replay-all",
    tag = "admin",
    operation_id = "adminReplayAllDeadLetters",
    summary = "Replay every parked job",
    responses((status = 200, description = "Replayed.", body = AdminReplayResult),
              (status = 401, description = "Admin authentication required.")),
    security(("AdminToken" = []))
)]
pub async fn replay_all_dead_letters(
    _auth: AdminAuth,
    State(state): State<AppState>,
) -> Result<Json<AdminReplayResult>, ApiError> {
    let replayed = dlq::replay_all(&state.pool, None)
        .await
        .map_err(ApiError::from)? as i64;
    Ok(Json(AdminReplayResult { replayed }))
}

// =============================================================================
// Shared helpers
// =============================================================================

fn parse_env_id(env_id: &str) -> Result<Uuid, ApiError> {
    ids::parse_typeid(ids::ENVIRONMENT, env_id)
        .ok_or_else(|| ApiError::not_found("no such environment"))
}

async fn ensure_environment(state: &AppState, env: Uuid) -> Result<(), ApiError> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM environments WHERE id = $1) AS "exists!""#,
        env,
    )
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found("no such environment"))
    }
}

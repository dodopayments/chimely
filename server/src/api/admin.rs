//! Admin plane: the embedded `/admin` dashboard's API.
//!
//! Built-in admin users with instance-wide roles gate the plane (`auth::AdminAuth`
//! and `roles`). Single-org: no organizations table, no per-environment user
//! scoping. Every query is either scoped to one `environment_id` or is an
//! explicit cross-environment admin path (the DLQ browser).
//!
//! Admin reads reuse the canonical queries (`inbox::list_items_for`,
//! `inbox::fetch_counts_for`) and admin writes reuse the canonical write-path
//! (`management::create_broadcast_idempotent`, `dlq::replay`). A second
//! implementation of the two-source merge or the broadcast write would
//! disagree with the first.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, SET_COOKIE};
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
use crate::auth::{self, AdminAuth};
use crate::error::ApiError;
use crate::extract::{ApiJson, ApiQuery};
use crate::roles::{Capability, Role};
use crate::state::AppState;
use crate::{dlq, ids};

const ADMIN_NOTIFICATION_PAGE: i64 = 50;
const ADMIN_NOTIFICATION_MAX_PAGE: i64 = 200;
const ADMIN_INBOX_PREVIEW: i64 = 20;
/// Vite emits content-hashed asset filenames, so the bundle is immutable.
const ASSET_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

// =============================================================================
// Embedded SPA (rust-embed): the binary ships the dashboard, no CDN or
// separate static host.
// =============================================================================

#[derive(rust_embed::RustEmbed)]
#[folder = "admin/dist"]
struct AdminAssets;

/// Serve the embedded SPA. Public by design. The shell must load to render the
/// login screen, then the SPA calls the session-gated JSON API which 401s until
/// login. The bundle carries no secrets. Unknown non-file paths fall back to
/// `index.html` so client-side routing works on refresh.
pub async fn serve_spa(uri: Uri) -> Response {
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

    // A path that looks like a file but is missing is a real 404. Anything
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
            "admin UI not built (run `pnpm --filter chimely-admin build`)",
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
    /// The dedicated subscriber HMAC secret. Plaintext by design. The customer
    /// backend computes hashes with it. Returned only to roles holding
    /// `env:read_secret` (developer/admin). Omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscriber_hmac_secret: Option<String>,
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
    /// Production default true. Set false for a dev/quickstart environment.
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
    responses(
        (status = 200, description = "Environments.", body = Vec<AdminEnvironment>),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn list_environments(
    auth: AdminAuth,
    State(state): State<AppState>,
) -> Result<Json<Vec<AdminEnvironment>>, ApiError> {
    auth.require(Capability::Read)?;
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
    responses(
        (status = 201, description = "Created.", body = AdminEnvironmentDetail),
        (
            status = 400,
            description = "Validation error or slug already exists.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "slug must be 1-255 characters"}}),
        ),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn create_environment(
    auth: AdminAuth,
    State(state): State<AppState>,
    ApiJson(req): ApiJson<AdminCreateEnvironmentRequest>,
) -> Result<Response, ApiError> {
    auth.require(Capability::EnvCreate)?;
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
            // The creator holds env:create, which only admin has, and admin
            // also holds env:read_secret, so the secret is always returned
            // here. It is needed immediately to wire up the widget.
            subscriber_hmac_secret: Some(hmac_secret),
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
    summary = "Get an environment",
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    responses(
        (status = 200, description = "Environment.", body = AdminEnvironmentDetail),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such environment"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn get_environment(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
) -> Result<Json<AdminEnvironmentDetail>, ApiError> {
    auth.require(Capability::Read)?;
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
        // env:read_secret gates the plaintext secret. viewer/operator see the
        // environment without it.
        subscriber_hmac_secret: auth
            .has(Capability::EnvReadSecret)
            .then_some(row.subscriber_hmac_secret),
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
    /// The new current secret. Update the customer backend with it. The
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
    summary = "Begin HMAC rotation",
    description = r#"Generate a new subscriber HMAC secret while keeping the old one valid during the changeover, so live inbox sessions are not interrupted. Complete the rotation once every backend uses the new secret."#,
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    responses(
        (status = 200, description = "Rotated.", body = AdminHmacRotation),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such environment"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn rotate_hmac(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
) -> Result<Json<AdminHmacRotation>, ApiError> {
    auth.require(Capability::HmacRotate)?;
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
    summary = "Complete HMAC rotation",
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    responses(
        (status = 204, description = "Previous secret cleared."),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such environment"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn complete_hmac_rotation(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require(Capability::HmacRotate)?;
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
    /// Display prefix for recognition. The full key is never retrievable.
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
    summary = "List API keys",
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    responses(
        (status = 200, description = "Keys.", body = Vec<AdminApiKey>),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such environment"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn list_api_keys(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
) -> Result<Json<Vec<AdminApiKey>>, ApiError> {
    auth.require(Capability::ApikeyRead)?;
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
    summary = "Create an API key",
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    request_body = AdminCreateApiKeyRequest,
    responses(
        (status = 201, description = "Created; `key` is shown once.", body = AdminApiKeyCreated),
        (
            status = 400,
            description = "Validation error.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "name must be 1-255 characters"}}),
        ),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such environment"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn create_api_key(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
    ApiJson(req): ApiJson<AdminCreateApiKeyRequest>,
) -> Result<Response, ApiError> {
    auth.require(Capability::ApikeyManage)?;
    let env = parse_env_id(&env_id)?;
    ensure_environment(&state, env).await?;
    let name = req.name.trim();
    if name.is_empty() || name.len() > 255 {
        return Err(ApiError::bad_request("name must be 1-255 characters"));
    }

    let id = ids::new_uuid();
    // 256 bits of randomness in the key body. The prefix is for display only.
    let key = format!("chml_live_{}", ids::new_uuid().as_simple());
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
    summary = "Revoke an API key",
    params(
        ("env_id" = String, Path, description = "Environment TypeID (env_…)."),
        ("key_id" = String, Path, description = "API key TypeID (key_…).")
    ),
    responses(
        (status = 204, description = "Revoked (now or already)."),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such API key.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such API key"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn revoke_api_key(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path((env_id, key_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    auth.require(Capability::ApikeyManage)?;
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
    summary = "List notifications",
    params(
        ("env_id" = String, Path, description = "Environment TypeID (env_…)."),
        AdminNotificationFilter
    ),
    responses(
        (status = 200, description = "A page of notifications.", body = AdminNotificationPage),
        (
            status = 400,
            description = "Malformed cursor or out-of-range limit.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "malformed cursor"}}),
        ),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such environment"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn list_notifications(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
    ApiQuery(filter): ApiQuery<AdminNotificationFilter>,
) -> Result<Json<AdminNotificationPage>, ApiError> {
    auth.require(Capability::Read)?;
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

    // Cross-partition, env-scoped admin scan, not a hot path. Keyset on
    // (visible_at, id) descending, matching the inbox ordering.
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
    summary = "Notification timeline",
    params(
        ("env_id" = String, Path, description = "Environment TypeID (env_…)."),
        ("notif_id" = String, Path, description = "Notification TypeID (notif_…).")
    ),
    responses(
        (status = 200, description = "The timeline so far.", body = AdminNotificationTimeline),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such notification.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such notification"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn notification_timeline(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path((env_id, notif_id)): Path<(String, String)>,
) -> Result<Json<AdminNotificationTimeline>, ApiError> {
    auth.require(Capability::Read)?;
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

    // Same read as the management-plane timeline. min() per status is a
    // defensive read-time dedupe. Writers guarantee one row per status.
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
    summary = "Compose a broadcast",
    description = "Send one announcement to every subscriber in the environment, from the admin dashboard.",
    params(("env_id" = String, Path, description = "Environment TypeID (env_…).")),
    request_body = AdminCreateBroadcastRequest,
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
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such environment.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such environment"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn create_broadcast(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path(env_id): Path<String>,
    ApiJson(req): ApiJson<AdminCreateBroadcastRequest>,
) -> Result<Response, ApiError> {
    auth.require(Capability::BroadcastCompose)?;
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
    summary = "Get a subscriber",
    params(
        ("env_id" = String, Path, description = "Environment TypeID (env_…)."),
        ("subscriber_id" = String, Path, description = "Customer-provided subscriber id.")
    ),
    responses(
        (status = 200, description = "Subscriber view.", body = AdminSubscriberView),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such subscriber.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such subscriber"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn get_subscriber(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path((env_id, subscriber_id)): Path<(String, String)>,
) -> Result<Json<AdminSubscriberView>, ApiError> {
    auth.require(Capability::Read)?;
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
    // The admin preview always shows the default view so it can never
    // disagree with what the subscriber sees.
    let inbox_rows = inbox::list_items_for(
        &mut *conn,
        env,
        identity.id,
        identity.created_at,
        inbox::ListWindow {
            cursor_ts: DateTime::<Utc>::MAX_UTC,
            cursor_id: Uuid::max(),
            limit: ADMIN_INBOX_PREVIEW,
            filter: inbox::InboxFilter::Default,
        },
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

/// Optional environment scope for a replay, mirroring the CLI's `--env`.
/// environment_id is part of every key. Without a scope the instance-wide
/// admin plane matches the bare job id across environments.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AdminReplayQuery {
    /// Environment slug to scope the replay to.
    pub environment: Option<String>,
}

/// Resolve an optional environment slug to its id, 404 if the slug is unknown.
/// A blank value (an empty `?environment=` from a form field or unset selector)
/// is treated as no scope, not a lookup of the empty slug.
async fn replay_scope(state: &AppState, slug: Option<String>) -> Result<Option<Uuid>, ApiError> {
    let Some(slug) = slug.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let id = dlq::environment_by_slug(&state.pool, &slug)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("no such environment"))?;
    Ok(Some(id))
}

#[utoipa::path(
    get,
    path = "/admin/api/dlq",
    tag = "admin",
    operation_id = "adminListDeadLetters",
    summary = "List dead letters",
    responses(
        (status = 200, description = "Parked jobs.", body = Vec<AdminDeadLetter>),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn list_dlq(
    auth: AdminAuth,
    State(state): State<AppState>,
) -> Result<Json<Vec<AdminDeadLetter>>, ApiError> {
    auth.require(Capability::Read)?;
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
    summary = "Replay dead letter",
    description = "Requeue one failed job for another attempt. Optionally scoped to one environment by slug.",
    params(
        ("job_id" = String, Path, description = "Job TypeID (job_…)."),
        AdminReplayQuery,
    ),
    responses(
        (status = 200, description = "Replayed.", body = AdminReplayResult),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such parked job, or unknown environment scope.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such parked job"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn replay_dead_letter(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    ApiQuery(query): ApiQuery<AdminReplayQuery>,
) -> Result<Json<AdminReplayResult>, ApiError> {
    auth.require(Capability::DlqReplay)?;
    let id = ids::parse_typeid(ids::JOB, &job_id)
        .ok_or_else(|| ApiError::not_found("no such parked job"))?;
    let environment = replay_scope(&state, query.environment).await?;
    let replayed = dlq::replay(&state.pool, id, environment)
        .await
        .map_err(ApiError::from)?;
    if replayed == 0 {
        return Err(ApiError::not_found("no such parked job"));
    }
    Ok(Json(AdminReplayResult {
        replayed: replayed as i64,
    }))
}

#[utoipa::path(
    post,
    path = "/admin/api/dlq/replay-all",
    tag = "admin",
    operation_id = "adminReplayAllDeadLetters",
    summary = "Replay all dead letters",
    description = "Requeue every parked job. Optionally scoped to one environment by slug.",
    params(AdminReplayQuery),
    responses(
        (status = 200, description = "Replayed.", body = AdminReplayResult),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Authenticated but the role lacks the capability.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "Unknown environment scope.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such environment"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn replay_all_dead_letters(
    auth: AdminAuth,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<AdminReplayQuery>,
) -> Result<Json<AdminReplayResult>, ApiError> {
    auth.require(Capability::DlqReplay)?;
    let environment = replay_scope(&state, query.environment).await?;
    let replayed = dlq::replay_all(&state.pool, environment)
        .await
        .map_err(ApiError::from)? as i64;
    Ok(Json(AdminReplayResult { replayed }))
}

// =============================================================================
// Session auth: login, logout, me
// =============================================================================

#[derive(Deserialize, ToSchema)]
pub struct AdminLoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize, ToSchema)]
pub struct AdminMe {
    /// TypeID, `adm_…`.
    pub id: String,
    pub email: String,
    pub name: String,
    /// 'viewer' | 'operator' | 'developer' | 'admin'.
    pub role: String,
    /// Capability strings the SPA gates UI on (server-side enforcement is the
    /// real boundary).
    pub capabilities: Vec<String>,
}

#[utoipa::path(
    post,
    path = "/admin/api/login",
    tag = "admin",
    operation_id = "adminLogin",
    summary = "Log in",
    description = "Sign in to the admin dashboard with email and password. On success a session cookie is set.",
    request_body = AdminLoginRequest,
    responses(
        (status = 200, description = "Logged in; sets the session cookie.", body = AdminMe),
        (
            status = 401,
            description = "Invalid email or password.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "invalid email or password"}}),
        ),
        (
            status = 403,
            description = "Missing X-Chimely-Admin header.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "missing X-Chimely-Admin header"}}),
        ),
    )
)]
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(req): ApiJson<AdminLoginRequest>,
) -> Result<Response, ApiError> {
    // login does not run the AdminAuth extractor, so it checks CSRF itself.
    auth::require_admin_csrf(&headers)?;
    let email = normalize_email(&req.email);

    let row = sqlx::query!(
        r#"SELECT id, email, name, role, password_hash
             FROM admin_users WHERE email = $1 AND disabled_at IS NULL"#,
        email,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let Some(row) = row else {
        // Spend the same time as a real verify so a missing email cannot be
        // distinguished from a wrong password.
        auth::equalize_login_timing(&req.password);
        return Err(ApiError::unauthorized("invalid email or password"));
    };
    if !auth::verify_password(&req.password, &row.password_hash) {
        return Err(ApiError::unauthorized("invalid email or password"));
    }
    let Some(role) = Role::parse(&row.role) else {
        return Err(ApiError::from(anyhow::anyhow!(
            "admin_users.role corrupt: {}",
            row.role
        )));
    };

    let token = auth::create_session(&state.pool, row.id, state.cfg.admin_session_ttl).await?;
    let cookie = auth::session_cookie(
        &token,
        state.cfg.admin_session_ttl,
        state.cfg.admin_tls_terminated,
    );

    let me = AdminMe {
        id: ids::typeid(ids::ADMIN_USER, row.id),
        email: row.email,
        name: row.name,
        role: role.as_str().to_owned(),
        capabilities: capability_strings(role),
    };
    Ok(([(SET_COOKIE, cookie)], Json(me)).into_response())
}

#[utoipa::path(
    post,
    path = "/admin/api/logout",
    tag = "admin",
    operation_id = "adminLogout",
    summary = "Log out",
    responses(
        (status = 204, description = "Logged out."),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Missing X-Chimely-Admin header.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "missing X-Chimely-Admin header"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn logout(auth: AdminAuth, State(state): State<AppState>) -> Result<Response, ApiError> {
    auth::delete_session(&state.pool, &auth.session_id).await?;
    let cookie = auth::clear_session_cookie(state.cfg.admin_tls_terminated);
    Ok((StatusCode::NO_CONTENT, [(SET_COOKIE, cookie)], ()).into_response())
}

#[utoipa::path(
    get,
    path = "/admin/api/me",
    tag = "admin",
    operation_id = "adminMe",
    summary = "Current admin user",
    responses(
        (status = 200, description = "The current user.", body = AdminMe),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn me(auth: AdminAuth) -> Json<AdminMe> {
    Json(AdminMe {
        id: ids::typeid(ids::ADMIN_USER, auth.user.id),
        email: auth.user.email.clone(),
        name: auth.user.name.clone(),
        role: auth.user.role.as_str().to_owned(),
        capabilities: capability_strings(auth.user.role),
    })
}

// =============================================================================
// User management (admin only). Guard rails: no self disable/delete, never
// remove the last enabled admin (no lockout).
// =============================================================================

#[derive(Serialize, ToSchema)]
pub struct AdminUserView {
    /// TypeID, `adm_…`.
    pub id: String,
    pub email: String,
    pub name: String,
    pub role: String,
    pub disabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize, ToSchema)]
pub struct AdminCreateUserRequest {
    pub email: String,
    pub name: String,
    pub role: String,
    /// At least 12 characters (server-enforced). The UI shows it once.
    pub password: String,
}

#[derive(Deserialize, ToSchema)]
pub struct AdminUpdateUserRequest {
    pub name: Option<String>,
    pub role: Option<String>,
    pub disabled: Option<bool>,
}

#[derive(Deserialize, ToSchema)]
pub struct AdminSetPasswordRequest {
    pub password: String,
}

#[utoipa::path(
    get,
    path = "/admin/api/users",
    tag = "admin",
    operation_id = "adminListUsers",
    summary = "List admin users",
    responses(
        (status = 200, description = "Users.", body = Vec<AdminUserView>),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Requires user:manage.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn list_users(
    auth: AdminAuth,
    State(state): State<AppState>,
) -> Result<Json<Vec<AdminUserView>>, ApiError> {
    auth.require(Capability::UserManage)?;
    let rows = sqlx::query!(
        r#"SELECT id, email, name, role, disabled_at, created_at, updated_at
             FROM admin_users ORDER BY created_at"#,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                user_view(
                    r.id,
                    r.email,
                    r.name,
                    r.role,
                    r.disabled_at,
                    r.created_at,
                    r.updated_at,
                )
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/api/users",
    tag = "admin",
    operation_id = "adminCreateUser",
    summary = "Create an admin user",
    request_body = AdminCreateUserRequest,
    responses(
        (status = 201, description = "Created.", body = AdminUserView),
        (
            status = 400,
            description = "Validation error.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "a valid email is required"}}),
        ),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Requires user:manage.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 409,
            description = "Email already exists.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "conflict", "message": "a user with that email already exists"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn create_user(
    auth: AdminAuth,
    State(state): State<AppState>,
    ApiJson(req): ApiJson<AdminCreateUserRequest>,
) -> Result<Response, ApiError> {
    auth.require(Capability::UserManage)?;
    let email = normalize_email(&req.email);
    if email.is_empty() || email.len() > 255 || !email.contains('@') {
        return Err(ApiError::bad_request("a valid email is required"));
    }
    let name = req.name.trim();
    if name.is_empty() || name.len() > 255 {
        return Err(ApiError::bad_request("name must be 1-255 characters"));
    }
    let role = Role::parse(req.role.trim()).ok_or_else(|| {
        ApiError::bad_request("role must be viewer, operator, developer, or admin")
    })?;
    auth::validate_password(&req.password)?;
    let hash = auth::hash_password(&req.password).map_err(ApiError::from)?;

    let id = ids::new_uuid();
    let created = sqlx::query!(
        r#"INSERT INTO admin_users (id, email, name, role, password_hash)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (email) DO NOTHING
           RETURNING created_at, updated_at"#,
        id,
        email,
        name,
        role.as_str(),
        hash,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let Some(created) = created else {
        return Err(ApiError::conflict("a user with that email already exists"));
    };

    Ok((
        StatusCode::CREATED,
        Json(user_view(
            id,
            email,
            name.to_owned(),
            role.as_str().to_owned(),
            None,
            created.created_at,
            created.updated_at,
        )),
    )
        .into_response())
}

#[utoipa::path(
    patch,
    path = "/admin/api/users/{user_id}",
    tag = "admin",
    operation_id = "adminUpdateUser",
    summary = "Update an admin user",
    params(("user_id" = String, Path, description = "Admin user TypeID (adm_…).")),
    request_body = AdminUpdateUserRequest,
    responses(
        (status = 200, description = "Updated.", body = AdminUserView),
        (
            status = 400,
            description = "Validation error.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "role must be viewer, operator, developer, or admin"}}),
        ),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Requires user:manage.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such user.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such user"}}),
        ),
        (
            status = 409,
            description = "Self-disable or last-admin guard rail.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "conflict", "message": "you cannot disable your own account"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn update_user(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    ApiJson(req): ApiJson<AdminUpdateUserRequest>,
) -> Result<Json<AdminUserView>, ApiError> {
    auth.require(Capability::UserManage)?;
    let id = parse_user_id(&user_id)?;

    // Validate the request shape before opening the transaction.
    let requested_role = match &req.role {
        Some(r) => Some(Role::parse(r.trim()).ok_or_else(|| {
            ApiError::bad_request("role must be viewer, operator, developer, or admin")
        })?),
        None => None,
    };
    let requested_name = match &req.name {
        Some(n) => {
            let n = n.trim();
            if n.is_empty() || n.len() > 255 {
                return Err(ApiError::bad_request("name must be 1-255 characters"));
            }
            Some(n.to_owned())
        }
        None => None,
    };

    // The last-admin check and the mutation run in one transaction. Taking the
    // roster advisory lock first serializes concurrent admin-roster changes, so
    // two requests cannot each see the other as the surviving admin (TOCTOU).
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    lock_admin_roster(&mut tx).await?;

    let current = sqlx::query!(
        r#"SELECT email, name, role, disabled_at FROM admin_users WHERE id = $1"#,
        id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no such user"))?;
    let current_role = Role::parse(&current.role).ok_or_else(|| {
        ApiError::from(anyhow::anyhow!(
            "admin_users.role corrupt: {}",
            current.role
        ))
    })?;

    let new_name = requested_name.unwrap_or_else(|| current.name.clone());
    let new_role = requested_role.unwrap_or(current_role);
    let currently_disabled = current.disabled_at.is_some();
    let new_disabled = req.disabled.unwrap_or(currently_disabled);

    // Guard rail: a user cannot disable themselves.
    if id == auth.user.id && new_disabled && !currently_disabled {
        return Err(ApiError::conflict("you cannot disable your own account"));
    }
    // Guard rail: never remove the last enabled admin.
    let was_enabled_admin = current_role == Role::Admin && !currently_disabled;
    let stays_enabled_admin = new_role == Role::Admin && !new_disabled;
    if was_enabled_admin && !stays_enabled_admin {
        ensure_other_enabled_admin_exists(&mut *tx, id).await?;
    }

    let row = sqlx::query!(
        r#"UPDATE admin_users
              SET name = $2,
                  role = $3,
                  disabled_at = CASE WHEN $4 THEN COALESCE(disabled_at, now()) ELSE NULL END,
                  updated_at = now()
            WHERE id = $1
            RETURNING email, created_at, updated_at, disabled_at"#,
        id,
        new_name,
        new_role.as_str(),
        new_disabled,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;

    // Newly disabled: drop their live sessions immediately.
    if new_disabled && !currently_disabled {
        sqlx::query!("DELETE FROM admin_sessions WHERE user_id = $1", id)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;

    Ok(Json(user_view(
        id,
        row.email,
        new_name,
        new_role.as_str().to_owned(),
        row.disabled_at,
        row.created_at,
        row.updated_at,
    )))
}

#[utoipa::path(
    post,
    path = "/admin/api/users/{user_id}/password",
    tag = "admin",
    operation_id = "adminSetUserPassword",
    summary = "Set user password",
    params(("user_id" = String, Path, description = "Admin user TypeID (adm_…).")),
    request_body = AdminSetPasswordRequest,
    responses(
        (status = 204, description = "Password updated."),
        (
            status = 400,
            description = "Password too short.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "invalid_request", "message": "password must be at least 12 characters"}}),
        ),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Not user:manage and not the target user.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such user.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such user"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn set_user_password(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    ApiJson(req): ApiJson<AdminSetPasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let id = parse_user_id(&user_id)?;
    // Self-service reset is allowed. Otherwise it needs user:manage.
    if id != auth.user.id {
        auth.require(Capability::UserManage)?;
    }
    auth::validate_password(&req.password)?;
    let hash = auth::hash_password(&req.password).map_err(ApiError::from)?;

    // The credential rotation and the session revocation are one transaction so
    // an admin-forced reset that evicts a compromised account is atomic. A
    // failure on the DELETE rolls back the password change rather than leaving
    // old sessions valid against the new credential.
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    let affected = sqlx::query!(
        "UPDATE admin_users SET password_hash = $2, updated_at = now() WHERE id = $1",
        id,
        hash,
    )
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from)?
    .rows_affected();
    if affected == 0 {
        return Err(ApiError::not_found("no such user"));
    }
    // Revoke every session for the target so an admin-forced reset evicts a
    // compromised account, and a self reset logs other devices out.
    sqlx::query!("DELETE FROM admin_sessions WHERE user_id = $1", id)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/admin/api/users/{user_id}",
    tag = "admin",
    operation_id = "adminDeleteUser",
    summary = "Delete an admin user",
    params(("user_id" = String, Path, description = "Admin user TypeID (adm_…).")),
    responses(
        (status = 204, description = "Deleted."),
        (
            status = 401,
            description = "Authentication required (no or expired session).",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "unauthorized", "message": "admin session required"}}),
        ),
        (
            status = 403,
            description = "Requires user:manage.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "forbidden", "message": "your role does not permit this action"}}),
        ),
        (
            status = 404,
            description = "No such user.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "not_found", "message": "no such user"}}),
        ),
        (
            status = 409,
            description = "Self-delete or last-admin guard rail.",
            body = crate::api::contract::Error,
            example = json!({"error": {"code": "conflict", "message": "you cannot delete your own account"}}),
        ),
    ),
    security(("AdminSession" = []))
)]
pub async fn delete_user(
    auth: AdminAuth,
    State(state): State<AppState>,
    Path(user_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require(Capability::UserManage)?;
    let id = parse_user_id(&user_id)?;
    if id == auth.user.id {
        return Err(ApiError::conflict("you cannot delete your own account"));
    }

    // Take the roster advisory lock, then check + delete in one transaction so
    // the last-admin guard cannot be raced (TOCTOU) by a concurrent removal.
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    lock_admin_roster(&mut tx).await?;

    let target = sqlx::query!(
        "SELECT role, disabled_at FROM admin_users WHERE id = $1",
        id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no such user"))?;
    if target.role == Role::Admin.as_str() && target.disabled_at.is_none() {
        ensure_other_enabled_admin_exists(&mut *tx, id).await?;
    }

    // Sessions cascade (admin_sessions FK ON DELETE CASCADE).
    sqlx::query!("DELETE FROM admin_users WHERE id = $1", id)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Shared helpers
// =============================================================================

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn capability_strings(role: Role) -> Vec<String> {
    role.capabilities()
        .iter()
        .map(|c| c.as_str().to_owned())
        .collect()
}

fn parse_user_id(user_id: &str) -> Result<Uuid, ApiError> {
    ids::parse_typeid(ids::ADMIN_USER, user_id).ok_or_else(|| ApiError::not_found("no such user"))
}

fn user_view(
    id: Uuid,
    email: String,
    name: String,
    role: String,
    disabled_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> AdminUserView {
    AdminUserView {
        id: ids::typeid(ids::ADMIN_USER, id),
        email,
        name,
        role,
        disabled: disabled_at.is_some(),
        created_at: format_ts(created_at),
        updated_at: format_ts(updated_at),
    }
}

/// Transaction-scoped advisory lock serializing admin-roster mutations
/// ("chmlADMR" as big-endian i64). A single lock, so the last-admin guard
/// cannot be raced (TOCTOU) and there is no row-order deadlock.
const ADMIN_ROSTER_LOCK_KEY: i64 = 0x63686d6c_41444d52;

/// Take the admin-roster advisory lock for the rest of the transaction.
async fn lock_admin_roster(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<(), ApiError> {
    // Runtime query (not the macro) for the void-returning advisory function,
    // matching the partition-maintenance lock.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMIN_ROSTER_LOCK_KEY)
        .execute(&mut **tx)
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

/// 409 unless another enabled admin (other than `excluding`) exists. The
/// no-lockout guard rail. Runs on the caller's executor so it shares the
/// transaction and the advisory lock taken by `lock_admin_roster`.
async fn ensure_other_enabled_admin_exists<'e, E>(
    executor: E,
    excluding: Uuid,
) -> Result<(), ApiError>
where
    E: sqlx::PgExecutor<'e>,
{
    let others = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM admin_users
            WHERE role = 'admin' AND disabled_at IS NULL AND id <> $1"#,
        excluding,
    )
    .fetch_one(executor)
    .await
    .map_err(ApiError::from)?;
    if others == 0 {
        return Err(ApiError::conflict(
            "cannot remove the last enabled admin (would lock everyone out)",
        ));
    }
    Ok(())
}

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

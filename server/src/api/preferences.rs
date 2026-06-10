//! Preferences, shared by both planes. Row ABSENCE means enabled: writes are
//! a partial upsert where `enabled=true` deletes the explicit row. The API
//! layer owns the allowed channel list ('in_app' only in v1 — deliberately no
//! CHECK constraint on the hot table).
//!
//! Counters ignore category mutes; a real flip enqueues a `counter_rebuild`
//! job for that one subscriber (eventual exactness) plus a hint, and bumps
//! `subscriber_counters.updated_at` so list ETags move.

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{ManagementAuth, SubscriberAuth, ensure_subscriber};
use crate::error::ApiError;
use crate::extract::ApiJson;
use crate::jobs;
use crate::state::AppState;

pub const ALLOWED_CHANNELS: &[&str] = &["in_app"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preference {
    pub category: String,
    /// Only `in_app` in v1; push channels later, no contract break.
    pub channel: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct PreferenceList {
    pub preferences: Vec<Preference>,
}

#[derive(Debug, Deserialize)]
pub struct PreferenceWriteList {
    pub preferences: Vec<Preference>,
}

#[utoipa::path(
    get,
    path = "/v1/subscribers/{subscriber_id}/preferences",
    tag = "management",
    operation_id = "getSubscriberPreferences",
    summary = "Read a subscriber's preferences (admin)",
    params(("subscriber_id" = String, Path, max_length = 255, description = "Customer-provided subscriber id (e.g. `usr_42`).")),
    responses(
        (status = 200, description = "Explicit preference rows only — absence means enabled.", body = crate::api::contract::PreferenceList),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
        (status = 404, description = "Resource not found in this environment.", body = crate::api::contract::Error),
    ),
    security(("ApiKeyBearer" = []))
)]
pub async fn get_subscriber_preferences(
    State(state): State<AppState>,
    auth: ManagementAuth,
    Path(subscriber_id): Path<String>,
) -> Result<Json<PreferenceList>, ApiError> {
    // Admin reads do NOT lazily create — a typo'd id should 404, not mint a
    // subscriber.
    let row = sqlx::query!(
        r#"SELECT id FROM subscribers WHERE environment_id = $1 AND subscriber_id = $2"#,
        auth.environment_id,
        &subscriber_id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no such subscriber"))?;
    Ok(Json(list(&state.pool, auth.environment_id, row.id).await?))
}

#[utoipa::path(
    put,
    path = "/v1/subscribers/{subscriber_id}/preferences",
    tag = "management",
    operation_id = "setSubscriberPreferences",
    summary = "Set preferences for a subscriber (admin)",
    params(("subscriber_id" = String, Path, max_length = 255, description = "Customer-provided subscriber id (e.g. `usr_42`).")),
    request_body = crate::api::contract::PreferenceWriteList,
    responses(
        (status = 200, description = "Updated.", body = crate::api::contract::PreferenceList),
        (status = 400, description = "Validation error.", body = crate::api::contract::Error),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
    ),
    security(("ApiKeyBearer" = []))
)]
pub async fn set_subscriber_preferences(
    State(state): State<AppState>,
    auth: ManagementAuth,
    Path(subscriber_id): Path<String>,
    ApiJson(body): ApiJson<PreferenceWriteList>,
) -> Result<Json<PreferenceList>, ApiError> {
    if subscriber_id.is_empty() || subscriber_id.len() > 255 {
        return Err(ApiError::bad_request(
            "subscriber_id must be 1–255 characters",
        ));
    }
    // Writes lazily create (the spec declares no 404 here) — preferences set
    // before the first notify must stick.
    let (subscriber, _) =
        ensure_subscriber(&state.pool, auth.environment_id, &subscriber_id).await?;
    Ok(Json(
        write(&state.pool, auth.environment_id, subscriber, body).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/inbox/preferences",
    tag = "subscriber",
    operation_id = "getPreferences",
    summary = "Read own preferences",
    responses(
        (status = 200, description = "Explicit preference rows only — absence means enabled.", body = crate::api::contract::PreferenceList),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn get_inbox_preferences(
    State(state): State<AppState>,
    auth: SubscriberAuth,
) -> Result<Json<PreferenceList>, ApiError> {
    Ok(Json(
        list(&state.pool, auth.environment_id, auth.subscriber_id).await?,
    ))
}

#[utoipa::path(
    put,
    path = "/v1/inbox/preferences",
    tag = "subscriber",
    operation_id = "setPreferences",
    summary = "Set own preferences",
    request_body = crate::api::contract::PreferenceWriteList,
    responses(
        (status = 200, description = "Updated.", body = crate::api::contract::PreferenceList),
        (status = 400, description = "Validation error.", body = crate::api::contract::Error),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
    ),
    security(("SubscriberEnv" = [], "SubscriberId" = [], "SubscriberHash" = []))
)]
pub async fn set_inbox_preferences(
    State(state): State<AppState>,
    auth: SubscriberAuth,
    ApiJson(body): ApiJson<PreferenceWriteList>,
) -> Result<Json<PreferenceList>, ApiError> {
    Ok(Json(
        write(&state.pool, auth.environment_id, auth.subscriber_id, body).await?,
    ))
}

/// Explicit preference rows only — absence means enabled.
pub async fn list(pool: &PgPool, env: Uuid, subscriber: Uuid) -> Result<PreferenceList, ApiError> {
    let rows = sqlx::query!(
        r#"SELECT category, channel, enabled FROM preferences
            WHERE environment_id = $1 AND subscriber_id = $2
            ORDER BY category, channel"#,
        env,
        subscriber,
    )
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(PreferenceList {
        preferences: rows
            .into_iter()
            .map(|r| Preference {
                category: r.category,
                channel: r.channel,
                enabled: r.enabled,
            })
            .collect(),
    })
}

/// Partial upsert: listed (category, channel) pairs are written, unlisted
/// pairs untouched. `enabled=true` deletes the explicit row.
pub async fn write(
    pool: &PgPool,
    env: Uuid,
    subscriber: Uuid,
    body: PreferenceWriteList,
) -> Result<PreferenceList, ApiError> {
    if body.preferences.is_empty() || body.preferences.len() > 100 {
        return Err(ApiError::bad_request(
            "preferences must contain 1–100 entries",
        ));
    }
    for p in &body.preferences {
        if p.category.is_empty() || p.category.len() > 255 {
            return Err(ApiError::bad_request("category must be 1–255 characters"));
        }
        if !ALLOWED_CHANNELS.contains(&p.channel.as_str()) {
            return Err(ApiError::bad_request(format!(
                "unknown channel: {}",
                p.channel
            )));
        }
    }

    let mut tx = pool.begin().await.map_err(ApiError::from)?;
    let mut changed = false;
    for p in &body.preferences {
        let affected = if p.enabled {
            sqlx::query!(
                r#"DELETE FROM preferences
                    WHERE environment_id = $1 AND subscriber_id = $2
                      AND category = $3 AND channel = $4"#,
                env,
                subscriber,
                &p.category,
                &p.channel,
            )
            .execute(&mut *tx)
            .await
            .map_err(ApiError::from)?
            .rows_affected()
        } else {
            sqlx::query!(
                r#"INSERT INTO preferences
                       (environment_id, subscriber_id, category, channel, enabled)
                   VALUES ($1, $2, $3, $4, false)
                   ON CONFLICT (environment_id, subscriber_id, category, channel)
                   DO UPDATE SET enabled = excluded.enabled, updated_at = now()
                   WHERE preferences.enabled IS DISTINCT FROM excluded.enabled"#,
                env,
                subscriber,
                &p.category,
                &p.channel,
            )
            .execute(&mut *tx)
            .await
            .map_err(ApiError::from)?
            .rows_affected()
        };
        changed |= affected > 0;
    }

    if changed {
        // updated_at is an ETag input; a preference DELETE can move
        // max(preferences.updated_at) backwards, so the counters bump is what
        // guarantees conditional refetches never serve a stale 304.
        sqlx::query!(
            r#"UPDATE subscriber_counters SET updated_at = now()
                WHERE environment_id = $1 AND subscriber_id = $2"#,
            env,
            subscriber,
        )
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        jobs::enqueue(
            &mut tx,
            env,
            jobs::TYPE_COUNTER_REBUILD,
            serde_json::json!({ "subscriber_id": subscriber }),
            None,
        )
        .await
        .map_err(ApiError::from)?;
        jobs::enqueue_hint(&mut tx, env, &[subscriber], "preferences")
            .await
            .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;

    list(pool, env, subscriber).await
}

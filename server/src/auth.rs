//! Both auth planes (specs/openapi.yaml, "securitySchemes").
//!
//! Management: `Authorization: Bearer <key>`; sha256(key) looked up over
//! non-revoked api_keys rows. The environment is implied by the key.
//!
//! Subscriber: environment slug + customer subscriber id + (when the
//! environment requires it) `hex(HMAC-SHA256(subscriber_hmac_secret,
//! subscriber_id))`, verified against the current then the previous secret
//! slot so secret rotation never invalidates live sessions. Headers with
//! query-parameter fallbacks (EventSource cannot set headers).

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;
use crate::ids;
use crate::state::AppState;

pub const HEADER_ENVIRONMENT: &str = "x-dronte-environment";
pub const HEADER_SUBSCRIBER: &str = "x-dronte-subscriber";
pub const HEADER_SUBSCRIBER_HASH: &str = "x-dronte-subscriber-hash";
pub const QUERY_ENVIRONMENT: &str = "environment";
pub const QUERY_SUBSCRIBER: &str = "subscriber_id";
pub const QUERY_SUBSCRIBER_HASH: &str = "subscriber_hash";

/// A management-plane caller. Resolving it authenticates the request.
pub struct ManagementAuth {
    pub environment_id: Uuid,
    /// The api_keys row that authenticated — the rate-limit bucket key.
    pub api_key_id: Uuid,
}

impl FromRequestParts<AppState> for ManagementAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|t| !t.is_empty())
            .ok_or_else(|| ApiError::unauthorized("missing bearer API key"))?;

        let key_hash: Vec<u8> = Sha256::digest(token.as_bytes()).to_vec();
        let row = sqlx::query!(
            r#"SELECT environment_id, id, last_used_at
                 FROM api_keys
                WHERE key_hash = $1 AND revoked_at IS NULL"#,
            &key_hash,
        )
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::unauthorized("invalid API key"))?;

        // Coarse last_used_at (at most ~1/min) — audit signal, not a hot write.
        if row
            .last_used_at
            .is_none_or(|t| Utc::now() - t > chrono::Duration::seconds(60))
        {
            sqlx::query!(
                "UPDATE api_keys SET last_used_at = now() WHERE environment_id = $1 AND id = $2",
                row.environment_id,
                row.id,
            )
            .execute(&state.pool)
            .await
            .ok();
        }

        Ok(Self {
            environment_id: row.environment_id,
            api_key_id: row.id,
        })
    }
}

/// A subscriber-plane caller. Resolving it authenticates the request AND
/// lazily upserts the subscriber + counters row ("first widget connect").
pub struct SubscriberAuth {
    pub environment_id: Uuid,
    /// Internal subscribers.id (uuid), not the customer-provided string.
    pub subscriber_id: Uuid,
    pub external_id: String,
    /// Drives broadcast visibility (`broadcasts.created_at >= this`).
    pub subscriber_created_at: DateTime<Utc>,
}

impl FromRequestParts<AppState> for SubscriberAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let query: Vec<(String, String)> = parts
            .uri
            .query()
            .map(|q| form_urlencoded::parse(q.as_bytes()).into_owned().collect())
            .unwrap_or_default();
        let pick = |header: &str, query_name: &str| -> Option<String> {
            parts
                .headers
                .get(header)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .or_else(|| {
                    query
                        .iter()
                        .find(|(k, _)| k == query_name)
                        .map(|(_, v)| v.clone())
                })
                .filter(|v| !v.is_empty())
        };

        let slug = pick(HEADER_ENVIRONMENT, QUERY_ENVIRONMENT)
            .ok_or_else(|| ApiError::unauthorized("missing environment"))?;
        let external_id = pick(HEADER_SUBSCRIBER, QUERY_SUBSCRIBER)
            .ok_or_else(|| ApiError::unauthorized("missing subscriber id"))?;
        if external_id.len() > 255 {
            return Err(ApiError::bad_request(
                "subscriber id exceeds 255 characters",
            ));
        }
        let hash = pick(HEADER_SUBSCRIBER_HASH, QUERY_SUBSCRIBER_HASH);

        let env = sqlx::query!(
            r#"SELECT id, require_subscriber_hash,
                      subscriber_hmac_secret, subscriber_hmac_secret_previous
                 FROM environments WHERE slug = $1"#,
            &slug,
        )
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::unauthorized("unknown environment"))?;

        match &hash {
            Some(hash) => {
                let valid = verify_subscriber_hash(&env.subscriber_hmac_secret, &external_id, hash)
                    || env
                        .subscriber_hmac_secret_previous
                        .as_deref()
                        .is_some_and(|prev| verify_subscriber_hash(prev, &external_id, hash));
                if !valid {
                    return Err(ApiError::unauthorized("invalid subscriber hash"));
                }
            }
            None if env.require_subscriber_hash => {
                return Err(ApiError::unauthorized("subscriber hash required"));
            }
            None => {} // dev-mode environment: hash optional
        }

        let (subscriber_id, subscriber_created_at) =
            ensure_subscriber(&state.pool, env.id, &external_id).await?;

        Ok(Self {
            environment_id: env.id,
            subscriber_id,
            external_id,
            subscriber_created_at,
        })
    }
}

/// `hex(HMAC-SHA256(secret, subscriber_id))`, constant-time comparison.
pub fn verify_subscriber_hash(secret: &str, subscriber_id: &str, hash_hex: &str) -> bool {
    let Ok(provided) = hex::decode(hash_hex) else {
        return false;
    };
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(subscriber_id.as_bytes());
    mac.verify_slice(&provided).is_ok()
}

/// Test/SDK helper: compute the subscriber hash the customer backend would.
pub fn compute_subscriber_hash(secret: &str, subscriber_id: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(subscriber_id.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Lazy subscriber + counters upsert. Returns (internal id, created_at).
pub async fn ensure_subscriber(
    pool: &sqlx::PgPool,
    environment_id: Uuid,
    external_id: &str,
) -> Result<(Uuid, DateTime<Utc>), ApiError> {
    let row = sqlx::query!(
        r#"SELECT id, created_at FROM subscribers
            WHERE environment_id = $1 AND subscriber_id = $2"#,
        environment_id,
        external_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    if let Some(row) = row {
        return Ok((row.id, row.created_at));
    }

    let mut tx = pool.begin().await.map_err(ApiError::from)?;
    sqlx::query!(
        r#"INSERT INTO subscribers (environment_id, id, subscriber_id)
           VALUES ($1, $2, $3)
           ON CONFLICT (environment_id, subscriber_id) DO NOTHING"#,
        environment_id,
        ids::new_uuid(),
        external_id,
    )
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    let row = sqlx::query!(
        r#"SELECT id, created_at FROM subscribers
            WHERE environment_id = $1 AND subscriber_id = $2"#,
        environment_id,
        external_id,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    sqlx::query!(
        r#"INSERT INTO subscriber_counters (environment_id, subscriber_id)
           VALUES ($1, $2)
           ON CONFLICT (environment_id, subscriber_id) DO NOTHING"#,
        environment_id,
        row.id,
    )
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;

    Ok((row.id, row.created_at))
}

//! The three auth planes.
//!
//! Management: `Authorization: Bearer <key>`; sha256(key) looked up over
//! non-revoked api_keys rows. The environment is implied by the key.
//!
//! Subscriber: environment slug + customer subscriber id + (when the
//! environment requires it) `hex(HMAC-SHA256(subscriber_hmac_secret,
//! subscriber_id))`, verified against the current then the previous secret
//! slot so secret rotation never invalidates live sessions. Headers with
//! query-parameter fallbacks (EventSource cannot set headers).
//!
//! Admin: built-in users (Argon2id passwords) with a server-side session in
//! an `HttpOnly; SameSite=Strict; Path=/admin` cookie. Roles are
//! instance-wide; capabilities gate each endpoint. Server-side sessions so
//! logout, expiry, disable-user, and role changes all take effect at once.

use std::time::Duration;

use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::header::COOKIE;
use axum::http::request::Parts;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ApiError;
use crate::ids;
use crate::roles::{Capability, Role};
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
    /// The api_keys row that authenticated. It keys the rate-limit bucket.
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

// =============================================================================
// Admin plane: built-in users, Argon2id passwords, server-side sessions.
// =============================================================================

/// The admin session cookie. `Path=/admin` scoped, `HttpOnly` (no JS access,
/// so XSS cannot exfiltrate it), `SameSite=Strict`, and `Secure` whenever TLS
/// terminates in front of the binary (config `admin_tls_terminated`).
pub const ADMIN_COOKIE: &str = "dronte_admin";

/// CSRF defense for mutating admin requests. A cross-site form cannot set a
/// custom header, and the admin plane has no CORS, so requiring it (on top of
/// `SameSite=Strict`) closes forged-request paths.
pub const ADMIN_CSRF_HEADER: &str = "x-dronte-admin";

/// Minimum admin password length, enforced server-side on create/reset.
pub const MIN_PASSWORD_LEN: usize = 12;

/// A resolved admin user (no password material).
#[derive(Clone)]
pub struct AdminUser {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub role: Role,
}

/// An authenticated admin caller (the embedded `/admin` dashboard plane).
/// Resolving it requires a live, non-expired session cookie for a
/// non-disabled user. Roles are instance-wide; capability checks gate each
/// endpoint. This is the security boundary — the SPA's UI gating is only
/// convenience.
pub struct AdminAuth {
    pub user: AdminUser,
    /// The session token that authenticated this request, so logout deletes
    /// exactly this session row.
    pub session_id: String,
}

impl AdminAuth {
    /// 403 unless the resolved role holds `cap`.
    pub fn require(&self, cap: Capability) -> Result<(), ApiError> {
        if self.user.role.has(cap) {
            Ok(())
        } else {
            Err(ApiError::forbidden("your role does not permit this action"))
        }
    }

    pub fn has(&self, cap: Capability) -> bool {
        self.user.role.has(cap)
    }
}

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        // CSRF: every state-changing method must carry the header. GET/HEAD
        // are safe and exempt. SameSite=Strict already blocks the cookie
        // cross-site; this is defense in depth.
        if !parts.method.is_safe() {
            require_admin_csrf(&parts.headers)?;
        }

        let token = admin_cookie(&parts.headers)
            .ok_or_else(|| ApiError::unauthorized("admin session required"))?;

        let row = sqlx::query!(
            r#"SELECT s.id AS session_id, u.id AS user_id, u.email, u.name, u.role
                 FROM admin_sessions s
                 JOIN admin_users u ON u.id = s.user_id
                WHERE s.id = $1 AND s.expires_at > now() AND u.disabled_at IS NULL"#,
            token,
        )
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::unauthorized("admin session required"))?;

        let Some(role) = Role::parse(&row.role) else {
            tracing::error!(role = %row.role, "admin_users.role is not a known role");
            return Err(ApiError::forbidden("role not recognized"));
        };

        // Coarse last_seen_at (at most ~1/min) — audit signal, not a hot write.
        sqlx::query!(
            "UPDATE admin_sessions SET last_seen_at = now()
              WHERE id = $1 AND last_seen_at < now() - interval '60 seconds'",
            token,
        )
        .execute(&state.pool)
        .await
        .ok();

        Ok(Self {
            user: AdminUser {
                id: row.user_id,
                email: row.email,
                name: row.name,
                role,
            },
            session_id: row.session_id,
        })
    }
}

/// The `dronte_admin` cookie value, if present.
fn admin_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(name, _)| *name == ADMIN_COOKIE)
        .map(|(_, value)| value.to_owned())
}

/// 403 unless the request carries a non-empty `X-Dronte-Admin` header.
pub fn require_admin_csrf(headers: &HeaderMap) -> Result<(), ApiError> {
    let present = headers
        .get(ADMIN_CSRF_HEADER)
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if present {
        Ok(())
    } else {
        Err(ApiError::forbidden("missing X-Dronte-Admin header"))
    }
}

// ----- Password hashing (Argon2id) ------------------------------------------

/// Argon2id PHC string with a per-hash random salt.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt_bytes: [u8; 16] = rand::random();
    let salt =
        SaltString::encode_b64(&salt_bytes).map_err(|e| anyhow::anyhow!("argon2 salt: {e}"))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 hash: {e}"))?;
    Ok(hash.to_string())
}

/// Constant-time verification via the crate. False on any parse/verify error.
pub fn verify_password(password: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Verify against a throwaway hash so a missing-user login costs the same as a
/// wrong-password login (no email enumeration via timing).
pub fn equalize_login_timing(password: &str) {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let phc = DUMMY.get_or_init(|| hash_password("not-a-real-password").unwrap_or_default());
    let _ = verify_password(password, phc);
}

pub fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(ApiError::bad_request(
            "password must be at least 12 characters",
        ));
    }
    Ok(())
}

// ----- Sessions -------------------------------------------------------------

/// 256 bits of randomness, hex-encoded — the opaque cookie value.
pub fn new_session_token() -> String {
    let bytes: [u8; 32] = rand::random();
    hex::encode(bytes)
}

/// Insert a session row expiring `ttl` from the DB clock; returns the token.
pub async fn create_session(
    pool: &PgPool,
    user_id: Uuid,
    ttl: Duration,
) -> Result<String, ApiError> {
    let token = new_session_token();
    sqlx::query!(
        r#"INSERT INTO admin_sessions (id, user_id, expires_at)
           VALUES ($1, $2, now() + make_interval(secs => $3))"#,
        token,
        user_id,
        ttl.as_secs_f64(),
    )
    .execute(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(token)
}

pub async fn delete_session(pool: &PgPool, token: &str) -> Result<(), ApiError> {
    sqlx::query!("DELETE FROM admin_sessions WHERE id = $1", token)
        .execute(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

/// Drop every session for a user (on disable, so access stops immediately even
/// without waiting for the live disabled_at check on their next request).
pub async fn delete_user_sessions(pool: &PgPool, user_id: Uuid) -> Result<(), ApiError> {
    sqlx::query!("DELETE FROM admin_sessions WHERE user_id = $1", user_id)
        .execute(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

/// `Set-Cookie` for a fresh session. `Secure` only with TLS terminated in
/// front (config) so the cookie still works over plain HTTP in dev/tests.
pub fn session_cookie(token: &str, ttl: Duration, secure: bool) -> String {
    let mut cookie = format!(
        "{ADMIN_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/admin; Max-Age={}",
        ttl.as_secs_f64().ceil() as u64,
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// `Set-Cookie` that clears the session cookie (logout).
pub fn clear_session_cookie(secure: bool) -> String {
    let mut cookie = format!("{ADMIN_COOKIE}=; HttpOnly; SameSite=Strict; Path=/admin; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
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

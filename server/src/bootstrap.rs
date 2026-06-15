//! Boot-time bootstrap: the dev-quickstart environment and the root admin.
//!
//! Dev quickstart: `DRONTE_DEV_ENVIRONMENT` plus `DRONTE_DEV_API_KEY` seed one
//! dev environment at boot: `require_subscriber_hash = false` so the widget
//! connects without a backend, and the API key is the plaintext from the env
//! var so the quickstart curl is copy-pasteable. Idempotent across restarts.
//!
//! Root admin: `DRONTE_ADMIN_EMAIL` + `DRONTE_ADMIN_PASSWORD` ensure a managed
//! `admin` account (see `ensure_admin`). This is the lockout-recovery path:
//! restart with the env vars set to restore admin access.

use sha2::Digest as _;
use sqlx::PgPool;

use crate::config::Config;
use crate::ids;

pub async fn run(pool: &PgPool, cfg: &Config) -> anyhow::Result<()> {
    let Some(slug) = cfg.dev_environment.as_deref() else {
        return Ok(());
    };
    tracing::warn!(
        environment = slug,
        "DEV bootstrap: subscriber hashes are NOT required in this environment. \
         Unset DRONTE_DEV_ENVIRONMENT in production."
    );

    // An existing environment is never modified. Rotating its HMAC secret
    // would silently break hashes computed against it, and downgrading
    // require_subscriber_hash would disable auth on an environment this
    // bootstrap does not own.
    let hmac_secret = format!("shmac_{}", ids::new_uuid().as_simple());
    let inserted = sqlx::query_scalar!(
        r#"INSERT INTO environments
               (id, slug, name, subscriber_hmac_secret, require_subscriber_hash)
           VALUES ($1, $2, $2, $3, false)
           ON CONFLICT (slug) DO NOTHING
           RETURNING id"#,
        ids::new_uuid(),
        slug,
        hmac_secret,
    )
    .fetch_optional(pool)
    .await?;
    let environment_id = match inserted {
        Some(id) => id,
        None => {
            let existing = sqlx::query!(
                "SELECT id, require_subscriber_hash FROM environments WHERE slug = $1",
                slug,
            )
            .fetch_one(pool)
            .await?;
            if existing.require_subscriber_hash {
                tracing::warn!(
                    environment = slug,
                    "DEV bootstrap: this environment already exists and requires \
                     subscriber hashes. Leaving it unchanged. The widget needs a \
                     subscriberHash to connect."
                );
            }
            existing.id
        }
    };

    if let Some(key) = cfg.dev_api_key.as_deref() {
        let key_hash = sha2::Sha256::digest(key.as_bytes()).to_vec();
        // Display prefix only. Slicing at a fixed byte offset would panic
        // when byte 14 falls inside a multi-byte character.
        let key_prefix = &key[..key.floor_char_boundary(14)];
        sqlx::query!(
            r#"INSERT INTO api_keys (environment_id, id, name, key_hash, key_prefix)
               VALUES ($1, $2, 'dev-bootstrap', $3, $4)
               ON CONFLICT (environment_id, key_hash) DO NOTHING"#,
            environment_id,
            ids::new_uuid(),
            key_hash,
            key_prefix,
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Ensure the managed root `admin` account when both env vars are set.
///
/// Idempotent: a no-op when the existing account already matches (same
/// password, role `admin`, not disabled). Otherwise the password is reset to
/// the env value, the role is forced to `admin`, and any disable is cleared,
/// so the env var stays the source of truth for the root credential and a UI
/// password change to it is overwritten on the next boot while the vars
/// remain set. Humans get their own UI-created accounts. No-op when either
/// var is unset.
pub async fn ensure_admin(pool: &PgPool, cfg: &Config) -> anyhow::Result<()> {
    let (Some(email), Some(password)) = (
        cfg.admin_bootstrap_email.as_deref(),
        cfg.admin_bootstrap_password.as_deref(),
    ) else {
        return Ok(());
    };
    let email = email.trim().to_lowercase();
    if password.chars().count() < crate::auth::MIN_PASSWORD_LEN {
        anyhow::bail!("DRONTE_ADMIN_PASSWORD must be at least 12 characters");
    }

    let existing = sqlx::query!(
        "SELECT id, role, password_hash, disabled_at FROM admin_users WHERE email = $1",
        email,
    )
    .fetch_optional(pool)
    .await?;

    match existing {
        Some(row)
            if row.role == "admin"
                && row.disabled_at.is_none()
                && crate::auth::verify_password(password, &row.password_hash) =>
        {
            // Already matches: nothing to do.
        }
        Some(row) => {
            let hash = crate::auth::hash_password(password)?;
            sqlx::query!(
                "UPDATE admin_users
                    SET password_hash = $2, role = 'admin', disabled_at = NULL, updated_at = now()
                  WHERE id = $1",
                row.id,
                hash,
            )
            .execute(pool)
            .await?;
            tracing::info!(email = %email, "bootstrap admin reconciled to the env credential");
        }
        None => {
            let hash = crate::auth::hash_password(password)?;
            sqlx::query!(
                "INSERT INTO admin_users (id, email, name, role, password_hash)
                 VALUES ($1, $2, 'Bootstrap admin', 'admin', $3)",
                ids::new_uuid(),
                email,
                hash,
            )
            .execute(pool)
            .await?;
            tracing::info!(email = %email, "bootstrap admin created");
        }
    }
    Ok(())
}

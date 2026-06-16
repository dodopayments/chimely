//! Dev-quickstart bootstrap.
//!
//! Environments and API keys are managed by the Phase 4 admin UI. Until
//! then (and forever, for the 30-second quickstart) `DRONTE_DEV_ENVIRONMENT`
//! plus `DRONTE_DEV_API_KEY` seed one dev environment at boot:
//! `require_subscriber_hash = false` so the widget connects without a
//! backend, and the API key is the plaintext from the env var so the
//! quickstart curl is copy-pasteable. Idempotent across restarts.

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

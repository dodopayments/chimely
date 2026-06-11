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

    // The HMAC secret only matters on first insert. The conflict arm must
    // not rotate it: hashes computed against it would silently break.
    let hmac_secret = format!("whsec_{}", ids::new_uuid().as_simple());
    let environment_id = sqlx::query_scalar!(
        r#"INSERT INTO environments
               (id, slug, name, subscriber_hmac_secret, require_subscriber_hash)
           VALUES ($1, $2, $2, $3, false)
           ON CONFLICT (slug) DO UPDATE SET require_subscriber_hash = false
           RETURNING id"#,
        ids::new_uuid(),
        slug,
        hmac_secret,
    )
    .fetch_one(pool)
    .await?;

    if let Some(key) = cfg.dev_api_key.as_deref() {
        let key_hash = sha2::Sha256::digest(key.as_bytes()).to_vec();
        let key_prefix = &key[..key.len().min(14)];
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

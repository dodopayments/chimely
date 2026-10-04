//! The subscriber hash binds to its environment. Isolation otherwise rests
//! entirely on per-environment server-minted secrets. If two environments
//! ever shared one (manual DB edit, restored dump), an unbound hash would
//! transfer between them. The environment-bound form includes the env
//! TypeID in the MAC input, so a hash minted for one environment is invalid
//! in every other. The legacy unbound form stays accepted while
//! subscriber_hash_legacy_accept is true (the default).

mod support;

use chimely::auth::{compute_subscriber_hash, compute_subscriber_hash_env_bound};
use chimely::ids;
use reqwest::header::{HeaderMap, HeaderValue};

fn subscriber_headers(slug: &str, subscriber: &str, hash: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Chimely-Environment",
        HeaderValue::from_str(slug).unwrap(),
    );
    headers.insert(
        "X-Chimely-Subscriber",
        HeaderValue::from_str(subscriber).unwrap(),
    );
    headers.insert(
        "X-Chimely-Subscriber-Hash",
        HeaderValue::from_str(hash).unwrap(),
    );
    headers
}

async fn counts_status(app: &support::TestApp, slug: &str, subscriber: &str, hash: &str) -> u16 {
    app.client
        .get(format!("{}/v1/inbox/counts", app.base))
        .headers(subscriber_headers(slug, subscriber, hash))
        .send()
        .await
        .expect("counts request")
        .status()
        .as_u16()
}

#[tokio::test]
async fn env_bound_hash_authenticates_and_legacy_stays_accepted() {
    let app = support::spawn().await;
    let env_typeid = ids::typeid(ids::ENVIRONMENT, app.env.id);

    let bound = compute_subscriber_hash_env_bound(&app.env.hmac_secret, &env_typeid, "usr_b");
    assert_eq!(
        counts_status(&app, &app.env.slug, "usr_b", &bound).await,
        200
    );

    let legacy = compute_subscriber_hash(&app.env.hmac_secret, "usr_b");
    assert_eq!(
        counts_status(&app, &app.env.slug, "usr_b", &legacy).await,
        200,
        "the legacy form must keep working through the changeover"
    );
}

#[tokio::test]
async fn legacy_hash_is_rejected_in_strict_mode() {
    let app = support::spawn_configured(false, |cfg| {
        cfg.subscriber_hash_legacy_accept = false;
    })
    .await;

    let legacy = compute_subscriber_hash(&app.env.hmac_secret, "usr_s");
    assert_eq!(
        counts_status(&app, &app.env.slug, "usr_s", &legacy).await,
        401
    );

    let env_typeid = ids::typeid(ids::ENVIRONMENT, app.env.id);
    let bound = compute_subscriber_hash_env_bound(&app.env.hmac_secret, &env_typeid, "usr_s");
    assert_eq!(
        counts_status(&app, &app.env.slug, "usr_s", &bound).await,
        200
    );
}

/// The audit threat: two environments sharing one secret. Server-minted
/// secrets make this unreachable normally, so it is staged with a direct
/// UPDATE. A bound hash minted for environment A must not authenticate in B.
#[tokio::test]
async fn bound_hash_does_not_transfer_between_environments_sharing_a_secret() {
    let app = support::spawn_configured(false, |cfg| {
        cfg.subscriber_hash_legacy_accept = false;
    })
    .await;
    let env_b = app.create_environment(true).await;
    sqlx::query("UPDATE environments SET subscriber_hmac_secret = $1 WHERE id = $2")
        .bind(&app.env.hmac_secret)
        .bind(env_b.id)
        .execute(&app.pool)
        .await
        .expect("share the secret");

    let env_a_typeid = ids::typeid(ids::ENVIRONMENT, app.env.id);
    let bound_for_a =
        compute_subscriber_hash_env_bound(&app.env.hmac_secret, &env_a_typeid, "usr_x");
    assert_eq!(
        counts_status(&app, &app.env.slug, "usr_x", &bound_for_a).await,
        200
    );
    assert_eq!(
        counts_status(&app, &env_b.slug, "usr_x", &bound_for_a).await,
        401,
        "a hash minted for environment A must not authenticate in B"
    );
}

/// Rotation interplay: a bound hash computed with the old secret verifies
/// against the previous slot, so formula adoption and secret rotation
/// compose without invalidating live sessions.
#[tokio::test]
async fn bound_hash_verifies_against_the_previous_secret_slot() {
    let app = support::spawn().await;
    sqlx::query(
        "UPDATE environments
            SET subscriber_hmac_secret_previous = subscriber_hmac_secret,
                subscriber_hmac_secret = 'fresh-secret-after-rotation'
          WHERE id = $1",
    )
    .bind(app.env.id)
    .execute(&app.pool)
    .await
    .expect("stage a rotation");

    let env_typeid = ids::typeid(ids::ENVIRONMENT, app.env.id);
    let bound_old = compute_subscriber_hash_env_bound(&app.env.hmac_secret, &env_typeid, "usr_r");
    assert_eq!(
        counts_status(&app, &app.env.slug, "usr_r", &bound_old).await,
        200
    );
}

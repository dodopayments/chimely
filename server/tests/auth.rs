//! HMAC subscriber-hash auth. Enforced when `require_subscriber_hash = true`,
//! optional in dev-mode environments, verified against current-then-previous
//! secret slots, headers with query fallbacks.

mod support;

use chimely::auth::compute_subscriber_hash;
use reqwest::header::{HeaderMap, HeaderValue};

const SUB: &str = "usr_auth";

fn headers(slug: &str, sub: &str, hash: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        "X-Chimely-Environment",
        HeaderValue::from_str(slug).unwrap(),
    );
    h.insert("X-Chimely-Subscriber", HeaderValue::from_str(sub).unwrap());
    if let Some(hash) = hash {
        h.insert(
            "X-Chimely-Subscriber-Hash",
            HeaderValue::from_str(hash).unwrap(),
        );
    }
    h
}

async fn get_counts_status(app: &support::TestApp, h: HeaderMap) -> u16 {
    app.client
        .get(format!("{}/v1/inbox/counts", app.base))
        .headers(h)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn hash_is_mandatory_when_the_environment_requires_it() {
    let app = support::spawn().await; // require_subscriber_hash = true
    let good = compute_subscriber_hash(&app.env.hmac_secret, app.env.id, SUB);

    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, None)).await,
        401,
        "missing hash"
    );
    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some("deadbeef"))).await,
        401,
        "wrong hash"
    );
    // A valid hash for a DIFFERENT subscriber id must not transfer.
    let other = compute_subscriber_hash(&app.env.hmac_secret, app.env.id, "usr_else");
    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some(&other))).await,
        401
    );

    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some(&good))).await,
        200
    );

    // Unknown environment slug.
    assert_eq!(
        get_counts_status(&app, headers("nope", SUB, Some(&good))).await,
        401
    );
    // Missing subscriber id.
    let mut h = HeaderMap::new();
    h.insert(
        "X-Chimely-Environment",
        HeaderValue::from_str(&app.env.slug).unwrap(),
    );
    assert_eq!(get_counts_status(&app, h).await, 401);
}

/// The pre-#55 formula: HMAC over subscriber_id alone, no environment input.
fn legacy_hash(secret: &str, sub: &str) -> String {
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .expect("hmac accepts any key length");
    mac.update(sub.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Dual-accept rollout for the env-bound hash formula (issue #55). A hash
/// minted with the legacy input (subscriber_id alone) still authenticates.
/// This test is deleted when the legacy fallback is dropped at the announced
/// minor version bump.
#[tokio::test]
async fn legacy_subscriber_id_only_hash_still_authenticates() {
    let app = support::spawn().await;
    let legacy = legacy_hash(&app.env.hmac_secret, SUB);
    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some(&legacy))).await,
        200,
        "legacy-formula hashes must keep working until the announced removal"
    );
}

/// A customer still on the legacy formula rotates their secret. Their
/// pre-rotation hash must verify through the previous-slot fallback, the same
/// overlap guarantee new-formula hashes get. Deleted with the legacy fallback.
#[tokio::test]
async fn legacy_hash_survives_rotation_via_previous_slot() {
    let app = support::spawn().await;
    let legacy = legacy_hash(&app.env.hmac_secret, SUB);

    sqlx::query(
        "UPDATE environments SET
             subscriber_hmac_secret = 'shmac_rotated_secret',
             subscriber_hmac_secret_previous = subscriber_hmac_secret,
             subscriber_hmac_rotated_at = now()
         WHERE id = $1",
    )
    .bind(app.env.id)
    .execute(&app.pool)
    .await
    .unwrap();

    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some(&legacy))).await,
        200,
        "legacy hashes ride the rotation overlap via the previous slot"
    );

    // Rotation ends: the legacy hash dies with the previous slot.
    sqlx::query("UPDATE environments SET subscriber_hmac_secret_previous = NULL WHERE id = $1")
        .bind(app.env.id)
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some(&legacy))).await,
        401
    );
}

#[tokio::test]
async fn dev_mode_environments_accept_missing_but_not_invalid_hashes() {
    let app = support::spawn_dev_mode().await; // require_subscriber_hash = false

    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, None)).await,
        200,
        "the 30-second quickstart: no backend, no hash"
    );
    // A present hash is still verified. A wrong one is rejected, not ignored.
    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some("deadbeef"))).await,
        401
    );
    let good = compute_subscriber_hash(&app.env.hmac_secret, app.env.id, SUB);
    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some(&good))).await,
        200
    );
}

#[tokio::test]
async fn rotation_verifies_current_then_previous_secret() {
    let app = support::spawn().await;
    let old_secret = app.env.hmac_secret.clone();
    let old_hash = compute_subscriber_hash(&old_secret, app.env.id, SUB);

    // Rotate: new secret current, old secret in the previous slot.
    let new_secret = "shmac_rotated_secret";
    sqlx::query(
        "UPDATE environments SET
             subscriber_hmac_secret = $2,
             subscriber_hmac_secret_previous = subscriber_hmac_secret,
             subscriber_hmac_rotated_at = now()
         WHERE id = $1",
    )
    .bind(app.env.id)
    .bind(new_secret)
    .execute(&app.pool)
    .await
    .unwrap();

    // Old-secret sessions keep working through the rotation overlap.
    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some(&old_hash))).await,
        200
    );
    let new_hash = compute_subscriber_hash(new_secret, app.env.id, SUB);
    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some(&new_hash))).await,
        200
    );

    // Rotation ends: previous slot cleared, old hashes die.
    sqlx::query("UPDATE environments SET subscriber_hmac_secret_previous = NULL WHERE id = $1")
        .bind(app.env.id)
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some(&old_hash))).await,
        401
    );
    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some(&new_hash))).await,
        200
    );
}

#[tokio::test]
async fn query_parameter_fallbacks_match_the_headers() {
    let app = support::spawn().await;
    let hash = compute_subscriber_hash(&app.env.hmac_secret, app.env.id, SUB);

    // Pure query auth (the EventSource case) on a regular endpoint.
    let res = app
        .client
        .get(format!(
            "{}/v1/inbox/counts?environment={}&subscriber_id={SUB}&subscriber_hash={hash}",
            app.base, app.env.slug,
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    // Headers win over conflicting query parameters.
    let res = app
        .client
        .get(format!(
            "{}/v1/inbox/counts?environment=wrong&subscriber_id=wrong&subscriber_hash=wrong",
            app.base,
        ))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    // Wrong query hash alone fails.
    let res = app
        .client
        .get(format!(
            "{}/v1/inbox/counts?environment={}&subscriber_id={SUB}&subscriber_hash=beef",
            app.base, app.env.slug,
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn subscriber_plane_lazily_creates_the_subscriber_on_first_connect() {
    let app = support::spawn().await;
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM subscribers WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(before, 0);

    assert_eq!(
        get_counts_status(&app, app.subscriber_headers("usr_fresh")).await,
        200
    );

    let (count, counters): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM subscribers WHERE environment_id = $1),
                (SELECT count(*) FROM subscriber_counters WHERE environment_id = $1)",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        (count, counters),
        (1, 1),
        "subscriber + counters row on first widget connect"
    );
}

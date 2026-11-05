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
    let good = compute_subscriber_hash(&app.env.hmac_secret, SUB);

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
    let other = compute_subscriber_hash(&app.env.hmac_secret, "usr_else");
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
    let good = compute_subscriber_hash(&app.env.hmac_secret, SUB);
    assert_eq!(
        get_counts_status(&app, headers(&app.env.slug, SUB, Some(&good))).await,
        200
    );
}

#[tokio::test]
async fn rotation_verifies_current_then_previous_secret() {
    let app = support::spawn().await;
    let old_secret = app.env.hmac_secret.clone();
    let old_hash = compute_subscriber_hash(&old_secret, SUB);

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
    let new_hash = compute_subscriber_hash(new_secret, SUB);
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
    let hash = compute_subscriber_hash(&app.env.hmac_secret, SUB);

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

// ----- Expiring (v2) subscriber hashes ---------------------------------------

/// The v2 message the customer backend signs. Built here, not with the crate
/// helper, so the tests also pin the wire format.
fn v2_hash(secret: &str, sub: &str, expires_at: i64) -> String {
    use hmac::{Hmac, Mac as _};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(format!("v2\n{sub}\n{expires_at}").as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn v2_headers(slug: &str, sub: &str, hash: &str, expires_at: &str) -> HeaderMap {
    let mut h = headers(slug, sub, Some(hash));
    h.insert(
        "X-Chimely-Subscriber-Expires",
        HeaderValue::from_str(expires_at).unwrap(),
    );
    h
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

async fn counts_error(app: &support::TestApp, h: HeaderMap) -> (u16, String) {
    let res = app
        .client
        .get(format!("{}/v1/inbox/counts", app.base))
        .headers(h)
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    let body: serde_json::Value = res.json().await.unwrap_or_default();
    (
        status,
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}

async fn patch_env(app: &support::TestApp, body: serde_json::Value) -> reqwest::Response {
    let env = chimely::ids::typeid(chimely::ids::ENVIRONMENT, app.env.id);
    app.admin_patch(&format!("/admin/api/environments/{env}"), body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn v2_hash_with_a_future_expiry_is_accepted() {
    let app = support::spawn().await;
    let exp = now_secs() + 600;
    let hash = v2_hash(&app.env.hmac_secret, SUB, exp);

    assert_eq!(
        get_counts_status(
            &app,
            v2_headers(&app.env.slug, SUB, &hash, &exp.to_string())
        )
        .await,
        200,
        "header form"
    );
    let res = app
        .client
        .get(format!(
            "{}/v1/inbox/counts?environment={}&subscriber_id={SUB}&subscriber_hash={hash}&subscriber_expires={exp}",
            app.base, app.env.slug,
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "query form");
}

#[tokio::test]
async fn v2_hash_is_refused_when_expired_too_far_or_for_another_id() {
    let app = support::spawn().await;
    let secret = app.env.hmac_secret.clone();
    let slug = app.env.slug.clone();

    let past = now_secs() - 1;
    let (status, message) = counts_error(
        &app,
        v2_headers(&slug, SUB, &v2_hash(&secret, SUB, past), &past.to_string()),
    )
    .await;
    assert_eq!((status, message.as_str()), (401, "subscriber hash expired"));

    // The default environment maximum is 86400 seconds.
    let far = now_secs() + 86_400 + 3600;
    let (status, message) = counts_error(
        &app,
        v2_headers(&slug, SUB, &v2_hash(&secret, SUB, far), &far.to_string()),
    )
    .await;
    assert_eq!(
        (status, message.as_str()),
        (
            401,
            "subscriber hash expiry exceeds the environment maximum"
        )
    );

    let exp = now_secs() + 600;
    let other = v2_hash(&secret, "usr_else", exp);
    assert_eq!(
        get_counts_status(&app, v2_headers(&slug, SUB, &other, &exp.to_string())).await,
        401,
        "a v2 hash for another subscriber id"
    );
    let legacy = compute_subscriber_hash(&secret, SUB);
    assert_eq!(
        get_counts_status(&app, v2_headers(&slug, SUB, &legacy, &exp.to_string())).await,
        401,
        "a legacy hash sent with an expiry"
    );
    let (status, message) = counts_error(&app, v2_headers(&slug, SUB, &legacy, "soon")).await;
    assert_eq!(
        (status, message.as_str()),
        (400, "subscriber hash expiry must be unix seconds")
    );

    // A newline in the id would make the v2 message ambiguous.
    let res = app
        .client
        .get(format!(
            "{}/v1/inbox/counts?environment={slug}&subscriber_id=usr%0A1",
            app.base,
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400, "newline in the subscriber id");
}

#[tokio::test]
async fn legacy_hash_is_refused_once_the_environment_requires_expiry() {
    let app = support::spawn().await;
    let slug = app.env.slug.clone();
    let legacy = compute_subscriber_hash(&app.env.hmac_secret, SUB);
    assert_eq!(
        get_counts_status(&app, headers(&slug, SUB, Some(&legacy))).await,
        200,
        "legacy hashes work by default"
    );

    let res = patch_env(
        &app,
        serde_json::json!({ "require_subscriber_hash_expiry": true }),
    )
    .await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["require_subscriber_hash_expiry"], true);
    assert_eq!(body["subscriber_hash_max_ttl_seconds"], 86_400);

    let (status, message) = counts_error(&app, headers(&slug, SUB, Some(&legacy))).await;
    assert_eq!(
        (status, message.as_str()),
        (401, "subscriber hash expiry required")
    );

    let exp = now_secs() + 600;
    let v2 = v2_hash(&app.env.hmac_secret, SUB, exp);
    assert_eq!(
        get_counts_status(&app, v2_headers(&slug, SUB, &v2, &exp.to_string())).await,
        200,
        "v2 still works"
    );

    // The previous secret slot verifies v2 too.
    let env = chimely::ids::typeid(chimely::ids::ENVIRONMENT, app.env.id);
    let res = app
        .admin_post(
            &format!("/admin/api/environments/{env}/hmac/rotate"),
            serde_json::json!({}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        get_counts_status(&app, v2_headers(&slug, SUB, &v2, &exp.to_string())).await,
        200,
        "previous secret slot"
    );
}

#[tokio::test]
async fn environment_max_ttl_is_enforced_and_validated() {
    let app = support::spawn().await;
    let slug = app.env.slug.clone();
    for bad in [59, 604_801] {
        let res = patch_env(
            &app,
            serde_json::json!({ "subscriber_hash_max_ttl_seconds": bad }),
        )
        .await;
        assert_eq!(res.status(), 400, "ttl {bad}");
    }
    let res = patch_env(
        &app,
        serde_json::json!({ "subscriber_hash_max_ttl_seconds": 120 }),
    )
    .await;
    assert_eq!(res.status(), 200);

    // 60 seconds of clock-skew allowance on top of the maximum.
    let ok = now_secs() + 150;
    let too_far = now_secs() + 600;
    let secret = app.env.hmac_secret.clone();
    assert_eq!(
        get_counts_status(
            &app,
            v2_headers(&slug, SUB, &v2_hash(&secret, SUB, ok), &ok.to_string())
        )
        .await,
        200
    );
    assert_eq!(
        get_counts_status(
            &app,
            v2_headers(
                &slug,
                SUB,
                &v2_hash(&secret, SUB, too_far),
                &too_far.to_string()
            )
        )
        .await,
        401
    );

    let env = chimely::ids::typeid(chimely::ids::ENVIRONMENT, app.env.id);
    let detail: serde_json::Value = app
        .admin_get(&format!("/admin/api/environments/{env}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["subscriber_hash_max_ttl_seconds"], 120);
    assert_eq!(detail["require_subscriber_hash_expiry"], false);
}

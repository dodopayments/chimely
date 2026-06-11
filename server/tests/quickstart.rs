//! The 30-second-quickstart enablers: the dev bootstrap (environment + API
//! key from env-var config) and subscriber-plane CORS. Both exist for the
//! Phase 2 Next.js example, which runs the widget in a browser against a
//! local Redis-less dronte.

mod support;

#[tokio::test]
async fn dev_bootstrap_seeds_an_environment_and_key_idempotently() {
    let app = support::spawn().await;
    let mut cfg = (*app.cfg).clone();
    cfg.dev_environment = Some("demo".into());
    cfg.dev_api_key = Some("dev-secret-key".into());

    dronte::bootstrap::run(&app.pool, &cfg)
        .await
        .expect("first bootstrap");
    dronte::bootstrap::run(&app.pool, &cfg)
        .await
        .expect("bootstrap reruns cleanly");

    let environments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM environments WHERE slug = 'demo'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(environments, 1, "rerun must not duplicate the environment");
    let keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_keys WHERE name = 'dev-bootstrap' AND revoked_at IS NULL",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(keys, 1, "rerun must not duplicate the key");

    // The seeded key authenticates a management create against the seeded
    // environment, the exact call the quickstart curl makes.
    let res = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .bearer_auth("dev-secret-key")
        .json(&serde_json::json!({
            "subscriber_id": "usr_demo",
            "category": "demo.greeting",
            "payload": { "title": "Hello from the quickstart" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
}

/// Tested invariant (risk: env-var misconfiguration in production): the
/// bootstrap never modifies an environment it did not create. A slug
/// collision with an environment that requires subscriber hashes must not
/// downgrade that requirement on restart.
#[tokio::test]
async fn dev_bootstrap_leaves_an_existing_environment_untouched() {
    let app = support::spawn().await;
    let mut cfg = (*app.cfg).clone();
    cfg.dev_environment = Some(app.env.slug.clone());
    cfg.dev_api_key = Some("dev-secret-key".into());

    let before: (String, bool) = sqlx::query_as(
        "SELECT subscriber_hmac_secret, require_subscriber_hash
         FROM environments WHERE slug = $1",
    )
    .bind(&app.env.slug)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(before.1, "the support environment requires hashes");

    dronte::bootstrap::run(&app.pool, &cfg)
        .await
        .expect("bootstrap against an existing environment succeeds");

    let after: (String, bool) = sqlx::query_as(
        "SELECT subscriber_hmac_secret, require_subscriber_hash
         FROM environments WHERE slug = $1",
    )
    .bind(&app.env.slug)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(after.1, "require_subscriber_hash must not be downgraded");
    assert_eq!(after.0, before.0, "the HMAC secret must not rotate");
}

#[tokio::test]
async fn dev_bootstrap_truncates_the_key_prefix_on_a_char_boundary() {
    let app = support::spawn().await;
    let mut cfg = (*app.cfg).clone();
    cfg.dev_environment = Some("demo-utf8".into());
    // Byte 14 falls inside the two-byte é. A fixed byte slice would panic.
    cfg.dev_api_key = Some("abcdefghijklmé-secret".into());

    dronte::bootstrap::run(&app.pool, &cfg)
        .await
        .expect("a multi-byte key must not panic the bootstrap");

    let prefix: String = sqlx::query_scalar(
        "SELECT key_prefix FROM api_keys WHERE name = 'dev-bootstrap'
         AND environment_id = (SELECT id FROM environments WHERE slug = 'demo-utf8')",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(prefix, "abcdefghijklm");
}

#[tokio::test]
async fn subscriber_plane_is_cors_enabled_for_the_widget() {
    let app = support::spawn_dev_mode().await;

    // Preflight, exactly as a browser sends it before a conditional list GET.
    let res = app
        .client
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/v1/inbox/items", app.base),
        )
        .header("Origin", "https://customer.example")
        .header("Access-Control-Request-Method", "GET")
        .header(
            "Access-Control-Request-Headers",
            "x-dronte-environment,x-dronte-subscriber,if-none-match",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("*")
    );
    let allowed_headers = res
        .headers()
        .get("access-control-allow-headers")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    for header in [
        "x-dronte-environment",
        "x-dronte-subscriber",
        "x-dronte-subscriber-hash",
        "if-none-match",
    ] {
        assert!(allowed_headers.contains(header), "missing {header}");
    }

    // The actual response must carry the allow-origin header AND expose
    // ETag. ETag is not CORS-safelisted, and a hidden ETag silently breaks
    // the SDK's conditional refetch.
    let res = app
        .client
        .get(format!("{}/v1/inbox/items", app.base))
        .header("Origin", "https://customer.example")
        .header("X-Dronte-Environment", &app.env.slug)
        .header("X-Dronte-Subscriber", "usr_cors")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("*")
    );
    let exposed = res
        .headers()
        .get("access-control-expose-headers")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(exposed.contains("etag"), "ETag must be CORS-exposed");

    // The management plane gets no CORS on purpose. API keys do not belong
    // in browsers.
    let res = app
        .client
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/v1/notifications", app.base),
        )
        .header("Origin", "https://customer.example")
        .header("Access-Control-Request-Method", "POST")
        .send()
        .await
        .unwrap();
    assert!(
        res.headers().get("access-control-allow-origin").is_none(),
        "management plane must not be CORS-enabled"
    );
}

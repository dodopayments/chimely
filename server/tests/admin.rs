//! Admin plane.
//!
//! Auth gating, environment/API-key lifecycle, HMAC two-slot rotation,
//! broadcast composing as ONE row, the subscriber-lookup golden test (admin
//! merged inbox == subscriber-plane API), DLQ replay through the normal claim
//! path, and the status-timeline browser.

mod support;

use chimely::auth::compute_subscriber_hash;
use chimely::ids;
use serde_json::{Value, json};

fn env_typeid(app: &support::TestApp) -> String {
    ids::typeid(ids::ENVIRONMENT, app.env.id)
}

/// Status of a GET with the given (already logged-in) client.
async fn get_status(client: &reqwest::Client, url: String) -> u16 {
    client.get(url).send().await.unwrap().status().as_u16()
}

/// Status of a POST (with the CSRF header) using the given client.
async fn post_status(client: &reqwest::Client, url: String, body: Value) -> u16 {
    client
        .post(url)
        .header("x-chimely-admin", "1")
        .json(&body)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// Subscriber-plane GET /v1/inbox/counts with a hash computed from `secret`.
async fn counts_status_with_secret(
    app: &support::TestApp,
    subscriber: &str,
    secret: &str,
) -> reqwest::StatusCode {
    let hash = compute_subscriber_hash(secret, subscriber);
    app.client
        .get(format!("{}/v1/inbox/counts", app.base))
        .header("X-Chimely-Environment", app.env.slug.clone())
        .header("X-Chimely-Subscriber", subscriber)
        .header("X-Chimely-Subscriber-Hash", hash)
        .send()
        .await
        .expect("counts")
        .status()
}

// =============================================================================
// Auth: sessions, login/logout, CSRF, capabilities
// =============================================================================

/// The JSON API requires a session. The SPA shell is public so it can render
/// the login screen.
#[tokio::test]
async fn admin_api_requires_a_session_but_the_spa_shell_is_public() {
    let app = support::spawn().await;
    let anon = reqwest::Client::new();

    // API + /me, no session.
    assert_eq!(
        get_status(&anon, format!("{}/admin/api/environments", app.base)).await,
        401
    );
    assert_eq!(
        get_status(&anon, format!("{}/admin/api/me", app.base)).await,
        401
    );

    // The SPA shell loads (so it can render login), at /admin and nested routes.
    let spa = anon
        .get(format!("{}/admin", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(spa.status(), 200);
    assert!(
        spa.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert_eq!(
        get_status(&anon, format!("{}/admin/environments", app.base)).await,
        200
    );

    // The logged-in harness client succeeds.
    assert_eq!(
        app.admin_get("/admin/api/environments")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

/// Login failures are generic. Success sets a hardened cookie and yields the
/// user. A mutating request without the CSRF header is refused.
#[tokio::test]
async fn login_success_failure_and_cookie_flags() {
    let app = support::spawn().await;
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let login_url = format!("{}/admin/api/login", app.base);

    // Wrong password and unknown email both return the same generic 401.
    for body in [
        json!({"email": support::ADMIN_TEST_EMAIL, "password": "wrong-password-xx"}),
        json!({"email": "nobody@nowhere.test", "password": "whatever-123456"}),
    ] {
        let res = client
            .post(&login_url)
            .header("x-chimely-admin", "1")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401);
        assert_eq!(
            res.json::<Value>().await.unwrap()["error"]["message"],
            json!("invalid email or password")
        );
    }

    // Login without the CSRF header is refused.
    let no_csrf = client
        .post(&login_url)
        .json(
            &json!({"email": support::ADMIN_TEST_EMAIL, "password": support::ADMIN_TEST_PASSWORD}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(no_csrf.status(), 403);

    // Correct login.
    let ok = client
        .post(&login_url)
        .header("x-chimely-admin", "1")
        .json(
            &json!({"email": support::ADMIN_TEST_EMAIL, "password": support::ADMIN_TEST_PASSWORD}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    let set_cookie = ok
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(set_cookie.contains("chimely_admin="));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Strict"));
    assert!(set_cookie.contains("Path=/admin"));
    // No TLS hint in tests, so no Secure (otherwise the cookie would not ride
    // over plain HTTP).
    assert!(!set_cookie.contains("Secure"));

    let me: Value = ok.json().await.unwrap();
    assert_eq!(me["email"], json!(support::ADMIN_TEST_EMAIL));
    assert_eq!(me["role"], json!("admin"));
    assert!(me["id"].as_str().unwrap().starts_with("adm_"));
    assert!(
        me["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "user:manage")
    );

    // The session now authenticates.
    assert_eq!(
        get_status(&client, format!("{}/admin/api/me", app.base)).await,
        200
    );
}

/// Logout deletes the session row. The cookie no longer authenticates.
#[tokio::test]
async fn logout_invalidates_the_session() {
    let app = support::spawn().await;
    let client = app
        .login_client(support::ADMIN_TEST_EMAIL, support::ADMIN_TEST_PASSWORD)
        .await;
    assert_eq!(
        get_status(&client, format!("{}/admin/api/me", app.base)).await,
        200
    );

    let out = client
        .post(format!("{}/admin/api/logout", app.base))
        .header("x-chimely-admin", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(out.status(), 204);

    assert_eq!(
        get_status(&client, format!("{}/admin/api/me", app.base)).await,
        401
    );
}

/// A session past `expires_at` is rejected.
#[tokio::test]
async fn session_expires() {
    let app = support::spawn_configured(false, |cfg| {
        cfg.admin_session_ttl = std::time::Duration::from_millis(500);
    })
    .await;
    let client = app
        .login_client(support::ADMIN_TEST_EMAIL, support::ADMIN_TEST_PASSWORD)
        .await;
    assert_eq!(
        get_status(&client, format!("{}/admin/api/me", app.base)).await,
        200
    );
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    assert_eq!(
        get_status(&client, format!("{}/admin/api/me", app.base)).await,
        401
    );
}

/// Disabling a user kills their live session and blocks re-login.
#[tokio::test]
async fn disabled_user_cannot_authenticate() {
    let app = support::spawn().await;
    let created: Value = app
        .admin_post(
            "/admin/api/users",
            json!({"email": "v@disable.test", "name": "V", "role": "viewer", "password": "viewer-password-1"}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let uid = created["id"].as_str().unwrap().to_owned();

    let viewer = app
        .login_client("v@disable.test", "viewer-password-1")
        .await;
    assert_eq!(
        get_status(&viewer, format!("{}/admin/api/me", app.base)).await,
        200
    );

    let patch = app
        .admin_patch(
            &format!("/admin/api/users/{uid}"),
            json!({"disabled": true}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(patch.status(), 200);

    // Existing session rejected, and re-login refused.
    assert_eq!(
        get_status(&viewer, format!("{}/admin/api/me", app.base)).await,
        401
    );
    let relogin = reqwest::Client::new()
        .post(format!("{}/admin/api/login", app.base))
        .header("x-chimely-admin", "1")
        .json(&json!({"email": "v@disable.test", "password": "viewer-password-1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(relogin.status(), 401);
}

/// A logged-in but headerless mutating request is refused (CSRF), GET is fine.
#[tokio::test]
async fn mutating_requests_require_the_csrf_header() {
    let app = support::spawn().await;
    let no_header = app
        .client
        .post(format!("{}/admin/api/environments", app.base))
        .json(&json!({"slug": "csrf", "name": "csrf"}))
        .send()
        .await
        .unwrap();
    assert_eq!(no_header.status(), 403);
    assert_eq!(
        app.client
            .get(format!("{}/admin/api/environments", app.base))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

/// Each role 200s on what it may do and 403s on what it may not.
#[tokio::test]
async fn capability_matrix_is_enforced_per_role() {
    let app = support::spawn().await;
    let env = env_typeid(&app);
    for role in ["viewer", "operator", "developer"] {
        let res = app
            .admin_post(
                "/admin/api/users",
                json!({"email": format!("{role}@matrix.test"), "name": role, "role": role, "password": "password-123456"}),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201, "create {role}");
    }
    let viewer = app
        .login_client("viewer@matrix.test", "password-123456")
        .await;
    let operator = app
        .login_client("operator@matrix.test", "password-123456")
        .await;
    let developer = app
        .login_client("developer@matrix.test", "password-123456")
        .await;

    let base = &app.base;
    let envs_url = || format!("{base}/admin/api/environments");
    let dlq_url = || format!("{base}/admin/api/dlq/replay-all");
    let bcast_url = || format!("{base}/admin/api/environments/{env}/broadcasts");
    let keys_url = || format!("{base}/admin/api/environments/{env}/api-keys");
    let hmac_url = || format!("{base}/admin/api/environments/{env}/hmac/rotate");
    let users_url = || format!("{base}/admin/api/users");

    // read: every role can list environments.
    for c in [&viewer, &operator, &developer] {
        assert_eq!(get_status(c, envs_url()).await, 200);
    }
    // dlq:replay: operator only (of the three).
    assert_eq!(post_status(&viewer, dlq_url(), json!({})).await, 403);
    assert_eq!(post_status(&developer, dlq_url(), json!({})).await, 403);
    assert_eq!(post_status(&operator, dlq_url(), json!({})).await, 200);
    // broadcast:compose: operator only.
    assert_eq!(
        post_status(&viewer, bcast_url(), json!({"category": "x"})).await,
        403
    );
    assert_eq!(
        post_status(&developer, bcast_url(), json!({"category": "x"})).await,
        403
    );
    assert!(matches!(
        post_status(&operator, bcast_url(), json!({"category": "x"})).await,
        200 | 201
    ));
    // apikey:read / apikey:manage: developer only.
    assert_eq!(get_status(&viewer, keys_url()).await, 403);
    assert_eq!(get_status(&operator, keys_url()).await, 403);
    assert_eq!(get_status(&developer, keys_url()).await, 200);
    assert_eq!(
        post_status(&operator, keys_url(), json!({"name": "k"})).await,
        403
    );
    assert_eq!(
        post_status(&developer, keys_url(), json!({"name": "k"})).await,
        201
    );
    // env:create / hmac:rotate / user:manage: admin only (none of the three).
    for c in [&viewer, &operator, &developer] {
        assert_eq!(
            post_status(c, envs_url(), json!({"slug": "no", "name": "no"})).await,
            403
        );
        assert_eq!(post_status(c, hmac_url(), json!({})).await, 403);
        assert_eq!(get_status(c, users_url()).await, 403);
    }
}

/// developer/admin see the HMAC secret in the environment detail. viewer does not.
#[tokio::test]
async fn env_secret_is_gated_by_capability() {
    let app = support::spawn().await;
    let env = env_typeid(&app);
    for role in ["viewer", "developer"] {
        app.admin_post(
            "/admin/api/users",
            json!({"email": format!("{role}@secret.test"), "name": role, "role": role, "password": "password-123456"}),
        )
        .send()
        .await
        .unwrap();
    }
    let viewer = app
        .login_client("viewer@secret.test", "password-123456")
        .await;
    let developer = app
        .login_client("developer@secret.test", "password-123456")
        .await;
    let url = format!("{}/admin/api/environments/{env}", app.base);

    let v: Value = viewer.get(&url).send().await.unwrap().json().await.unwrap();
    assert!(
        v.get("subscriber_hmac_secret").is_none(),
        "viewer must not see the secret"
    );
    let d: Value = developer
        .get(&url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        d["subscriber_hmac_secret"]
            .as_str()
            .unwrap()
            .starts_with("shmac_"),
        "developer must see the secret"
    );
    // admin too.
    let a: Value = app
        .admin_get(&format!("/admin/api/environments/{env}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        a["subscriber_hmac_secret"]
            .as_str()
            .unwrap()
            .starts_with("shmac_")
    );
}

/// No self-disable, no self-delete, and never remove the last enabled admin.
#[tokio::test]
async fn guard_rails_self_and_last_admin() {
    let app = support::spawn().await;
    let me: Value = app
        .admin_get("/admin/api/me")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let my_id = me["id"].as_str().unwrap().to_owned();

    // Cannot disable or delete self.
    assert_eq!(
        app.admin_patch(
            &format!("/admin/api/users/{my_id}"),
            json!({"disabled": true})
        )
        .send()
        .await
        .unwrap()
        .status(),
        409
    );
    assert_eq!(
        app.admin_delete(&format!("/admin/api/users/{my_id}"))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    // Cannot demote the last enabled admin.
    assert_eq!(
        app.admin_patch(
            &format!("/admin/api/users/{my_id}"),
            json!({"role": "viewer"})
        )
        .send()
        .await
        .unwrap()
        .status(),
        409
    );

    // With a second admin, deleting one is allowed.
    let other: Value = app
        .admin_post(
            "/admin/api/users",
            json!({"email": "admin2@guard.test", "name": "A2", "role": "admin", "password": "password-123456"}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap().to_owned();
    assert_eq!(
        app.admin_delete(&format!("/admin/api/users/{other_id}"))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
}

/// Two admins demoted concurrently: the last-admin guard is transactional, so
/// exactly one wins and an enabled admin always remains (no TOCTOU lockout).
#[tokio::test]
async fn concurrent_admin_demotion_keeps_one_admin() {
    let app = support::spawn().await;
    let me: Value = app
        .admin_get("/admin/api/me")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let seed_id = me["id"].as_str().unwrap().to_owned();

    // A second admin, so exactly two enabled admins exist.
    let other: Value = app
        .admin_post(
            "/admin/api/users",
            json!({"email": "admin2@race.test", "name": "A2", "role": "admin", "password": "password-123456"}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap().to_owned();
    let other_client = app
        .login_client("admin2@race.test", "password-123456")
        .await;

    // Race two demotions toward zero admins. The other client demotes the seed
    // admin while the seed client demotes the other admin.
    let demote_seed = other_client
        .patch(format!("{}/admin/api/users/{seed_id}", app.base))
        .header("x-chimely-admin", "1")
        .json(&json!({"role": "viewer"}))
        .send();
    let demote_other = app
        .client
        .patch(format!("{}/admin/api/users/{other_id}", app.base))
        .header("x-chimely-admin", "1")
        .json(&json!({"role": "viewer"}))
        .send();
    let (r1, r2) = tokio::join!(demote_seed, demote_other);
    let statuses = [r1.unwrap().status().as_u16(), r2.unwrap().status().as_u16()];

    assert_eq!(
        statuses.iter().filter(|&&s| s == 200).count(),
        1,
        "exactly one demotion wins: {statuses:?}"
    );
    assert_eq!(
        statuses.iter().filter(|&&s| s == 409).count(),
        1,
        "the other is refused by the last-admin guard: {statuses:?}"
    );
    let enabled_admins: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_users WHERE role = 'admin' AND disabled_at IS NULL",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(enabled_admins >= 1, "an enabled admin must always remain");
}

/// Password reset (self-service) invalidates the old password.
#[tokio::test]
async fn password_change_then_relogin() {
    let app = support::spawn().await;
    app.admin_post(
        "/admin/api/users",
        json!({"email": "p@pw.test", "name": "P", "role": "viewer", "password": "old-password-123"}),
    )
    .send()
    .await
    .unwrap();
    let user = app.login_client("p@pw.test", "old-password-123").await;
    let me: Value = user
        .get(format!("{}/admin/api/me", app.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let uid = me["id"].as_str().unwrap().to_owned();

    // Self-service change.
    let chg = user
        .post(format!("{}/admin/api/users/{uid}/password", app.base))
        .header("x-chimely-admin", "1")
        .json(&json!({"password": "new-password-456"}))
        .send()
        .await
        .unwrap();
    assert_eq!(chg.status(), 204);

    // Old password no longer logs in, new one does.
    let old = reqwest::Client::new()
        .post(format!("{}/admin/api/login", app.base))
        .header("x-chimely-admin", "1")
        .json(&json!({"email": "p@pw.test", "password": "old-password-123"}))
        .send()
        .await
        .unwrap();
    assert_eq!(old.status(), 401);
    let _ = app.login_client("p@pw.test", "new-password-456").await;
}

/// An admin-forced password reset revokes the target's live sessions, so a
/// stolen session cannot outlive the reset.
#[tokio::test]
async fn password_reset_revokes_target_sessions() {
    let app = support::spawn().await;
    let created: Value = app
        .admin_post(
            "/admin/api/users",
            json!({"email": "victim@reset.test", "name": "V", "role": "viewer", "password": "old-password-123"}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let uid = created["id"].as_str().unwrap().to_owned();

    let victim = app
        .login_client("victim@reset.test", "old-password-123")
        .await;
    assert_eq!(
        get_status(&victim, format!("{}/admin/api/me", app.base)).await,
        200
    );

    // Admin resets the password.
    let reset = app
        .admin_post(
            &format!("/admin/api/users/{uid}/password"),
            json!({"password": "new-password-456"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(reset.status(), 204);

    // The victim's pre-reset session no longer authenticates.
    assert_eq!(
        get_status(&victim, format!("{}/admin/api/me", app.base)).await,
        401
    );
}

/// Passwords shorter than the minimum are rejected.
#[tokio::test]
async fn short_passwords_are_rejected() {
    let app = support::spawn().await;
    let res = app
        .admin_post(
            "/admin/api/users",
            json!({"email": "short@pw.test", "name": "S", "role": "viewer", "password": "short"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

/// Bootstrap-from-env creates the root admin once and is a no-op on re-run.
#[tokio::test]
async fn bootstrap_admin_is_idempotent() {
    let app = support::spawn_configured(false, |cfg| {
        cfg.admin_bootstrap_email = Some("root@boot.test".into());
        cfg.admin_bootstrap_password = Some("root-password-1234".into());
    })
    .await;

    chimely::bootstrap::ensure_admin(&app.pool, &app.cfg)
        .await
        .unwrap();
    let hash1: String =
        sqlx::query_scalar("SELECT password_hash FROM admin_users WHERE email = 'root@boot.test'")
            .fetch_one(&app.pool)
            .await
            .unwrap();

    // Second run: still one row, unchanged hash (a true no-op).
    chimely::bootstrap::ensure_admin(&app.pool, &app.cfg)
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM admin_users WHERE email = 'root@boot.test'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let hash2: String =
        sqlx::query_scalar("SELECT password_hash FROM admin_users WHERE email = 'root@boot.test'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(hash1, hash2, "idempotent boot must not re-hash");

    // The bootstrap admin can log in.
    let _ = app
        .login_client("root@boot.test", "root-password-1234")
        .await;
}

/// The CHIMELY_ADMIN_* bootstrap credentials authenticate through the real
/// login endpoint. A trailing newline in the env password (heredocs, `.env`
/// files, `export VAR=$(...)`) is trimmed at boot the way the email is, so the
/// operator signs in with the clean value instead of hitting a phantom 401.
#[tokio::test]
async fn bootstrap_admin_credentials_authenticate() {
    let app = support::spawn_configured(false, |cfg| {
        cfg.admin_bootstrap_email = Some("root@auth.test".into());
        cfg.admin_bootstrap_password = Some("root-password-1234\n".into());
    })
    .await;

    chimely::bootstrap::ensure_admin(&app.pool, &app.cfg)
        .await
        .unwrap();

    // login_client asserts a 200. The clean password authenticates even though
    // the configured env value carried a trailing newline.
    let _ = app
        .login_client("root@auth.test", "root-password-1234")
        .await;
}

/// Rotating the bootstrap credential (reconcile branch) revokes sessions that
/// predate the rotation, so a restart recovers from a compromise.
#[tokio::test]
async fn bootstrap_reconcile_revokes_existing_sessions() {
    let app = support::spawn_configured(false, |cfg| {
        cfg.admin_bootstrap_email = Some("root@reconcile.test".into());
        cfg.admin_bootstrap_password = Some("root-password-1234".into());
    })
    .await;
    chimely::bootstrap::ensure_admin(&app.pool, &app.cfg)
        .await
        .unwrap();

    let client = app
        .login_client("root@reconcile.test", "root-password-1234")
        .await;
    assert_eq!(
        get_status(&client, format!("{}/admin/api/me", app.base)).await,
        200
    );

    // Simulate credential drift (e.g. a UI password change) so the next boot
    // takes the reconcile branch.
    let drift = chimely::auth::hash_password("a-different-password").unwrap();
    sqlx::query("UPDATE admin_users SET password_hash = $1 WHERE email = 'root@reconcile.test'")
        .bind(drift)
        .execute(&app.pool)
        .await
        .unwrap();
    chimely::bootstrap::ensure_admin(&app.pool, &app.cfg)
        .await
        .unwrap();

    // The pre-rotation session is revoked. The env credential still logs in.
    assert_eq!(
        get_status(&client, format!("{}/admin/api/me", app.base)).await,
        401
    );
    let _ = app
        .login_client("root@reconcile.test", "root-password-1234")
        .await;
}

// =============================================================================
// Environment + API key lifecycle
// =============================================================================

#[tokio::test]
async fn environment_and_api_key_create_revoke_roundtrip() {
    let app = support::spawn().await;

    // Create an environment.
    let res = app
        .admin_post(
            "/admin/api/environments",
            json!({"slug": "dashboard-prod", "name": "Dashboard (prod)", "require_subscriber_hash": true}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let env: Value = res.json().await.unwrap();
    let env_id = env["id"].as_str().unwrap().to_owned();
    assert!(env_id.starts_with("env_"));
    assert!(
        env["subscriber_hmac_secret"]
            .as_str()
            .unwrap()
            .starts_with("shmac_")
    );
    assert_eq!(env["has_previous_secret"], json!(false));

    // It shows up in the list.
    let list: Value = app
        .admin_get("/admin/api/environments")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|e| e["slug"] == json!("dashboard-prod"))
    );

    // Duplicate slug is rejected.
    let dup = app
        .admin_post(
            "/admin/api/environments",
            json!({"slug": "dashboard-prod", "name": "again"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), 400);

    // Create an API key. Plaintext returned exactly once.
    let key_res = app
        .admin_post(
            &format!("/admin/api/environments/{env_id}/api-keys"),
            json!({"name": "ci"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(key_res.status(), 201);
    let key_body: Value = key_res.json().await.unwrap();
    let key_id = key_body["id"].as_str().unwrap().to_owned();
    let plaintext = key_body["key"].as_str().unwrap().to_owned();
    assert!(plaintext.starts_with("chml_live_"));
    assert!(key_id.starts_with("key_"));

    // The key authenticates the management plane for THIS environment.
    let notify = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .bearer_auth(&plaintext)
        .json(&json!({"subscriber_id": "usr_new", "category": "demo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(notify.status(), 201);

    // Listing shows the prefix, last_used_at, and NEVER the key.
    let keys: Value = app
        .admin_get(&format!("/admin/api/environments/{env_id}/api-keys"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key_row = keys
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["id"] == json!(key_id))
        .unwrap();
    assert_eq!(key_row["key_prefix"], json!(&plaintext[..14]));
    assert!(
        key_row.get("key").is_none(),
        "the plaintext key must never be listed"
    );
    assert!(
        key_row["last_used_at"].is_string(),
        "use should record last_used_at"
    );

    // Revoke it. The key 401s immediately on the management plane.
    let revoke = app
        .admin_post(
            &format!("/admin/api/environments/{env_id}/api-keys/{key_id}/revoke"),
            json!({}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(revoke.status(), 204);
    let after = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .bearer_auth(&plaintext)
        .json(&json!({"subscriber_id": "usr_new", "category": "demo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 401);

    // The revoked row is kept for audit with revoked_at set.
    let keys2: Value = app
        .admin_get(&format!("/admin/api/environments/{env_id}/api-keys"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let revoked_row = keys2
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["id"] == json!(key_id))
        .unwrap();
    assert!(revoked_row["revoked_at"].is_string());
}

// =============================================================================
// HMAC rotation (two-slot overlap) via the admin API
// =============================================================================

#[tokio::test]
async fn hmac_rotation_overlap_then_completion() {
    let app = support::spawn().await;
    let env = env_typeid(&app);
    let subscriber = "usr_rot";
    let old_secret = app.env.hmac_secret.clone();

    // The current secret authenticates a widget session.
    assert_eq!(
        counts_status_with_secret(&app, subscriber, &old_secret).await,
        200
    );

    // Begin rotation: a new current secret, old secret moved to previous.
    let rot: Value = app
        .admin_post(
            &format!("/admin/api/environments/{env}/hmac/rotate"),
            json!({}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let new_secret = rot["subscriber_hmac_secret"].as_str().unwrap().to_owned();
    assert_ne!(new_secret, old_secret);
    assert!(
        new_secret.starts_with("shmac_"),
        "rotation mints a shmac_-prefixed secret"
    );
    assert_eq!(rot["has_previous_secret"], json!(true));
    assert!(rot["subscriber_hmac_rotated_at"].is_string());

    // Overlap: BOTH secrets verify live sessions (zero-downtime rotation).
    assert_eq!(
        counts_status_with_secret(&app, subscriber, &old_secret).await,
        200
    );
    assert_eq!(
        counts_status_with_secret(&app, subscriber, &new_secret).await,
        200
    );

    // Rotation is observable in the environment view.
    let detail: Value = app
        .admin_get(&format!("/admin/api/environments/{env}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["subscriber_hmac_secret"], json!(new_secret));
    assert_eq!(detail["has_previous_secret"], json!(true));

    // Complete the rotation: the previous slot is cleared.
    let done = app
        .admin_post(
            &format!("/admin/api/environments/{env}/hmac/rotate/complete"),
            json!({}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(done.status(), 204);

    // Old secret now dies. The new one still works.
    assert_eq!(
        counts_status_with_secret(&app, subscriber, &old_secret).await,
        401
    );
    assert_eq!(
        counts_status_with_secret(&app, subscriber, &new_secret).await,
        200
    );
}

/// The secret prefix is cosmetic and never parsed: auth feeds the whole
/// secret string into HMAC-SHA256. A legacy `whsec_`-minted secret keeps
/// authenticating after the prefix rename, so existing customer secrets do
/// not need re-minting.
#[tokio::test]
async fn legacy_whsec_prefixed_secret_still_authenticates() {
    let app = support::spawn().await;
    let legacy_secret = "whsec_pre_rename_minted_secret";

    sqlx::query("UPDATE environments SET subscriber_hmac_secret = $1 WHERE id = $2")
        .bind(legacy_secret)
        .bind(app.env.id)
        .execute(&app.pool)
        .await
        .unwrap();

    assert_eq!(
        counts_status_with_secret(&app, "usr_legacy", legacy_secret).await,
        200,
        "a whsec_-prefixed secret must still verify"
    );
    // The prefix is not what grants access: a wrong secret is still rejected.
    assert_eq!(
        counts_status_with_secret(&app, "usr_legacy", "whsec_wrong").await,
        401
    );
}

// =============================================================================
// Broadcast composer
// =============================================================================

#[tokio::test]
async fn admin_broadcast_lands_as_one_row_and_respects_visibility() {
    let app = support::spawn().await;
    let env = env_typeid(&app);

    // A subscriber that exists BEFORE the broadcast sees it.
    let _ = app.counts("usr_before").await;

    let res = app
        .admin_post(
            &format!("/admin/api/environments/{env}/broadcasts"),
            json!({"category": "product.update", "payload": {"title": "Launch"}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let bcast: Value = res.json().await.unwrap();
    let bcast_id = bcast["id"].as_str().unwrap().to_owned();
    assert!(bcast_id.starts_with("bcast_"));

    // Exactly ONE row, never materialized per subscriber.
    let row_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM broadcasts WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(row_count, 1);

    // It appears in the pre-existing subscriber's merged list.
    let before_list = app.list_items("usr_before").await;
    assert!(
        before_list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == json!(bcast_id)),
        "pre-existing subscriber should see the broadcast"
    );

    // A subscriber created AFTER the broadcast does not see it.
    let _ = app.counts("usr_after").await;
    let after_list = app.list_items("usr_after").await;
    assert!(
        !after_list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == json!(bcast_id)),
        "later subscriber must not see an earlier broadcast"
    );
}

// =============================================================================
// Subscriber lookup: golden test against the subscriber plane
// =============================================================================

#[tokio::test]
async fn subscriber_lookup_inbox_matches_the_subscriber_plane() {
    let app = support::spawn().await;
    let env = env_typeid(&app);
    let subscriber = "usr_golden";

    // Make the subscriber exist first, then a mix of direct + broadcast.
    let _ = app.counts(subscriber).await;
    app.create_broadcast("announce.one").await;
    for i in 0..3 {
        app.create_notification(subscriber, &format!("cat.{i}"))
            .await;
    }
    app.create_broadcast("announce.two").await;
    app.drain_jobs().await;

    // Subscriber-plane truth.
    let plane_ids: Vec<String> = app
        .list_all_items(subscriber, 100)
        .await
        .into_iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();

    // Admin subscriber view runs the SAME canonical merge.
    let view: Value = app
        .admin_get(&format!(
            "/admin/api/environments/{env}/subscribers/{subscriber}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let admin_ids: Vec<String> = view["inbox"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();

    assert_eq!(
        admin_ids, plane_ids,
        "admin inbox must equal the subscriber plane list"
    );

    // Counters agree with the subscriber plane too.
    let (plane_unread, plane_unseen) = app.counts(subscriber).await;
    assert_eq!(view["counters"]["unread"].as_i64().unwrap(), plane_unread);
    assert_eq!(view["counters"]["unseen"].as_i64().unwrap(), plane_unseen);
    assert!(view["read_watermark"].is_string());
    assert!(view["seen_watermark"].is_string());
}

#[tokio::test]
async fn subscriber_lookup_404s_for_unknown_subscriber() {
    let app = support::spawn().await;
    let env = env_typeid(&app);
    let res = app
        .admin_get(&format!("/admin/api/environments/{env}/subscribers/nope"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

// =============================================================================
// DLQ replay through the normal claim path
// =============================================================================

#[tokio::test]
async fn dlq_replay_re_enters_the_claim_path_and_deletes_on_completion() {
    let app = support::spawn().await;

    // A subscriber to own a replayable counter_rebuild job.
    let _ = app.counts("usr_dlq").await;
    let internal: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM subscribers WHERE environment_id = $1 AND subscriber_id = 'usr_dlq'",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();

    // Park a job directly in the DLQ (as the worker would after exhausting
    // attempts).
    let job_id = ids::new_uuid();
    sqlx::query(
        "INSERT INTO dead_letters
             (environment_id, id, job_type, payload, attempts, max_attempts, last_error, created_at)
         VALUES ($1, $2, 'counter_rebuild', jsonb_build_object('subscriber_id', $3::text), 10, 10, 'boom', now())",
    )
    .bind(app.env.id)
    .bind(job_id)
    .bind(internal.to_string())
    .execute(&app.pool)
    .await
    .unwrap();

    // The admin DLQ browser lists it.
    let job_typeid = ids::typeid(ids::JOB, job_id);
    let dlq: Value = app
        .admin_get("/admin/api/dlq")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        dlq.as_array()
            .unwrap()
            .iter()
            .any(|d| d["id"] == json!(job_typeid)),
        "parked job should be listed"
    );

    // Replay via the admin API.
    let replay = app
        .admin_post(&format!("/admin/api/dlq/{job_typeid}/replay"), json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<Value>().await.unwrap()["replayed"], json!(1));

    // The DLQ row is gone (moved back into jobs).
    assert_eq!(app.dead_letter_count().await, 0);
    let in_jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id = $1 AND id = $2")
            .bind(app.env.id)
            .bind(job_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(in_jobs, 1, "replay re-enqueues into the normal claim path");

    // The normal worker loop completes it and DELETEs the row.
    app.drain_jobs().await;
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id = $1 AND id = $2")
            .bind(app.env.id)
            .bind(job_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0, "a completed job leaves no row");
}

#[tokio::test]
async fn dlq_replay_404s_for_unknown_job() {
    let app = support::spawn().await;
    let missing = ids::typeid(ids::JOB, ids::new_uuid());
    let res = app
        .admin_post(&format!("/admin/api/dlq/{missing}/replay"), json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

// =============================================================================
// Status / notification browser
// =============================================================================

#[tokio::test]
async fn status_browser_lists_notifications_and_their_timeline() {
    let app = support::spawn().await;
    let env = env_typeid(&app);
    let subscriber = "usr_status";

    let created = app.create_notification(subscriber, "payment.failed").await;
    let notif_id = created["notifications"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // Deliver the hint so delivered_hint appears in the timeline.
    app.drain_jobs().await;

    // The browser lists it, filtered by subscriber.
    let page: Value = app
        .admin_get(&format!(
            "/admin/api/environments/{env}/notifications?subscriber_id={subscriber}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed = page["items"].as_array().unwrap();
    assert!(listed.iter().any(|n| n["id"] == json!(notif_id)));
    let row = listed.iter().find(|n| n["id"] == json!(notif_id)).unwrap();
    assert_eq!(row["subscriber_id"], json!(subscriber));
    assert_eq!(row["category"], json!("payment.failed"));

    // The "did it send?" timeline shows created → delivered_hint.
    let timeline: Value = app
        .admin_get(&format!(
            "/admin/api/environments/{env}/notifications/{notif_id}/timeline"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let statuses: Vec<String> = timeline["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["status"].as_str().unwrap().to_owned())
        .collect();
    assert!(statuses.contains(&"created".to_owned()));
    assert!(statuses.contains(&"delivered_hint".to_owned()));

    // Category filter narrows results.
    let filtered: Value = app
        .admin_get(&format!(
            "/admin/api/environments/{env}/notifications?category=nonexistent"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(filtered["items"].as_array().unwrap().is_empty());
}

// =============================================================================
// Root path
// =============================================================================

/// The server root is a convenience redirect to the embedded admin dashboard.
/// The binary serves the app at /admin; the marketing site is hosted elsewhere.
/// Temporary (307), not permanent, so the mapping stays reversible and no
/// operator's browser caches / -> /admin forever.
///
/// The redirect is GET-only. axum get(...) registers GET (and auto HEAD) and
/// answers other verbs with 405, so a POST to / never redirects.
#[tokio::test]
async fn root_redirects_to_the_admin_dashboard() {
    let app = support::spawn().await;
    // reqwest follows redirects by default. A non-following client makes the
    // 3xx itself observable.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let res = client.get(format!("{}/", app.base)).send().await.unwrap();

    assert_eq!(res.status(), 307, "GET / is a temporary redirect");
    assert_eq!(
        res.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/admin"),
        "root redirects to the admin dashboard",
    );

    // Only GET redirects. get(...) answers non-GET verbs with 405, so a POST
    // to / returns Method Not Allowed and carries no Location.
    let res = client.post(format!("{}/", app.base)).send().await.unwrap();
    assert_eq!(res.status(), 405, "POST / is not a redirect");
    assert!(
        res.headers().get(reqwest::header::LOCATION).is_none(),
        "non-GET root does not redirect",
    );
}

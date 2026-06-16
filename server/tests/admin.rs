//! Phase 4 admin plane (specs/phase-4-admin.md acceptance criteria).
//!
//! Auth gating, environment/API-key lifecycle, HMAC two-slot rotation,
//! broadcast composing as ONE row, the subscriber-lookup golden test (admin
//! merged inbox == subscriber-plane API), DLQ replay through the normal claim
//! path, and the status-timeline browser.

mod support;

use dronte::auth::compute_subscriber_hash;
use dronte::ids;
use reqwest::header::HeaderValue;
use serde_json::{Value, json};

fn env_typeid(app: &support::TestApp) -> String {
    ids::typeid(ids::ENVIRONMENT, app.env.id)
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
        .header("X-Dronte-Environment", app.env.slug.clone())
        .header("X-Dronte-Subscriber", subscriber)
        .header("X-Dronte-Subscriber-Hash", hash)
        .send()
        .await
        .expect("counts")
        .status()
}

// =============================================================================
// Auth
// =============================================================================

#[tokio::test]
async fn every_admin_route_401s_without_the_credential() {
    let app = support::spawn().await;

    // API route, no credential.
    let res = app
        .client
        .get(format!("{}/admin/api/environments", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    // The 401 prompts the browser via Basic.
    assert_eq!(
        res.headers().get("www-authenticate"),
        Some(&HeaderValue::from_static(
            "Basic realm=\"dronte admin\", charset=\"UTF-8\""
        ))
    );

    // SPA shell, no credential.
    let spa = app
        .client
        .get(format!("{}/admin", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(spa.status(), 401);

    // A nested SPA route, no credential.
    let route = app
        .client
        .get(format!("{}/admin/environments", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(route.status(), 401);

    // Wrong password.
    let wrong = app
        .client
        .get(format!("{}/admin/api/environments", app.base))
        .basic_auth("admin", Some("not-the-token"))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    // Correct credential succeeds for both API and SPA.
    assert_eq!(
        app.admin_get("/admin/api/environments")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let spa_ok = app.admin_get("/admin").send().await.unwrap();
    assert_eq!(spa_ok.status(), 200);
    assert!(
        spa_ok
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    // A POST also gates (and a username other than the password is fine).
    let create_unauth = app
        .client
        .post(format!("{}/admin/api/environments", app.base))
        .json(&json!({"slug": "x", "name": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(create_unauth.status(), 401);
}

#[tokio::test]
async fn admin_plane_disabled_without_a_configured_token() {
    let app = support::spawn_configured(false, |cfg| cfg.admin_token = None).await;
    // Even a well-formed credential 401s when the plane is disabled.
    let res = app
        .admin_get("/admin/api/environments")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
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

    // Create an API key — plaintext returned exactly once.
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
    assert!(plaintext.starts_with("drnt_live_"));
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

    // Revoke it; the key 401s immediately on the management plane.
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

    // Old secret now dies; the new one still works.
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
// Subscriber lookup — golden test against the subscriber plane
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

//! Cross-environment isolation regression pack. environment_id is part of
//! every key and environments are the isolation unit. Every surface here was
//! audited sound. These tests pin the current behavior: subscriber
//! credentials, row probes, management reads, idempotency keys, admin key
//! management, SSE hints, subscriber lists, and DLQ replay never cross an
//! environment boundary.

mod support;

use std::time::Duration;

use chimely::auth::compute_subscriber_hash;
use chimely::ids;
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::json;
use support::{SseStream, TestApp, TestEnvironment};
use uuid::Uuid;

async fn create_notification_in(
    app: &TestApp,
    env: &TestEnvironment,
    subscriber: &str,
    category: &str,
) -> serde_json::Value {
    let res = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .bearer_auth(&env.api_key)
        .json(&json!({ "subscriber_id": subscriber, "category": category }))
        .send()
        .await
        .expect("create notification");
    assert_eq!(res.status(), 201, "create notification in {}", env.slug);
    res.json().await.expect("create notification body")
}

async fn create_broadcast_in(
    app: &TestApp,
    env: &TestEnvironment,
    category: &str,
) -> serde_json::Value {
    let res = app
        .client
        .post(format!("{}/v1/broadcasts", app.base))
        .bearer_auth(&env.api_key)
        .json(&json!({ "category": category }))
        .send()
        .await
        .expect("create broadcast");
    assert_eq!(res.status(), 201, "create broadcast in {}", env.slug);
    res.json().await.expect("create broadcast body")
}

async fn upsert_subscriber_in(app: &TestApp, env: &TestEnvironment, subscriber: &str) {
    let res = app
        .client
        .put(format!("{}/v1/subscribers/{subscriber}", app.base))
        .bearer_auth(&env.api_key)
        .send()
        .await
        .expect("upsert subscriber");
    assert_eq!(res.status(), 200, "upsert subscriber in {}", env.slug);
}

async fn counts_in(app: &TestApp, env: &TestEnvironment, subscriber: &str) -> (i64, i64) {
    let res = app
        .client
        .get(format!("{}/v1/inbox/counts", app.base))
        .headers(app.subscriber_headers_for(env, subscriber))
        .send()
        .await
        .expect("counts");
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("counts body");
    (
        body["unread"].as_i64().unwrap(),
        body["unseen"].as_i64().unwrap(),
    )
}

async fn list_items_in(
    app: &TestApp,
    env: &TestEnvironment,
    subscriber: &str,
) -> Vec<serde_json::Value> {
    let res = app
        .client
        .get(format!("{}/v1/inbox/items", app.base))
        .headers(app.subscriber_headers_for(env, subscriber))
        .send()
        .await
        .expect("list items");
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("list body");
    body["items"].as_array().expect("items").clone()
}

/// (read_at, unread_at, archived_at, unarchived_at) for one notification row.
async fn notification_flags(
    pool: &sqlx::PgPool,
    env: Uuid,
    id: Uuid,
) -> (
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
) {
    sqlx::query_as(
        "SELECT read_at, unread_at, archived_at, unarchived_at
           FROM notifications WHERE environment_id = $1 AND id = $2",
    )
    .bind(env)
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("notification flags")
}

async fn counter_updated_at(pool: &sqlx::PgPool, env: Uuid, subscriber: &str) -> DateTime<Utc> {
    sqlx::query_scalar(
        "SELECT c.updated_at FROM subscriber_counters c
           JOIN subscribers s ON s.environment_id = c.environment_id
                             AND s.id = c.subscriber_id
          WHERE c.environment_id = $1 AND s.subscriber_id = $2",
    )
    .bind(env)
    .bind(subscriber)
    .fetch_one(pool)
    .await
    .expect("counter updated_at")
}

/// A valid (subscriber_id, subscriber_hash) pair minted with env A's secret
/// is rejected when presented under env B's slug. Auth resolves the secret
/// from the presented slug, so env B can never verify env A's hash. The
/// rejection happens before the lazy subscriber upsert, so the probe leaves
/// no subscriber row behind in env B.
#[tokio::test]
async fn env_a_subscriber_hash_replayed_against_env_b_is_401() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;
    let sub = "usr_replay";
    let hash_a = compute_subscriber_hash(&app.env.hmac_secret, sub);

    // Control: the pair is a live credential in env A.
    let res = app
        .client
        .get(format!("{}/v1/inbox/items", app.base))
        .headers(app.subscriber_headers(sub))
        .send()
        .await
        .expect("env A control");
    assert_eq!(res.status(), 200, "the credential is valid in env A");

    let mut replayed = HeaderMap::new();
    replayed.insert(
        "X-Chimely-Environment",
        HeaderValue::from_str(&env_b.slug).unwrap(),
    );
    replayed.insert("X-Chimely-Subscriber", HeaderValue::from_str(sub).unwrap());
    replayed.insert(
        "X-Chimely-Subscriber-Hash",
        HeaderValue::from_str(&hash_a).unwrap(),
    );
    let res = app
        .client
        .get(format!("{}/v1/inbox/items", app.base))
        .headers(replayed)
        .send()
        .await
        .expect("cross-env replay");
    assert_eq!(res.status(), 401, "env B must reject env A's hash");
    let body: serde_json::Value = res.json().await.expect("error body");
    assert_eq!(body["error"]["code"], "unauthorized");

    let env_b_subscribers: i64 =
        sqlx::query_scalar("SELECT count(*) FROM subscribers WHERE environment_id = $1")
            .bind(env_b.id)
            .fetch_one(&app.pool)
            .await
            .expect("subscriber count");
    assert_eq!(
        env_b_subscribers, 0,
        "a rejected replay must not create a subscriber in env B"
    );
}

/// An env-A-authenticated subscriber probing REAL env-B ids through every
/// per-item mutation gets 404 and leaves env B untouched: the notification
/// row keeps its flags, no read or archive exception rows appear, the
/// victim's counts do not move, and no jobs are enqueued.
#[tokio::test]
async fn foreign_row_probes_return_404_and_leave_env_b_untouched() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;
    let victim = "usr_victim";

    let created = create_notification_in(&app, &env_b, victim, "victim.direct").await;
    let notif_id = created["notifications"][0]["id"].as_str().unwrap();
    let notif_uuid = ids::parse_typeid(ids::NOTIFICATION, notif_id).expect("notification uuid");
    let broadcast = create_broadcast_in(&app, &env_b, "victim.announce").await;
    let bcast_id = broadcast["id"].as_str().unwrap();
    app.drain_jobs().await;

    let counts_before = counts_in(&app, &env_b, victim).await;
    assert_eq!(counts_before.0, 2, "both env B items are unread");
    let updated_before = counter_updated_at(&app.pool, env_b.id, victim).await;

    for op in ["read", "unread", "archive", "unarchive"] {
        let res = app
            .post_inbox(
                "usr_attacker",
                &format!("/v1/inbox/notifications/{notif_id}/{op}"),
            )
            .await;
        assert_eq!(
            res.status(),
            404,
            "notification {op} must 404 across environments"
        );
        let res = app
            .post_inbox(
                "usr_attacker",
                &format!("/v1/inbox/broadcasts/{bcast_id}/{op}"),
            )
            .await;
        assert_eq!(
            res.status(),
            404,
            "broadcast {op} must 404 across environments"
        );
    }

    let flags = notification_flags(&app.pool, env_b.id, notif_uuid).await;
    assert_eq!(flags, (None, None, None, None), "env B row flags untouched");
    let exception_rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM broadcast_reads)
              + (SELECT count(*) FROM broadcast_archives)",
    )
    .fetch_one(&app.pool)
    .await
    .expect("exception rows");
    assert_eq!(exception_rows, 0, "no exception rows in any environment");

    assert_eq!(
        counts_in(&app, &env_b, victim).await,
        counts_before,
        "victim counts unchanged"
    );
    assert_eq!(
        counter_updated_at(&app.pool, env_b.id, victim).await,
        updated_before,
        "victim counters row untouched"
    );
    assert_eq!(
        app.job_count(env_b.id).await,
        0,
        "no jobs enqueued in env B"
    );
    assert_eq!(
        app.job_count(app.env.id).await,
        0,
        "no jobs enqueued in env A"
    );
}

/// The management timeline is scoped to the key's environment. Env A's API
/// key asking for a REAL env-B notification id gets 404 while env B's own
/// key reads it fine.
#[tokio::test]
async fn timeline_with_env_a_key_for_env_b_notification_is_404() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;

    let created = create_notification_in(&app, &env_b, "usr_timeline", "victim.direct").await;
    let notif_id = created["notifications"][0]["id"].as_str().unwrap();

    // timeline_api authenticates with env A's key.
    let res = app.timeline_api(notif_id).await;
    assert_eq!(
        res.status(),
        404,
        "env A's key must not read env B's timeline"
    );
    let body: serde_json::Value = res.json().await.expect("error body");
    assert_eq!(body["error"]["code"], "not_found");

    let res = app
        .client
        .get(format!("{}/v1/notifications/{notif_id}/timeline", app.base))
        .bearer_auth(&env_b.api_key)
        .send()
        .await
        .expect("env B timeline");
    assert_eq!(res.status(), 200, "env B's own key reads the timeline");
}

/// Idempotency keys are scoped per environment. The same key in env A and
/// env B creates two independent notifications with distinct ids, one row
/// per environment, and the env B request is never served env A's snapshot.
/// A same-env retry still replays the snapshot.
#[tokio::test]
async fn same_idempotency_key_in_two_envs_creates_independent_notifications() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;
    let key = "shared-cross-env-key";
    let body = json!({
        "subscriber_id": "usr_idem",
        "category": "cross.env",
        "idempotency_key": key,
    });

    let first = app
        .mgmt_post("/v1/notifications", body.clone())
        .send()
        .await
        .expect("env A create");
    assert_eq!(first.status(), 201);
    let first: serde_json::Value = first.json().await.expect("env A body");
    let id_a = first["notifications"][0]["id"].as_str().unwrap().to_owned();

    let second = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .bearer_auth(&env_b.api_key)
        .json(&body)
        .send()
        .await
        .expect("env B create");
    assert_eq!(
        second.status(),
        201,
        "env B must create, not replay env A's snapshot"
    );
    let second: serde_json::Value = second.json().await.expect("env B body");
    let id_b = second["notifications"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(id_a, id_b, "each environment mints its own notification");

    for env in [app.env.id, env_b.id] {
        let rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM notifications WHERE environment_id = $1")
                .bind(env)
                .fetch_one(&app.pool)
                .await
                .expect("notification count");
        assert_eq!(rows, 1, "one notification per environment");
    }
    let key_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM idempotency_keys WHERE idempotency_key = $1")
            .bind(key)
            .fetch_one(&app.pool)
            .await
            .expect("idempotency key count");
    assert_eq!(key_rows, 2, "the key is stored once per environment");

    // Same-env control: a retry inside env B replays env B's own snapshot.
    let replay = app
        .client
        .post(format!("{}/v1/notifications", app.base))
        .bearer_auth(&env_b.api_key)
        .json(&body)
        .send()
        .await
        .expect("env B replay");
    assert_eq!(replay.status(), 200, "same-env retry is a snapshot replay");
    let replay: serde_json::Value = replay.json().await.expect("replay body");
    assert_eq!(replay["notifications"][0]["id"].as_str().unwrap(), id_b);
}

/// Revoking env B's API key through a path parameterized with env A is 404.
/// The revoke UPDATE is keyed by (environment_id, key_id), so a mismatched
/// pair matches nothing and the key keeps authenticating.
#[tokio::test]
async fn revoke_with_mismatched_env_and_key_is_404_and_key_survives() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;
    let key_uuid: Uuid = sqlx::query_scalar("SELECT id FROM api_keys WHERE environment_id = $1")
        .bind(env_b.id)
        .fetch_one(&app.pool)
        .await
        .expect("env B key id");

    let env_a_id = ids::typeid(ids::ENVIRONMENT, app.env.id);
    let key_id = ids::typeid(ids::API_KEY, key_uuid);
    let res = app
        .admin_post(
            &format!("/admin/api/environments/{env_a_id}/api-keys/{key_id}/revoke"),
            json!({}),
        )
        .send()
        .await
        .expect("mismatched revoke");
    assert_eq!(res.status(), 404, "mismatched (env, key) pair must 404");

    let revoked_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT revoked_at FROM api_keys WHERE environment_id = $1 AND id = $2")
            .bind(env_b.id)
            .bind(key_uuid)
            .fetch_one(&app.pool)
            .await
            .expect("revoked_at");
    assert_eq!(revoked_at, None, "env B's key is not revoked");

    // The key still authenticates on the management plane.
    create_notification_in(&app, &env_b, "usr_survivor", "still.works").await;
}

/// SSE hints are environment-scoped. With the same external id subscribed in
/// both environments, a notification and an env-wide broadcast created in
/// env B produce no hint on env A's stream. The quiet window covers the
/// 250ms test debounce several times over, and drain_jobs proves env B's
/// hints were actually published rather than still pending. A control hint
/// in env A proves the stream itself is live.
#[tokio::test]
async fn env_b_activity_produces_no_hint_on_env_a_stream() {
    let app = support::spawn_with_redis().await;
    let env_b = app.create_environment(true).await;
    let sub = "usr_shared";
    upsert_subscriber_in(&app, &app.env, sub).await;
    upsert_subscriber_in(&app, &env_b, sub).await;

    let mut stream = SseStream::connect(&app, sub, None).await;

    create_notification_in(&app, &env_b, sub, "cross.direct").await;
    create_broadcast_in(&app, &env_b, "cross.announce").await;
    app.drain_jobs().await;
    assert_eq!(
        app.job_count(env_b.id).await,
        0,
        "env B hints were published"
    );

    assert!(
        stream
            .next_hint(Duration::from_millis(1500))
            .await
            .is_none(),
        "env B activity leaked a hint onto env A's stream"
    );

    // Control: the stream is live and receives env A's own hint.
    app.create_notification(sub, "local.direct").await;
    app.drain_jobs().await;
    let frame = stream
        .next_hint(Duration::from_secs(5))
        .await
        .expect("env A's own hint must arrive");
    assert!(frame.contains("event: hint"), "{frame}");
}

/// The same external subscriber id in two environments reads two disjoint
/// inboxes. Each list and count reflects only that environment's items.
#[tokio::test]
async fn same_external_id_sees_only_its_own_envs_items() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;
    let sub = "usr_shared";

    create_notification_in(&app, &app.env, sub, "from.env.a").await;
    create_notification_in(&app, &env_b, sub, "from.env.b").await;
    app.drain_jobs().await;

    let items_a = list_items_in(&app, &app.env, sub).await;
    assert_eq!(items_a.len(), 1, "env A sees exactly its own item");
    assert_eq!(items_a[0]["category"], "from.env.a");

    let items_b = list_items_in(&app, &env_b, sub).await;
    assert_eq!(items_b.len(), 1, "env B sees exactly its own item");
    assert_eq!(items_b[0]["category"], "from.env.b");

    assert_eq!(counts_in(&app, &app.env, sub).await.0, 1);
    assert_eq!(counts_in(&app, &env_b, sub).await.0, 1);
}

async fn park_dead_letter(pool: &sqlx::PgPool, env: Uuid, id: Uuid) {
    sqlx::query(
        "INSERT INTO dead_letters
             (environment_id, id, job_type, payload, attempts, max_attempts,
              last_error, progress_cursor, created_at)
         VALUES ($1, $2, 'counter_rebuild', '{\"subscriber_id\": \"x\"}'::jsonb,
                 10, 10, 'simulated outage', '{\"offset\": 3}'::jsonb, now())",
    )
    .bind(env)
    .bind(id)
    .execute(pool)
    .await
    .expect("park dead letter");
}

/// An env-scoped library replay (Some(env)) moves only that environment's
/// parked job. Env B's identically-shaped dead letter survives byte-for-byte
/// and no job appears in env B's queue. The unscoped HTTP replay path (None)
/// is tracked as issue #56 and deliberately not exercised here.
#[tokio::test]
async fn replay_all_scoped_to_env_a_leaves_env_b_identical_parked_job() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;

    park_dead_letter(&app.pool, app.env.id, ids::new_uuid()).await;
    park_dead_letter(&app.pool, env_b.id, ids::new_uuid()).await;

    type ParkedRow = (
        String,
        serde_json::Value,
        i32,
        i32,
        String,
        Option<serde_json::Value>,
    );
    let parked_before: ParkedRow = sqlx::query_as(
        "SELECT job_type, payload, attempts, max_attempts, last_error, progress_cursor
           FROM dead_letters WHERE environment_id = $1",
    )
    .bind(env_b.id)
    .fetch_one(&app.pool)
    .await
    .expect("env B parked row");

    let moved = chimely::dlq::replay_all(&app.pool, Some(app.env.id))
        .await
        .expect("replay env A");
    assert_eq!(moved, 1, "only env A's parked job is replayed");

    let parked_after: ParkedRow = sqlx::query_as(
        "SELECT job_type, payload, attempts, max_attempts, last_error, progress_cursor
           FROM dead_letters WHERE environment_id = $1",
    )
    .bind(env_b.id)
    .fetch_one(&app.pool)
    .await
    .expect("env B parked row after replay");
    assert_eq!(
        parked_after, parked_before,
        "env B's same-shape dead letter survives unchanged"
    );

    let env_a_parked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dead_letters WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .expect("env A parked count");
    assert_eq!(env_a_parked, 0, "env A's dead letter row is deleted");
    assert_eq!(
        app.job_count(app.env.id).await,
        1,
        "env A's job moved back to the queue"
    );
    assert_eq!(
        app.job_count(env_b.id).await,
        0,
        "no job materialized in env B"
    );
}

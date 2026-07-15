//! Dead-letter replay is scoped to one environment. environment_id is part of
//! every key. The dead_letters primary key is (environment_id, id). A replay
//! predicate matching bare job ids reaches across environments, so both
//! replay_all and replay-by-id must scope to an environment.
//!
//! Replaying environment A must leave environment B's dead_letters untouched.

mod support;

use serde_json::json;
use uuid::Uuid;

async fn park_dead_letter_with_id(pool: &sqlx::PgPool, env: Uuid, id: Uuid) {
    sqlx::query(
        "INSERT INTO dead_letters
             (environment_id, id, job_type, payload, attempts, max_attempts,
              last_error, progress_cursor, created_at)
         VALUES ($1, $2, 'counter_rebuild', '{\"subscriber_id\": \"x\"}'::jsonb,
                 10, 10, 'simulated outage', NULL, now())",
    )
    .bind(env)
    .bind(id)
    .execute(pool)
    .await
    .expect("park dead letter");
}

async fn dead_letters_in(pool: &sqlx::PgPool, env: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM dead_letters WHERE environment_id = $1")
        .bind(env)
        .fetch_one(pool)
        .await
        .expect("count dead letters")
}

#[tokio::test]
async fn replay_all_replays_only_the_scoped_environment() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;

    park_dead_letter_with_id(&app.pool, app.env.id, chimely::ids::new_uuid()).await;
    park_dead_letter_with_id(&app.pool, env_b.id, chimely::ids::new_uuid()).await;
    assert_eq!(dead_letters_in(&app.pool, app.env.id).await, 1);
    assert_eq!(dead_letters_in(&app.pool, env_b.id).await, 1);

    let moved = chimely::dlq::replay_all(&app.pool, Some(app.env.id))
        .await
        .expect("replay env A");
    assert_eq!(moved, 1, "only env A's parked job is replayed");

    assert_eq!(
        dead_letters_in(&app.pool, app.env.id).await,
        0,
        "env A's dead letter moved back to jobs"
    );
    assert_eq!(
        dead_letters_in(&app.pool, env_b.id).await,
        1,
        "env B's dead letter must survive an env-A-scoped replay-all"
    );
}

#[tokio::test]
async fn replay_by_id_replays_only_the_scoped_environment() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;

    // The same job id parked in both environments is legal under the
    // (environment_id, id) PK. An id-only replay predicate replays both copies.
    let shared = chimely::ids::new_uuid();
    park_dead_letter_with_id(&app.pool, app.env.id, shared).await;
    park_dead_letter_with_id(&app.pool, env_b.id, shared).await;

    let replayed = chimely::dlq::replay(&app.pool, shared, Some(app.env.id))
        .await
        .expect("replay env A's copy");
    assert_eq!(replayed, 1, "only env A's copy was replayed");

    assert_eq!(
        dead_letters_in(&app.pool, app.env.id).await,
        0,
        "env A's copy moved back to jobs"
    );
    assert_eq!(
        dead_letters_in(&app.pool, env_b.id).await,
        1,
        "env B's same-id dead letter must survive an env-A-scoped replay"
    );
}

#[tokio::test]
async fn admin_replay_honors_the_environment_query_param() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;

    let shared = chimely::ids::new_uuid();
    park_dead_letter_with_id(&app.pool, app.env.id, shared).await;
    park_dead_letter_with_id(&app.pool, env_b.id, shared).await;

    let job = chimely::ids::typeid(chimely::ids::JOB, shared);
    let res = app
        .admin_post(
            &format!("/admin/api/dlq/{job}/replay?environment={}", app.env.slug),
            json!({}),
        )
        .send()
        .await
        .expect("replay request");
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("replay body");
    assert_eq!(body["replayed"], json!(1));

    assert_eq!(
        dead_letters_in(&app.pool, app.env.id).await,
        0,
        "env A's copy moved back to jobs"
    );
    assert_eq!(
        dead_letters_in(&app.pool, env_b.id).await,
        1,
        "env B's same-id dead letter must survive an env-scoped HTTP replay"
    );
}

#[tokio::test]
async fn admin_replay_reports_the_actual_moved_count_when_unscoped() {
    let app = support::spawn().await;
    let env_b = app.create_environment(true).await;

    // Without an environment filter the admin path keeps its documented
    // cross-environment reach. The response must report every row it moved.
    let shared = chimely::ids::new_uuid();
    park_dead_letter_with_id(&app.pool, app.env.id, shared).await;
    park_dead_letter_with_id(&app.pool, env_b.id, shared).await;

    let job = chimely::ids::typeid(chimely::ids::JOB, shared);
    let res = app
        .admin_post(&format!("/admin/api/dlq/{job}/replay"), json!({}))
        .send()
        .await
        .expect("replay request");
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("replay body");
    assert_eq!(body["replayed"], json!(2), "both moved rows are counted");

    assert_eq!(dead_letters_in(&app.pool, app.env.id).await, 0);
    assert_eq!(dead_letters_in(&app.pool, env_b.id).await, 0);
}

#[tokio::test]
async fn admin_replay_404s_for_an_unknown_environment_slug() {
    let app = support::spawn().await;

    let id = chimely::ids::new_uuid();
    park_dead_letter_with_id(&app.pool, app.env.id, id).await;

    let job = chimely::ids::typeid(chimely::ids::JOB, id);
    let res = app
        .admin_post(
            &format!("/admin/api/dlq/{job}/replay?environment=no-such-env"),
            json!({}),
        )
        .send()
        .await
        .expect("replay request");
    assert_eq!(res.status(), 404);

    assert_eq!(
        dead_letters_in(&app.pool, app.env.id).await,
        1,
        "a rejected replay must not move the parked job"
    );
}

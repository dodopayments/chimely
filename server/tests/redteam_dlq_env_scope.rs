//! RED-TEAM regression guard: dead-letter replay is scoped to one environment.
//! environment_id is part of every key (CLAUDE.md). The dead_letters primary
//! key is (environment_id, id). The pre-fix replay predicate matched bare job
//! ids ("$1 IS NULL OR id = $1"), so a replay reached across environments. The
//! fix adds an environment scope to both replay_all and replay-by-id.
//!
//! Two environments each hold a parked job. Replaying ONLY environment A must
//! leave environment B's dead_letters untouched. The unscoped pre-fix predicate
//! replays B as well and these tests go red.

mod support;

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

    park_dead_letter_with_id(&app.pool, app.env.id, dronte::ids::new_uuid()).await;
    park_dead_letter_with_id(&app.pool, env_b.id, dronte::ids::new_uuid()).await;
    assert_eq!(dead_letters_in(&app.pool, app.env.id).await, 1);
    assert_eq!(dead_letters_in(&app.pool, env_b.id).await, 1);

    let moved = dronte::dlq::replay_all(&app.pool, Some(app.env.id))
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

    // The SAME job id parked in BOTH environments. The dead_letters PK is
    // (environment_id, id), so this is legal. An id-only replay predicate
    // would match (and replay) both copies.
    let shared = dronte::ids::new_uuid();
    park_dead_letter_with_id(&app.pool, app.env.id, shared).await;
    park_dead_letter_with_id(&app.pool, env_b.id, shared).await;

    let replayed = dronte::dlq::replay(&app.pool, shared, Some(app.env.id))
        .await
        .expect("replay env A's copy");
    assert!(replayed, "env A's copy was replayed");

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

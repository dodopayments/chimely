//! Task 1: migrations, typeid SQL helpers, partition maintenance, and the
//! migration lint (risk W2) — all against real Postgres via testcontainers.

mod support;

use chrono::{Datelike, Months, Utc};
use dronte::{db, ids, partitions};

#[tokio::test]
async fn migrations_are_idempotent_and_gate_readiness() {
    let app = support::spawn().await;
    // Boot already migrated; a second (racing-replica) run is a no-op.
    db::migrate(&app.pool).await.expect("re-running migrations");
    assert!(db::ready(&app.pool).await.expect("readiness query"));
}

#[tokio::test]
async fn sql_typeid_helpers_match_the_rust_implementation() {
    let app = support::spawn().await;
    for prefix in ["notif", "bcast", "env"] {
        for _ in 0..50 {
            let id = ids::new_uuid();
            let formatted: String = sqlx::query_scalar("SELECT typeid_format($1, $2)")
                .bind(id)
                .bind(prefix)
                .fetch_one(&app.pool)
                .await
                .expect("typeid_format");
            assert_eq!(formatted, ids::typeid(prefix, id));
            let parsed: uuid::Uuid = sqlx::query_scalar("SELECT typeid_parse($1)")
                .bind(&formatted)
                .fetch_one(&app.pool)
                .await
                .expect("typeid_parse");
            assert_eq!(parsed, id);
        }
    }
    // The canonical TypeID spec vector, end to end in SQL.
    let parsed: uuid::Uuid =
        sqlx::query_scalar("SELECT typeid_parse('notif_01h455vb4pex5vsknk084sn02q')")
            .fetch_one(&app.pool)
            .await
            .expect("spec vector parses");
    assert_eq!(parsed.to_string(), "01890a5d-ac96-774b-bcce-b302099a8057");
}

async fn partition_names(pool: &sqlx::PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT c.relname FROM pg_inherits i
           JOIN pg_class c ON c.oid = i.inhrelid
          WHERE i.inhparent = 'notifications'::regclass
          ORDER BY c.relname",
    )
    .fetch_all(pool)
    .await
    .expect("listing partitions")
}

#[tokio::test]
async fn partitions_cover_retention_through_thirteen_months_out() {
    let app = support::spawn().await;
    let names = partition_names(&app.pool).await;
    // [now - retention, now + 13] inclusive.
    assert_eq!(
        names.len() as u32,
        support::RETENTION_MONTHS + 13 + 1,
        "{names:?}"
    );

    let now = Utc::now();
    let current = format!("notifications_{:04}_{:02}", now.year(), now.month());
    let horizon = now + Months::new(13);
    let last = format!("notifications_{:04}_{:02}", horizon.year(), horizon.month());
    assert!(names.contains(&current), "missing current month: {names:?}");
    assert!(names.contains(&last), "missing 13-month horizon: {names:?}");
}

#[tokio::test]
async fn retention_detaches_and_drops_expired_partitions() {
    let app = support::spawn().await;
    let before = partition_names(&app.pool).await.len();
    // Re-run with a tighter retention window: the older partitions must be
    // DETACHed + DROPped, the rest untouched.
    partitions::run(&app.pool, 2, 30)
        .await
        .expect("maintenance with retention=2");
    let names = partition_names(&app.pool).await;
    assert_eq!(names.len() as u32, 2 + 13 + 1);
    assert!(names.len() < before);

    let now = Utc::now();
    let dropped = now - Months::new(3);
    let dropped = format!("notifications_{:04}_{:02}", dropped.year(), dropped.month());
    assert!(
        !names.contains(&dropped),
        "expired partition survived: {names:?}"
    );
}

#[tokio::test]
async fn month_boundary_insert_lands_in_a_precreated_partition() {
    let app = support::spawn().await;
    let next_month = Utc::now() + Months::new(1);
    let res = app
        .mgmt_post(
            "/v1/notifications",
            serde_json::json!({
                "subscriber_id": "usr_boundary",
                "category": "test",
                "deliver_at": next_month.to_rfc3339(),
            }),
        )
        .send()
        .await
        .expect("scheduled create");
    assert_eq!(res.status(), 201);

    let expected = format!(
        "notifications_{:04}_{:02}",
        next_month.year(),
        next_month.month()
    );
    let landed: String = sqlx::query_scalar(
        "SELECT n.tableoid::regclass::text FROM notifications n WHERE n.environment_id = $1",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .expect("partition lookup");
    assert_eq!(landed, expected);
}

/// Migration lint (risk W2): every table's PK/UNIQUE must include
/// environment_id (environments itself is the allowlisted root), and no
/// column may have a serial/sequence default (shard-readiness invariant 2).
#[tokio::test]
async fn migration_lint_environment_id_in_every_key_and_no_sequences() {
    let app = support::spawn().await;

    let offending_keys: Vec<String> = sqlx::query_scalar(
        r"SELECT c.conrelid::regclass::text || ' ' || c.conname
            FROM pg_constraint c
            JOIN pg_class t ON t.oid = c.conrelid
            JOIN pg_namespace n ON n.oid = t.relnamespace
           WHERE n.nspname = 'public'
             AND c.contype IN ('p', 'u')
             AND t.relname NOT IN ('environments', '_sqlx_migrations')
             AND t.relname NOT LIKE 'notifications_2%'  -- partitions inherit the parent PK
             AND NOT EXISTS (
                 SELECT 1 FROM unnest(c.conkey) AS k(attnum)
                   JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.attnum
                  WHERE a.attname = 'environment_id')",
    )
    .fetch_all(&app.pool)
    .await
    .expect("key lint query");
    assert!(
        offending_keys.is_empty(),
        "keys missing environment_id: {offending_keys:?}"
    );

    let sequence_defaults: Vec<String> = sqlx::query_scalar(
        r"SELECT t.relname || '.' || a.attname
            FROM pg_attribute a
            JOIN pg_class t ON t.oid = a.attrelid
            JOIN pg_namespace n ON n.oid = t.relnamespace
            LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
           WHERE n.nspname = 'public' AND t.relkind IN ('r', 'p') AND a.attnum > 0
             AND (a.attidentity <> '' OR pg_get_expr(d.adbin, d.adrelid) LIKE 'nextval%')",
    )
    .fetch_all(&app.pool)
    .await
    .expect("sequence lint query");
    assert!(
        sequence_defaults.is_empty(),
        "sequence defaults found: {sequence_defaults:?}"
    );
}

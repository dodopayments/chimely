//! Partition + retention maintenance for the monthly-partitioned tables
//! (risk W4): `notifications` and `notification_status_log`.
//!
//! Runs at boot and then daily, under a Postgres advisory lock so N replicas
//! never race on DDL. Pre-creates monthly partitions covering
//! `[now - retention, now + 13 months]` — 13 months because the API caps
//! `deliver_at` at 13 months out, and there is deliberately NO DEFAULT
//! partition: a missing partition is a loud insert error, never silent
//! unprunable growth. Retention is DETACH + DROP.
//!
//! Exposes the `dronte_partitions_remaining{table}` gauge: the number of
//! pre-created partitions still entirely in the future. The metrics sampler
//! recomputes it from pg_inherits on every sample, INDEPENDENTLY of this job
//! (`remaining_at`), so a stalled maintenance job shows the gauge decaying
//! by 1 per month instead of freezing at its last healthy value. **Alert at
//! < 2**: two months of headroom left means the job has been dead for ~11
//! months, and a stalled job plus exhausted headroom is a total write outage.
//!
//! Also purges aged idempotency snapshots (default 30 days), per the schema
//! contract ("purged by age via the maintenance job").

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use sqlx::{Connection, PgPool};

/// Advisory lock key for maintenance DDL ("drntPART" as big-endian i64).
const MAINTENANCE_LOCK_KEY: i64 = 0x64726e74_50415254;

/// How far ahead partitions are pre-created. Must cover the API's 13-month
/// `deliver_at` cap.
const HEADROOM_MONTHS: i32 = 13;

/// Every monthly-partitioned table. Each gets the same horizon and the same
/// retention window.
pub const PARTITIONED_TABLES: &[&str] = &["notifications", "notification_status_log"];

pub async fn run(
    pool: &PgPool,
    retention_months: u32,
    idempotency_retention_days: u32,
) -> anyhow::Result<()> {
    // A dedicated connection owns the advisory lock for the whole run;
    // session-scoped locks on pooled connections would leak across checkouts.
    let mut conn = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MAINTENANCE_LOCK_KEY)
        .execute(&mut *conn)
        .await?;
    let result = run_locked(&mut conn, retention_months, idempotency_retention_days).await;
    let unlocked = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MAINTENANCE_LOCK_KEY)
        .execute(&mut *conn)
        .await;
    if unlocked.is_err() {
        // A session lock on a connection returned to the pool would block
        // every future maintenance run. Close the connection instead: the
        // server releases its locks when the session ends.
        let _ = conn.detach().close().await;
    }
    result
}

async fn run_locked(
    conn: &mut sqlx::PgConnection,
    retention_months: u32,
    idempotency_retention_days: u32,
) -> anyhow::Result<()> {
    // DB clock, not app clock — the same rule as every ordering timestamp.
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *conn)
        .await?;
    let current = month_start(now);
    let from = add_months(current, -(retention_months as i32));
    let to = add_months(current, HEADROOM_MONTHS);

    for table in PARTITIONED_TABLES {
        let mut month = from;
        while month <= to {
            let next = add_months(month, 1);
            // Identifiers are derived from validated dates only, no user input.
            let ddl = format!(
                "CREATE TABLE IF NOT EXISTS {} PARTITION OF {table} \
                 FOR VALUES FROM ('{}+00') TO ('{}+00')",
                partition_name(table, month),
                month,
                next,
            );
            // AssertSqlSafe: identifiers/bounds come from validated dates only.
            sqlx::query(sqlx::AssertSqlSafe(ddl))
                .execute(&mut *conn)
                .await?;
            month = next;
        }

        // Retention: drop partitions whose entire range is older than the
        // window.
        for name in partition_names(&mut *conn, table).await? {
            let Some(start) = parse_partition_name(table, &name) else {
                tracing::warn!(partition = %name, parent = %table, "unrecognized partition; skipping");
                continue;
            };
            if add_months(start, 1) <= from {
                let mut tx = conn.begin().await?;
                // AssertSqlSafe: `name` matched the strict partition-name parse.
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "ALTER TABLE {table} DETACH PARTITION {name}"
                )))
                .execute(&mut *tx)
                .await?;
                sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE {name}")))
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                tracing::info!(partition = %name, parent = %table, "dropped expired partition");
            }
        }

        let future_partitions = remaining_at(&mut *conn, table, now).await?;
        metrics::gauge!("dronte_partitions_remaining", "table" => *table)
            .set(future_partitions as f64);
    }

    sqlx::query(
        "DELETE FROM idempotency_keys WHERE created_at < now() - make_interval(days => $1)",
    )
    .bind(idempotency_retention_days as i32)
    .execute(&mut *conn)
    .await?;

    // Admin sessions are server-side rows; drop the expired ones (the admin
    // multi-user auth design folds session GC into this job).
    sqlx::query("DELETE FROM admin_sessions WHERE expires_at < now()")
        .execute(&mut *conn)
        .await?;

    Ok(())
}

/// Pre-created partitions of `table` still entirely in the future relative
/// to `at`. Computed from pg_inherits on every call, never from maintenance
/// state: with the maintenance job stalled this decays by 1 per elapsed
/// month, which is exactly what the W4 alert needs to see.
pub async fn remaining_at(
    conn: &mut sqlx::PgConnection,
    table: &str,
    at: DateTime<Utc>,
) -> anyhow::Result<i64> {
    let current = month_start(at);
    let names = partition_names(conn, table).await?;
    Ok(names
        .iter()
        .filter_map(|n| parse_partition_name(table, n))
        .filter(|start| *start > current)
        .count() as i64)
}

async fn partition_names(
    conn: &mut sqlx::PgConnection,
    table: &str,
) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT c.relname FROM pg_inherits i
           JOIN pg_class c ON c.oid = i.inhrelid
          WHERE i.inhparent = $1::regclass",
    )
    .bind(table)
    .fetch_all(conn)
    .await?)
}

/// Boot ran `run()` already. This keeps it going daily.
pub async fn run_daily(pool: PgPool, retention_months: u32, idempotency_retention_days: u32) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await; // immediate first tick: boot already ran
    loop {
        interval.tick().await;
        if let Err(err) = run(&pool, retention_months, idempotency_retention_days).await {
            tracing::error!(error = ?err, "partition maintenance failed");
        }
    }
}

fn partition_name(table: &str, month: NaiveDate) -> String {
    format!("{table}_{:04}_{:02}", month.year(), month.month())
}

fn parse_partition_name(table: &str, name: &str) -> Option<NaiveDate> {
    let rest = name.strip_prefix(table)?.strip_prefix('_')?;
    let (y, m) = rest.split_once('_')?;
    NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, 1)
}

fn month_start(t: DateTime<Utc>) -> NaiveDate {
    NaiveDate::from_ymd_opt(t.year(), t.month(), 1).expect("valid month start")
}

fn add_months(d: NaiveDate, n: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + n;
    NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1)
        .expect("valid month arithmetic")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_arithmetic_handles_year_boundaries() {
        let jan = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(
            add_months(jan, -1),
            NaiveDate::from_ymd_opt(2025, 12, 1).unwrap()
        );
        assert_eq!(
            add_months(jan, 13),
            NaiveDate::from_ymd_opt(2027, 2, 1).unwrap()
        );
        assert_eq!(
            add_months(jan, -13),
            NaiveDate::from_ymd_opt(2024, 12, 1).unwrap()
        );
    }

    #[test]
    fn partition_names_round_trip() {
        let m = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
        assert_eq!(partition_name("notifications", m), "notifications_2026_06");
        assert_eq!(
            parse_partition_name("notifications", "notifications_2026_06"),
            Some(m)
        );
        assert_eq!(
            partition_name("notification_status_log", m),
            "notification_status_log_2026_06"
        );
        assert_eq!(
            parse_partition_name("notification_status_log", "notification_status_log_2026_06"),
            Some(m)
        );
        assert_eq!(parse_partition_name("notifications", "notifications"), None);
        // A status-log partition is not a notifications partition even though
        // the name shares the prefix shape.
        assert_eq!(
            parse_partition_name("notification_status_log", "notifications_2026_06"),
            None
        );
        assert_eq!(
            parse_partition_name("notifications", "broadcasts_2026_06"),
            None
        );
    }
}

//! Phase 3 observability: W3C trace context crosses the outbox. The
//! enqueuing span's traceparent is stored inside the job payload and the
//! worker adopts it as the remote parent, so ingest -> outbox -> worker ->
//! hint is ONE trace. cargo-nextest gives this test its own process, so the
//! global subscriber install below cannot leak elsewhere.

mod support;

use opentelemetry::trace::TracerProvider as _;
use serde_json::json;
use tracing::Instrument as _;
use tracing_subscriber::layer::SubscriberExt as _;

#[tokio::test]
async fn trace_context_rides_the_outbox_and_survives_processing() {
    // A real OTLP-style tracer (no exporter needed): spans get valid,
    // sampled contexts, exactly like a production deployment.
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    let tracer = provider.tracer("test");
    let subscriber =
        tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));
    tracing::subscriber::set_global_default(subscriber).expect("install subscriber");

    let app = support::spawn().await;
    app.create_notification("usr_tp", "x").await;
    let subscriber_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM subscribers WHERE environment_id = $1 AND subscriber_id = 'usr_tp'",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM jobs WHERE environment_id = $1")
        .bind(app.env.id)
        .execute(&app.pool)
        .await
        .unwrap();

    // Enqueue inside a sampled span, as a handler would.
    let span = tracing::info_span!("ingest.test");
    async {
        let mut conn = app.pool.acquire().await.unwrap();
        dronte::jobs::enqueue(
            &mut conn,
            app.env.id,
            dronte::jobs::TYPE_COUNTER_REBUILD,
            json!({ "subscriber_id": subscriber_id }),
            None,
        )
        .await
        .unwrap();
    }
    .instrument(span)
    .await;

    let payload: serde_json::Value =
        sqlx::query_scalar("SELECT payload FROM jobs WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    let traceparent = payload["_traceparent"]
        .as_str()
        .expect("traceparent stored in the outbox row");
    let parts: Vec<&str> = traceparent.split('-').collect();
    assert_eq!(parts.len(), 4, "W3C traceparent shape: {traceparent}");
    assert_eq!(parts[0], "00");
    assert_eq!(parts[1].len(), 32);
    assert_ne!(parts[1], "0".repeat(32), "valid trace id");
    assert_eq!(parts[2].len(), 16);

    // The worker adopts it and processes normally; the marker key must not
    // confuse any handler.
    app.drain_jobs().await;
    assert_eq!(app.job_count(app.env.id).await, 0);
    app.assert_consistent().await;
}

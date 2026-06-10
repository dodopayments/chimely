//! Router assembly: the v1 surface, health endpoints, Prometheus metrics,
//! and the Scalar-rendered API reference at /docs.

use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Router, middleware};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use utoipa_scalar::Servable as _;

use crate::api::{inbox, management, preferences, sse};
use crate::state::AppState;
use crate::{db, openapi};

pub fn router(state: AppState) -> Router {
    let prometheus = prometheus_handle();

    Router::new()
        // Management plane
        .route("/v1/notifications", post(management::create_notifications))
        .route("/v1/broadcasts", post(management::create_broadcast))
        .route(
            "/v1/subscribers/{subscriber_id}",
            put(management::upsert_subscriber),
        )
        .route(
            "/v1/subscribers/{subscriber_id}/preferences",
            get(preferences::get_subscriber_preferences)
                .put(preferences::set_subscriber_preferences),
        )
        // Subscriber plane
        .route("/v1/inbox/items", get(inbox::list_items))
        .route("/v1/inbox/counts", get(inbox::get_counts))
        .route(
            "/v1/inbox/notifications/{id}/read",
            post(inbox::mark_notification_read),
        )
        .route(
            "/v1/inbox/broadcasts/{id}/read",
            post(inbox::mark_broadcast_read),
        )
        .route("/v1/inbox/read-all", post(inbox::mark_all_read))
        .route("/v1/inbox/seen-all", post(inbox::mark_all_seen))
        .route(
            "/v1/inbox/preferences",
            get(preferences::get_inbox_preferences).put(preferences::set_inbox_preferences),
        )
        .route("/v1/inbox/stream", get(sse::stream))
        // Operational plane
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route(
            "/metrics",
            get(move || std::future::ready(prometheus.render())),
        )
        .merge(utoipa_scalar::Scalar::with_url("/docs", openapi::api_doc()))
        .layer(middleware::from_fn(access_log))
        .with_state(state)
}

/// The recorder is process-global. Tests build many routers in one process.
fn prometheus_handle() -> PrometheusHandle {
    static HANDLE: std::sync::OnceLock<PrometheusHandle> = std::sync::OnceLock::new();
    HANDLE
        .get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .expect("installing Prometheus recorder")
        })
        .clone()
}

/// Liveness: process is up.
async fn healthz() -> &'static str {
    "ok"
}

/// Readiness gates on Postgres reachable + migrations applied. Redis is the
/// hint/cache plane, degraded-OK and deliberately not readiness-fatal.
async fn readyz(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<&'static str, StatusCode> {
    match db::ready(&state.pool).await {
        Ok(true) => Ok("ok"),
        Ok(false) => Err(StatusCode::SERVICE_UNAVAILABLE),
        Err(err) => {
            tracing::warn!(error = ?err, "readiness probe failed");
            Err(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// Access log with credential scrubbing. `subscriber_hash` is a query-string
/// credential on the SSE endpoint (EventSource cannot set headers) and must
/// never reach log lines. This is a tested invariant.
async fn access_log(
    req: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let query = req.uri().query().map(scrub_query);
    let started = std::time::Instant::now();
    let response = next.run(req).await;
    tracing::info!(
        target: "dronte::access",
        %method,
        %path,
        query = query.as_deref().unwrap_or(""),
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "request"
    );
    response
}

/// Replaces the `subscriber_hash` value with a fixed marker. The match runs
/// on percent-DECODED names (auth decodes too, so `subscriber%5Fhash=` is a
/// valid credential and must scrub the same way) and the output is
/// re-encoded, so a credential value can never smuggle raw bytes into a log
/// line.
pub fn scrub_query(query: &str) -> String {
    form_urlencoded::parse(query.as_bytes())
        .map(|(name, value)| {
            let value = if name.eq_ignore_ascii_case("subscriber_hash") {
                std::borrow::Cow::Borrowed("redacted")
            } else {
                value
            };
            format!(
                "{}={}",
                form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>(),
                form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>(),
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_subscriber_hash_wherever_it_appears() {
        assert_eq!(
            scrub_query("environment=acme&subscriber_id=u1&subscriber_hash=deadbeef"),
            "environment=acme&subscriber_id=u1&subscriber_hash=redacted"
        );
        assert_eq!(scrub_query("subscriber_hash=x"), "subscriber_hash=redacted");
        assert_eq!(
            scrub_query("SUBSCRIBER_HASH=x&a=b"),
            "SUBSCRIBER_HASH=redacted&a=b"
        );
        // Percent-encoded names decode before matching: auth accepts
        // `subscriber%5Fhash`, so the scrub must catch it too.
        assert_eq!(
            scrub_query("subscriber%5Fhash=deadbeef"),
            "subscriber_hash=redacted"
        );
        assert_eq!(
            scrub_query("%73ubscriber_hash=deadbeef&a=b"),
            "subscriber_hash=redacted&a=b"
        );
        assert_eq!(scrub_query("a=b"), "a=b");
    }
}

//! HTTP plane scaffolding: health endpoints, Prometheus metrics, and the
//! Scalar-rendered API reference at /docs. The v1 API surface lands here in
//! Phase 1 (management + subscriber planes per specs/openapi.yaml).

use anyhow::Context;
use axum::{Router, routing::get};
use metrics_exporter_prometheus::PrometheusBuilder;
use utoipa_scalar::Servable as _;

use crate::openapi;

pub fn router() -> anyhow::Result<Router> {
    let prometheus = PrometheusBuilder::new()
        .install_recorder()
        .context("installing Prometheus recorder")?;

    Ok(Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route(
            "/metrics",
            get(move || std::future::ready(prometheus.render())),
        )
        .merge(utoipa_scalar::Scalar::with_url("/docs", openapi::api_doc())))
}

/// Liveness: process is up.
async fn healthz() -> &'static str {
    "ok"
}

/// Readiness. Phase 1 gates this on Postgres connectivity + migrations
/// applied; Redis is degraded-OK and deliberately NOT readiness-fatal
/// (hint/cache plane only — see CLAUDE.md).
async fn readyz() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_handlers_respond() {
        assert_eq!(healthz().await, "ok");
        assert_eq!(readyz().await, "ok");
    }
}

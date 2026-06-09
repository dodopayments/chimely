//! Tracing + OTLP wiring. Structured logs always; an OTLP trace exporter is
//! attached only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set, so local dev and
//! CI need no collector.

use anyhow::Context;
use opentelemetry::trace::TracerProvider as _;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init() -> anyhow::Result<()> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer();

    let otel_layer = if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok() {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .build()
            .context("building OTLP span exporter")?;
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(
                opentelemetry_sdk::Resource::builder()
                    .with_service_name("dronte")
                    .build(),
            )
            .build();
        let tracer = provider.tracer("dronte");
        Some(tracing_opentelemetry::layer().with_tracer(tracer))
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_layer)
        .try_init()
        .context("installing tracing subscriber")?;
    Ok(())
}

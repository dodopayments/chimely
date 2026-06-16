//! Tracing + OTLP wiring. Logs are JSON by default, `DRONTE_LOG_FORMAT=text`
//! for human eyes. The OTLP trace exporter attaches only when
//! `OTEL_EXPORTER_OTLP_ENDPOINT` is set, so local dev and CI need no collector.
//!
//! Trace context crosses the outbox as a W3C `traceparent` in the job payload
//! (`_traceparent`). `jobs::enqueue` injects it and the worker claims it as the
//! remote parent, so one trace spans ingest -> outbox -> worker -> hint publish.

use anyhow::Context;
use opentelemetry::trace::{
    SpanContext, SpanId, TraceContextExt as _, TraceFlags, TraceId, TraceState, TracerProvider as _,
};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init() -> anyhow::Result<()> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let json = std::env::var("DRONTE_LOG_FORMAT").as_deref() != Ok("text");
    let fmt_layer = if json {
        tracing_subscriber::fmt::layer().json().boxed()
    } else {
        tracing_subscriber::fmt::layer().boxed()
    };

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

/// The current span's context as a W3C traceparent. None when no sampled OTLP
/// trace is active, meaning no exporter is configured or there is no span.
pub fn current_traceparent() -> Option<String> {
    let ctx = tracing::Span::current().context();
    let binding = ctx.span();
    let sc = binding.span_context();
    if !sc.is_valid() {
        return None;
    }
    Some(format!(
        "00-{}-{}-{:02x}",
        sc.trace_id(),
        sc.span_id(),
        sc.trace_flags().to_u8()
    ))
}

/// Adopt `traceparent` as the remote parent of `span`. Malformed input and
/// registry errors are ignored. Tracing is observability, never a failure
/// source.
pub fn set_remote_parent(span: &tracing::Span, traceparent: &str) {
    if let Some(ctx) = parse_traceparent(traceparent) {
        span.set_parent(ctx).ok();
    }
}

fn parse_traceparent(s: &str) -> Option<opentelemetry::Context> {
    let mut parts = s.split('-');
    if parts.next()? != "00" {
        return None;
    }
    let trace_id = TraceId::from_hex(parts.next()?).ok()?;
    let span_id = SpanId::from_hex(parts.next()?).ok()?;
    let flags = u8::from_str_radix(parts.next()?, 16).ok()?;
    let sc = SpanContext::new(
        trace_id,
        span_id,
        TraceFlags::new(flags),
        true,
        TraceState::default(),
    );
    if !sc.is_valid() {
        return None;
    }
    Some(opentelemetry::Context::new().with_remote_span_context(sc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceparent_parses_and_rejects() {
        let ctx =
            parse_traceparent("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01").unwrap();
        let binding = ctx.span();
        let sc = binding.span_context();
        assert_eq!(
            sc.trace_id().to_string(),
            "0af7651916cd43dd8448eb211c80319c"
        );
        assert_eq!(sc.span_id().to_string(), "b7ad6b7169203331");
        assert!(sc.is_sampled());
        assert!(sc.is_remote());

        assert!(parse_traceparent("garbage").is_none());
        assert!(
            parse_traceparent("01-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01").is_none()
        );
        // All-zero ids are invalid per W3C.
        assert!(
            parse_traceparent("00-00000000000000000000000000000000-b7ad6b7169203331-01").is_none()
        );
    }
}

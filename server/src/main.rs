//! Dronte — in-app notification inbox infrastructure.
//!
//! Single binary, two entrypoints:
//!   `dronte serve`   (default) — API plane + workers
//!   `dronte openapi`           — print the utoipa-generated OpenAPI spec to
//!                                stdout (the artifact CI diffs against
//!                                specs/openapi.yaml until v1; see CLAUDE.md)

mod http;
mod openapi;
mod telemetry;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        // Spec export must stay side-effect free: no runtime, no sockets, no
        // tracing init — CI and the docs pipeline call this in tight loops.
        Some("openapi") => {
            let yaml = openapi::api_doc()
                .to_yaml()
                .context("serializing OpenAPI document")?;
            print!("{yaml}");
            Ok(())
        }
        Some("serve") | None => {
            telemetry::init()?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("building tokio runtime")?
                .block_on(serve())
        }
        Some(other) => {
            eprintln!("unknown subcommand: {other}\nusage: dronte [serve|openapi]");
            std::process::exit(2);
        }
    }
}

async fn serve() -> anyhow::Result<()> {
    let addr = std::env::var("DRONTE_LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "dronte listening");

    axum::serve(listener, http::router()?)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")
}

/// Graceful shutdown contract (Phase 1 expands this): stop claiming jobs,
/// finish in-flight work, close SSE streams with a jittered `retry:` hint.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("installing ctrl-c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("installing SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown signal received");
}

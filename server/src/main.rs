//! Dronte — in-app notification inbox infrastructure.
//!
//! Single binary, two entrypoints:
//!   `dronte serve`   (default) — API plane + workers
//!   `dronte openapi`           — print the utoipa-generated OpenAPI spec to
//!                                stdout (the artifact CI diffs against
//!                                specs/openapi.yaml until v1; see CLAUDE.md)

use std::sync::Arc;

use anyhow::Context;
use dronte::{config, db, http, openapi, partitions, pubsub, state, telemetry, worker};

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
    let cfg = Arc::new(config::Config::from_env()?);

    let pool = db::connect(&cfg.database_url)
        .await
        .context("connecting to Postgres")?;
    db::migrate(&pool).await.context("running migrations")?;
    partitions::run(&pool, cfg.retention_months, cfg.idempotency_retention_days)
        .await
        .context("boot partition maintenance")?;

    let pubsub = pubsub::build(cfg.redis_url.as_deref(), &pool)
        .await
        .context("connecting the hint plane")?;
    if cfg.redis_url.is_none() {
        tracing::warn!(
            "Redis-less mode: hints ride Postgres LISTEN/NOTIFY on a dedicated direct \
             connection. Transaction-mode PgBouncer breaks LISTEN — DATABASE_URL must \
             point at Postgres directly (or a session-mode pooler)."
        );
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let app_state = state::AppState::new(
        pool.clone(),
        cfg.clone(),
        pubsub.clone(),
        shutdown_rx.clone(),
    );

    tokio::spawn(partitions::run_daily(
        pool.clone(),
        cfg.retention_months,
        cfg.idempotency_retention_days,
    ));
    let worker_handle = tokio::spawn(worker::run(
        pool.clone(),
        pubsub,
        cfg.clone(),
        shutdown_rx.clone(),
    ));

    let listener = tokio::net::TcpListener::bind(&cfg.listen_addr)
        .await
        .with_context(|| format!("binding {}", cfg.listen_addr))?;
    tracing::info!(addr = %cfg.listen_addr, "dronte listening");

    // Graceful shutdown: flip the watch (workers stop claiming; SSE streams
    // send a jittered `retry:` and close), then stop accepting.
    let mut shutdown_rx_for_serve = shutdown_rx.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        shutdown_tx.send(true).ok();
    });
    axum::serve(listener, http::router(app_state))
        .with_graceful_shutdown(async move {
            shutdown_rx_for_serve.changed().await.ok();
        })
        .await
        .context("server error")?;

    worker_handle.await.ok();
    Ok(())
}

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

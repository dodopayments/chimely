//! Dronte — in-app notification inbox infrastructure.
//!
//! Single binary, three entrypoints:
//!   `dronte serve`   (default) — API plane + workers
//!   `dronte openapi`           — print the utoipa-generated OpenAPI spec to
//!                                stdout (the artifact CI diffs against
//!                                specs/openapi.yaml until v1; see CLAUDE.md)
//!   `dronte dlq`               lists and replays dead-lettered jobs

use std::sync::Arc;

use anyhow::Context;
use dronte::{
    bootstrap, config, db, dlq, http, ids, metrics_sampler, openapi, partitions, pubsub, ratelimit,
    state, telemetry, worker,
};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        // Spec export must stay side-effect free: no runtime, no sockets, no
        // tracing init — CI and the docs pipeline call this in tight loops.
        Some("openapi") => {
            let yaml = openapi::to_contract_yaml().context("serializing OpenAPI document")?;
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
        Some("dlq") => {
            telemetry::init()?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("building tokio runtime")?
                .block_on(dlq_command(args.collect()))
        }
        Some(other) => {
            eprintln!("unknown subcommand: {other}\nusage: dronte [serve|openapi|dlq]");
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
    bootstrap::run(&pool, &cfg)
        .await
        .context("dev environment bootstrap")?;
    bootstrap::ensure_admin(&pool, &cfg)
        .await
        .context("admin bootstrap")?;
    if !cfg.admin_tls_terminated {
        tracing::warn!(
            "Admin session cookies require TLS. This binary serves plain HTTP: terminate TLS at a \
             proxy and set DRONTE_ADMIN_TLS_TERMINATED=true. Until then the session cookie omits \
             its Secure attribute and admin access is exposed if served over plain HTTP."
        );
    }

    let pubsub = pubsub::build(cfg.redis_url.as_deref(), &pool)
        .await
        .context("connecting the hint plane")?;
    let ratelimit = ratelimit::build(cfg.redis_url.as_deref())
        .await
        .context("connecting the rate limiter")?;
    if cfg.redis_url.is_none() {
        tracing::warn!(
            "Redis-less mode: hints ride Postgres LISTEN/NOTIFY on a dedicated direct \
             connection (transaction-mode PgBouncer breaks LISTEN — DATABASE_URL must \
             point at Postgres directly or a session-mode pooler), and rate limits are \
             per-replica, not cross-replica."
        );
    }

    let (draining_tx, draining_rx) = tokio::sync::watch::channel(false);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let app_state = state::AppState::new(
        pool.clone(),
        cfg.clone(),
        pubsub.clone(),
        ratelimit,
        draining_rx,
        shutdown_rx.clone(),
    );

    tokio::spawn(partitions::run_daily(
        pool.clone(),
        cfg.retention_months,
        cfg.idempotency_retention_days,
    ));
    tokio::spawn(metrics_sampler::run(
        pool.clone(),
        cfg.clone(),
        shutdown_rx.clone(),
    ));
    let mut worker_handle = tokio::spawn(worker::run(
        pool.clone(),
        pubsub,
        cfg.clone(),
        shutdown_rx.clone(),
    ));

    let listener = tokio::net::TcpListener::bind(&cfg.listen_addr)
        .await
        .with_context(|| format!("binding {}", cfg.listen_addr))?;
    tracing::info!(addr = %cfg.listen_addr, "dronte listening");

    // Graceful shutdown, two phases:
    //   1. readiness flips to 503 while the listener KEEPS serving, so load
    //      balancers drain the replica without dropping in-flight requests;
    //   2. after the grace period the shutdown watch flips: workers stop
    //      claiming, SSE streams send a jittered `retry:` and close, the
    //      listener stops accepting.
    let grace = cfg.shutdown_readiness_grace;
    let mut shutdown_rx_for_serve = shutdown_rx.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        draining_tx.send(true).ok();
        tracing::info!(grace = ?grace, "draining: readiness now 503; listener closes after grace");
        tokio::time::sleep(grace).await;
        shutdown_tx.send(true).ok();
    });
    axum::serve(listener, http::router(app_state))
        .with_graceful_shutdown(async move {
            shutdown_rx_for_serve.changed().await.ok();
        })
        .await
        .context("server error")?;

    // Phase 3 of the drain: the worker finishes its in-flight sweep within a
    // deadline. Past it, abort: the open transaction rolls back and the job
    // is re-claimed by the next replica (at-least-once by design).
    if tokio::time::timeout(cfg.shutdown_drain_deadline, &mut worker_handle)
        .await
        .is_err()
    {
        tracing::warn!(
            deadline = ?cfg.shutdown_drain_deadline,
            "worker drain deadline exceeded; aborting in-flight job (replay-safe)"
        );
        worker_handle.abort();
    }
    Ok(())
}

async fn dlq_command(args: Vec<String>) -> anyhow::Result<()> {
    let cfg = config::Config::from_env()?;
    let pool = db::connect(&cfg.database_url)
        .await
        .context("connecting to Postgres")?;

    // `--env <slug>` pins replay to one environment. environment_id is part
    // of every key; an unscoped id match would reach across environments.
    let mut args = args;
    let environment = match args.iter().position(|a| a == "--env") {
        Some(i) if i + 1 < args.len() => {
            args.remove(i);
            let slug = args.remove(i);
            match dlq::environment_by_slug(&pool, &slug).await? {
                Some(id) => Some(id),
                None => {
                    eprintln!("no such environment: {slug}");
                    std::process::exit(1);
                }
            }
        }
        Some(_) => {
            eprintln!("usage: dronte dlq replay <job_id|--all> [--env <slug>]");
            std::process::exit(2);
        }
        None => None,
    };

    match args.first().map(String::as_str) {
        Some("list") => {
            let letters = dlq::list(&pool).await?;
            if letters.is_empty() {
                println!("dead-letter queue is empty");
                return Ok(());
            }
            for l in letters {
                println!(
                    "{}\t{}\t{}\tattempts={}\tparked_at={}\t{}",
                    l.typeid(),
                    l.environment_slug,
                    l.job_type,
                    l.attempts,
                    l.parked_at.to_rfc3339(),
                    l.last_error.lines().next().unwrap_or(""),
                );
            }
            Ok(())
        }
        Some("replay") => match args.get(1).map(String::as_str) {
            Some("--all") => {
                let moved = dlq::replay_all(&pool, environment).await?;
                println!("replayed {moved} job(s)");
                Ok(())
            }
            Some(id) => {
                let id = ids::parse_typeid(ids::JOB, id)
                    .or_else(|| id.parse().ok())
                    .context("expected a job_… TypeID or a raw UUID")?;
                if dlq::replay(&pool, id, environment).await? {
                    println!("replayed {}", ids::typeid(ids::JOB, id));
                } else {
                    eprintln!("no such dead letter");
                    std::process::exit(1);
                }
                Ok(())
            }
            None => {
                eprintln!("usage: dronte dlq replay <job_id|--all> [--env <slug>]");
                std::process::exit(2);
            }
        },
        _ => {
            eprintln!("usage: dronte dlq [list|replay <job_id|--all> [--env <slug>]]");
            std::process::exit(2);
        }
    }
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

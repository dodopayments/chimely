//! `chimely dev`: a zero-config local launcher.
//!
//! Boots an ephemeral embedded Postgres (postgresql_embedded, theseus binaries
//! cached under the user data dir on first run) and runs the normal serve path
//! in Redis-less mode, where hints ride Postgres LISTEN/NOTIFY. It seeds a `dev`
//! environment with a copy-pasteable API key and a root admin, prints a banner,
//! and discards the database on exit. Compiles only under the `dev` feature.

use anyhow::Context as _;
use postgresql_embedded::{PostgreSQL, Settings};

/// Ports, generated credentials, and embedded-Postgres settings, decided up
/// front. Computed synchronously so the environment is fully exported before
/// any runtime or telemetry thread starts.
pub struct Plan {
    settings: Settings,
    pub server_addr: String,
    pub api_key: String,
    pub admin_email: String,
    pub admin_password: String,
    pub database_url: String,
}

/// Pick ports, generate credentials, and export the environment `serve()` reads.
///
/// Must run in `main()` before the tokio runtime and telemetry initialise: it
/// mutates the process environment, which is only sound while single-threaded.
pub fn plan() -> anyhow::Result<Plan> {
    let pg_port = free_port().context("allocating a Postgres port")?;
    let server_port = free_port().context("allocating a server port")?;
    let server_addr = format!("127.0.0.1:{server_port}");
    let api_key = format!("chimely_dev_{}", token());
    let admin_email = "admin@chimely.dev".to_string();
    let admin_password = token();

    let settings = Settings {
        host: "127.0.0.1".to_string(),
        port: pg_port,
        username: "postgres".to_string(),
        password: "postgres".to_string(),
        temporary: true,
        // Governs each embedded-Postgres command (initdb, pg_ctl). The crate
        // default is 5s, which initdb blows past on a cold or busy machine.
        timeout: Some(std::time::Duration::from_secs(120)),
        ..Settings::default()
    };
    let database_url = settings.url("chimely");

    // SAFETY: `plan()` runs in `main()` before the tokio runtime, telemetry, or
    // any worker thread exists, so nothing can read the environment
    // concurrently. `serve()` consumes these via `Config::from_env()`.
    unsafe {
        std::env::set_var("DATABASE_URL", &database_url);
        std::env::remove_var("REDIS_URL");
        std::env::set_var("CHIMELY_LISTEN_ADDR", &server_addr);
        std::env::set_var("CHIMELY_DEV_ENVIRONMENT", "dev");
        std::env::set_var("CHIMELY_DEV_API_KEY", &api_key);
        std::env::set_var("CHIMELY_ADMIN_EMAIL", &admin_email);
        std::env::set_var("CHIMELY_ADMIN_PASSWORD", &admin_password);
        std::env::set_var("CHIMELY_ADMIN_TLS_TERMINATED", "false");
        // Snappy Ctrl-C for a local launcher: no load balancer to drain.
        std::env::set_var("CHIMELY_SHUTDOWN_GRACE_MS", "0");
    }

    Ok(Plan {
        settings,
        server_addr,
        api_key,
        admin_email,
        admin_password,
        database_url,
    })
}

/// Download (first run only), initialise, and start the embedded Postgres, then
/// ensure the `chimely` database exists. The returned handle stops and discards
/// the instance on `stop()` or drop.
pub async fn start_postgres(plan: &Plan) -> anyhow::Result<PostgreSQL> {
    let mut pg = PostgreSQL::new(plan.settings.clone());
    eprintln!("Starting embedded Postgres (first run downloads it, then it is cached)...");
    pg.setup()
        .await
        .map_err(|e| anyhow::anyhow!("embedded Postgres setup failed: {e}"))?;
    pg.start()
        .await
        .map_err(|e| anyhow::anyhow!("starting embedded Postgres failed: {e}"))?;
    let exists = pg
        .database_exists("chimely")
        .await
        .map_err(|e| anyhow::anyhow!("checking for the chimely database failed: {e}"))?;
    if !exists {
        pg.create_database("chimely")
            .await
            .map_err(|e| anyhow::anyhow!("creating the chimely database failed: {e}"))?;
    }
    Ok(pg)
}

/// Human-facing banner: server URL, the ready-to-paste API key, and the admin
/// login. Printed to stderr so stdout stays usable for piping.
pub fn print_banner(plan: &Plan) {
    let url = format!("http://{}", plan.server_addr);
    eprintln!();
    eprintln!("  Chimely dev server is up.");
    eprintln!();
    eprintln!("    API + docs   {url}");
    eprintln!(
        "    Admin UI     {url}/admin   ({} / {})",
        plan.admin_email, plan.admin_password
    );
    eprintln!("    Environment  dev   (subscriber hashes not required)");
    eprintln!("    API key      {}", plan.api_key);
    eprintln!();
    eprintln!("  Send your first notification:");
    eprintln!();
    eprintln!("    curl -X POST {url}/v1/notifications \\");
    eprintln!("      -H 'Authorization: Bearer {}' \\", plan.api_key);
    eprintln!("      -d '{{\"subscriber_id\":\"usr_1\",\"category\":\"welcome\"}}'");
    eprintln!();
    eprintln!("  Ephemeral Postgres, Redis-less. Ctrl-C to stop (data is discarded).");
    eprintln!();
}

/// Bind an ephemeral port on loopback and hand back the number the OS chose.
/// The listener is dropped immediately; a brief TOCTOU window is acceptable for
/// a local dev launcher.
fn free_port() -> anyhow::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// A short random lowercase-hex token for dev credentials. Long enough to clear
/// the admin password minimum and to be unguessable on a loopback bind.
fn token() -> String {
    use rand::Rng as _;
    let mut rng = rand::rng();
    (0..24)
        .map(|_| char::from_digit(rng.random_range(0..16), 16).expect("nibble is < 16"))
        .collect()
}

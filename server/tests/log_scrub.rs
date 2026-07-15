//! Identifier scrubbing across the config -> middleware -> output boundary.
//! A router configured with `log_scrub_identifiers` serves real requests and
//! the tests assert on captured access-log lines and `http.request` span
//! fields. nextest gives each test its own process, so the global subscriber
//! install below cannot leak elsewhere.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::Digest as _;
use support::SseStream;
use tracing_subscriber::fmt::format::FmtSpan;

/// First 12 hex chars of SHA-256("usr_123"), computed outside this codebase
/// (shasum -a 256).
const HASH_USR_123: &str = "ca010ec7feb3";

/// Captures all log output for the process. Span-open events are enabled so
/// the `http.request` span's fields appear in the capture alongside the
/// access-log events.
fn install_log_capture() -> Arc<Mutex<Vec<u8>>> {
    struct BufWriter(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let logs = Arc::new(Mutex::new(Vec::new()));
    let sink = logs.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_span_events(FmtSpan::NEW)
        .with_ansi(false)
        .with_writer(move || BufWriter(sink.clone()))
        .try_init()
        .expect("install capture subscriber");
    logs
}

/// Drives one request carrying the subscriber id in the path and one
/// carrying subscriber and environment identifiers in the query string,
/// then returns the captured log output.
async fn capture_identifier_requests(app: &support::TestApp) -> String {
    let logs = install_log_capture();

    let res = app
        .client
        .put(format!("{}/v1/subscribers/usr_123", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .expect("upsert request");
    assert_eq!(res.status(), 200);

    let mut stream = SseStream::connect(app, "usr_123", None).await;
    stream.next_frame(Duration::from_secs(2)).await;
    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;

    String::from_utf8_lossy(&logs.lock().unwrap()).to_string()
}

fn find_line<'a>(captured: &'a str, needles: &[&str]) -> &'a str {
    captured
        .lines()
        .find(|line| needles.iter().all(|needle| line.contains(needle)))
        .unwrap_or_else(|| panic!("no log line containing {needles:?}:\n{captured}"))
}

#[tokio::test]
async fn enabled_scrub_hashes_identifiers_in_access_log_and_span() {
    let app = support::spawn_configured(false, |cfg| cfg.log_scrub_identifiers = true).await;
    let captured = capture_identifier_requests(&app).await;
    let hashed_path = format!("/v1/subscribers/{HASH_USR_123}");

    let span_line = find_line(&captured, &["http.request{", "/v1/subscribers/"]);
    assert!(
        span_line.contains(&hashed_path),
        "span path is not hashed: {span_line}"
    );

    let access_line = find_line(&captured, &["chimely::access", "/v1/subscribers/"]);
    assert!(
        access_line.contains(&hashed_path),
        "access-log path is not hashed: {access_line}"
    );
    assert!(
        !captured.contains("/v1/subscribers/usr_123"),
        "raw subscriber path leaked:\n{captured}"
    );

    let stream_line = find_line(&captured, &["chimely::access", "/v1/inbox/stream"]);
    let env_hash = hex::encode(&sha2::Sha256::digest(app.env.slug.as_bytes())[..6]);
    assert!(
        stream_line.contains(&format!("subscriber_id={HASH_USR_123}")),
        "subscriber_id is not hashed: {stream_line}"
    );
    assert!(
        stream_line.contains(&format!("environment={env_hash}")),
        "environment is not hashed: {stream_line}"
    );
    assert!(
        stream_line.contains("subscriber_hash=redacted"),
        "{stream_line}"
    );
    assert!(
        !stream_line.contains("usr_123"),
        "raw subscriber id leaked: {stream_line}"
    );
    assert!(
        !stream_line.contains(&app.env.slug),
        "raw environment leaked: {stream_line}"
    );
}

/// The default-off contract. With the flag unset, identifiers log raw in
/// the access log and the span, exactly as before the flag existed. The
/// credential scrub stays on.
#[tokio::test]
async fn default_off_logs_raw_identifiers_unchanged() {
    let app = support::spawn().await;
    let captured = capture_identifier_requests(&app).await;

    let span_line = find_line(&captured, &["http.request{", "/v1/subscribers/"]);
    assert!(
        span_line.contains("/v1/subscribers/usr_123"),
        "span path changed with the flag off: {span_line}"
    );

    let access_line = find_line(&captured, &["chimely::access", "/v1/subscribers/"]);
    assert!(
        access_line.contains("path=/v1/subscribers/usr_123"),
        "access-log path changed with the flag off: {access_line}"
    );

    let stream_line = find_line(&captured, &["chimely::access", "/v1/inbox/stream"]);
    assert!(
        stream_line.contains("subscriber_id=usr_123"),
        "subscriber_id changed with the flag off: {stream_line}"
    );
    assert!(
        stream_line.contains(&format!("environment={}", app.env.slug)),
        "environment changed with the flag off: {stream_line}"
    );
    assert!(
        stream_line.contains("subscriber_hash=redacted"),
        "{stream_line}"
    );
    assert!(
        !captured.contains(HASH_USR_123),
        "identifier hashed despite the flag being off:\n{captured}"
    );
}

//! Task 5: the SSE hint stream — debounced hints over BOTH pub/sub planes
//! (fred Redis and the LISTEN/NOTIFY fallback), Last-Event-ID resume,
//! keep-alive comment frames, per-subscriber connection caps, graceful
//! shutdown `retry:`, and the subscriber_hash log-scrubbing invariant.

mod support;

use std::time::Duration;

use support::SseStream;

const SUB: &str = "usr_sse";

async fn end_to_end_hint(app: &support::TestApp) {
    let mut stream = SseStream::connect(app, SUB, None).await;
    app.spawn_worker();
    app.create_notification(SUB, "x").await;

    let frame = stream
        .next_hint(Duration::from_secs(5))
        .await
        .expect("hint frame");
    assert!(frame.contains("event: hint"), "{frame}");
    assert!(
        frame.contains(r#"data: {"reason":"notification"}"#),
        "{frame}"
    );
    assert!(
        support::event_id(&frame).is_some(),
        "every event carries a resume token: {frame}"
    );
}

#[tokio::test]
async fn hints_flow_end_to_end_over_redis() {
    let app = support::spawn_with_redis().await;
    end_to_end_hint(&app).await;
}

#[tokio::test]
async fn hints_flow_end_to_end_over_listen_notify_fallback() {
    let app = support::spawn().await; // Redis-less mode
    end_to_end_hint(&app).await;
}

#[tokio::test]
async fn hints_are_debounced_per_subscriber() {
    let app = support::spawn_with_redis().await;
    // Subscriber + connection first, then a burst of 6 creates.
    app.create_notification(SUB, "seed").await;
    let mut stream = SseStream::connect(&app, SUB, None).await;
    app.spawn_worker();
    for i in 0..6 {
        app.create_notification(SUB, &format!("burst.{i}")).await;
    }

    // Count hints over ~4 debounce windows (window = 250ms in tests).
    // Exact-duplicate pending hint jobs coalesce on claim: a leading publish
    // plus at most a trailing one per window for jobs enqueued after it —
    // never one publish per create.
    let mut hints = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1000);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        if stream.next_hint(remaining).await.is_some() {
            hints += 1;
        }
    }
    assert!(hints >= 1, "the burst must produce at least one hint");
    assert!(hints <= 3, "7 creates must coalesce, got {hints} hints");
}

#[tokio::test]
async fn broadcast_hints_reach_every_subscriber_with_one_publish() {
    let app = support::spawn_with_redis().await;
    app.create_notification("usr_a", "seed").await;
    app.create_notification("usr_b", "seed").await;
    let mut stream_a = SseStream::connect(&app, "usr_a", None).await;
    let mut stream_b = SseStream::connect(&app, "usr_b", None).await;
    // Flush the seed hints out of the queue before the broadcast.
    app.drain_jobs().await;

    app.create_broadcast("announce").await;
    app.drain_jobs().await;

    // The seed hints may arrive first; wait for the broadcast reason.
    for (name, stream) in [("a", &mut stream_a), ("b", &mut stream_b)] {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        loop {
            let remaining = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .unwrap_or_else(|| panic!("subscriber {name} missed the broadcast hint"));
            let frame = stream
                .next_hint(remaining)
                .await
                .unwrap_or_else(|| panic!("subscriber {name} missed the broadcast hint"));
            if frame.contains(r#""reason":"broadcast""#) {
                break;
            }
        }
    }
}

#[tokio::test]
async fn last_event_id_resume_answers_with_an_immediate_hint_only_if_changed() {
    let app = support::spawn().await;
    app.create_notification(SUB, "x").await;
    app.drain_jobs().await;
    let mut stream = SseStream::connect(&app, SUB, None).await;

    // Provoke one event to harvest a current resume token.
    app.create_notification(SUB, "y").await;
    app.drain_jobs().await;
    let frame = stream
        .next_hint(Duration::from_secs(3))
        .await
        .expect("hint");
    let token = support::event_id(&frame).expect("token");
    drop(stream);

    // Nothing changed since the token ⇒ no immediate hint on resume.
    let mut quiet = SseStream::connect(&app, SUB, Some(&token)).await;
    assert!(
        quiet.next_hint(Duration::from_millis(300)).await.is_none(),
        "spurious resume hint",
    );
    drop(quiet);

    // A change after the token ⇒ exactly one immediate hint, no replay.
    app.create_notification(SUB, "z").await; // job intentionally NOT drained:
    let mut resumed = SseStream::connect(&app, SUB, Some(&token)).await;
    let frame = resumed
        .next_hint(Duration::from_millis(1500))
        .await
        .expect("immediate resume hint");
    assert!(frame.contains(r#""reason":"resume""#), "{frame}");
}

#[tokio::test]
async fn keep_alive_is_a_comment_frame() {
    let app = support::spawn().await;
    let mut stream = SseStream::connect(&app, SUB, None).await;
    // Test config pings every 400ms.
    let frame = stream
        .next_frame(Duration::from_secs(2))
        .await
        .expect("keep-alive frame");
    assert!(
        frame.starts_with(": ping"),
        "expected comment frame, got: {frame}"
    );
}

#[tokio::test]
async fn per_subscriber_connections_are_capped() {
    let app = support::spawn().await; // cap = 2 in the test config
    let _one = SseStream::connect(&app, SUB, None).await;
    let _two = SseStream::connect(&app, SUB, None).await;
    let third = SseStream::try_connect(&app, SUB).await;
    assert_eq!(third.status(), 429, "cap must reject the third stream");
    // Other subscribers are unaffected.
    let _other = SseStream::connect(&app, "usr_other", None).await;

    // Dropping a stream frees a slot.
    drop(_one);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let again = SseStream::try_connect(&app, SUB).await;
    assert_eq!(again.status(), 200, "slot must be released on disconnect");
}

#[tokio::test]
async fn graceful_shutdown_sends_a_retry_directive() {
    let app = support::spawn().await;
    let mut stream = SseStream::connect(&app, SUB, None).await;
    app.shutdown_tx.send(true).unwrap();

    let mut goodbye = None;
    while let Some(frame) = stream.next_frame(Duration::from_secs(2)).await {
        if frame.contains("retry:") {
            goodbye = Some(frame);
            break;
        }
    }
    let frame = goodbye.expect("shutdown must emit a retry frame before closing");

    // The protocol field alone is invisible to EventSource listeners, so
    // the frame must also be a named `retry` event whose data is the same
    // delay in milliseconds. SDK clients run their own reconnect loop and
    // can only honor the override if they can observe it.
    assert!(
        frame.contains("event: retry"),
        "shutdown frame must be a named retry event, got: {frame}"
    );
    let field_ms: u64 = frame
        .lines()
        .find_map(|line| line.strip_prefix("retry: "))
        .expect("protocol retry field present")
        .trim()
        .parse()
        .expect("retry field is milliseconds");
    let data_ms: u64 = frame
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("data line present")
        .trim()
        .parse()
        .expect("retry event data is milliseconds");
    assert_eq!(
        data_ms, field_ms,
        "event data must equal the protocol field"
    );
}

/// Tested invariant (risk: query-string credentials leak into logs):
/// subscriber_hash never appears in access-log lines for the SSE endpoint.
#[tokio::test]
async fn subscriber_hash_is_scrubbed_from_access_logs() {
    use std::sync::{Arc, Mutex};

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
        .with_writer(move || BufWriter(sink.clone()))
        .try_init()
        .expect("install capture subscriber");

    let app = support::spawn().await;
    let hash = dronte::auth::compute_subscriber_hash(&app.env.hmac_secret, SUB);
    let mut stream = SseStream::connect(&app, SUB, None).await;
    // Force the response (and its access-log line) to materialize.
    stream.next_frame(Duration::from_secs(2)).await;
    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let captured = String::from_utf8_lossy(&logs.lock().unwrap()).to_string();
    assert!(
        captured.contains("/v1/inbox/stream"),
        "expected an access-log line for the stream endpoint:\n{captured}"
    );
    assert!(
        !captured.contains(&hash),
        "subscriber_hash leaked into logs:\n{captured}"
    );
    assert!(captured.contains("subscriber_hash=redacted"), "{captured}");
}

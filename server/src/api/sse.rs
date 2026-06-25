//! `GET /v1/inbox/stream` SSE hint stream.
//!
//! SSE is a hint, not a transport. Clients refetch via REST (conditional,
//! ETag) on every hint and every (re)connect, so a missed hint is harmless by
//! construction. The server never replays missed events. `Last-Event-ID` is
//! answered with ONE immediate hint if anything changed after that token.
//!
//! Auth rides query parameters because EventSource cannot set headers, so
//! `subscriber_hash` is scrubbed from access-log lines (see `http::scrub`).

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use chrono::{DateTime, Utc};
use futures::Stream;
use rand::Rng as _;
use serde_json::json;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::auth::SubscriberAuth;
use crate::error::ApiError;
use crate::state::AppState;

#[utoipa::path(
    get,
    path = "/v1/inbox/stream",
    tag = "subscriber",
    operation_id = "streamInbox",
    summary = "SSE hint stream",
    description = r#"`text/event-stream` of **hints, not transports**: the client refetches
via REST (conditional, ETag) on every hint and on every (re)connect, so
missed hints are harmless by construction.

Auth via query parameters (browser `EventSource` cannot set headers).
Server requirement (tested invariant, not a habit): `subscriber_hash`
is scrubbed from access/proxy log lines for this endpoint —
query-string credentials otherwise leak into logs.

Events (`id:` on every event is an opaque resume token):
* `hint` — something changed for this subscriber; refetch list/counts.
  Debounced server-side (at most one per subscriber per interval).

Keep-alive is a comment frame (`: ping`) every 30 seconds, not an
event — comment frames are deliberately invisible to EventSource
listeners. Unknown future event types must be ignored by clients.

Resume: browsers replay `Last-Event-ID` automatically on reconnect; the
server answers by emitting an immediate `hint` if anything changed
after that token (it does not replay individual missed events — the
REST refetch is the recovery mechanism). On graceful shutdown the
server sends a `retry:` directive with jitter before closing, so a
deploy does not produce a reconnect stampede.
"#,
    params(("Last-Event-ID" = Option<String>, Header, description = "Sent automatically by EventSource on reconnect.")),
    responses(
        (status = 200, description = "Event stream.",
            body = inline(crate::api::contract::SseEventStream),
            content_type = "text/event-stream"),
        (status = 401, description = "Missing/invalid API key or subscriber hash.", body = crate::api::contract::Error),
    ),
    security(("SubscriberEnvQ" = [], "SubscriberIdQ" = [], "SubscriberHashQ" = []))
)]
pub async fn stream(
    State(state): State<AppState>,
    auth: SubscriberAuth,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    // Per-subscriber connection cap. Without it a dev environment with
    // optional subscriber hashes is an open connection-exhaustion relay.
    let key = (auth.environment_id, auth.subscriber_id);
    {
        let mut conns = state.sse_connections.lock().expect("sse connection map");
        let count = conns.entry(key).or_insert(0);
        if *count >= state.cfg.sse_max_connections_per_subscriber {
            return Err(ApiError::too_many_connections(
                "too many concurrent streams for this subscriber",
                state.cfg.sse_retry_base.as_secs().max(1),
            ));
        }
        *count += 1;
    }
    metrics::gauge!("dronte_sse_connections").increment(1.0);
    let guard = ConnectionGuard {
        map: Arc::clone(&state.sse_connections),
        key,
    };

    let rx = state.pubsub.subscribe();

    // One immediate resume hint if anything changed after the token. The REST
    // refetch is the recovery mechanism, not event replay.
    let resume_hint = match headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(decode_token)
    {
        Some(since) => changed_since(&state, &auth, since).await?,
        None => false,
    };

    // Graceful shutdown sends `retry:` with per-connection jitter so a deploy
    // does not produce a reconnect stampede.
    let retry_after = state.cfg.sse_retry_base
        + rand::rng().random_range(
            std::time::Duration::ZERO
                ..state
                    .cfg
                    .sse_retry_jitter
                    .max(std::time::Duration::from_millis(1)),
        );
    let mut shutdown = state.shutdown.clone();
    let env = auth.environment_id;
    let sub = auth.subscriber_id;

    let stream = async_stream::stream! {
        let _guard = guard;
        let mut rx = rx;
        if resume_hint {
            yield Ok(hint_event("resume"));
        }
        loop {
            if *shutdown.borrow() {
                yield Ok(retry_event(retry_after));
                break;
            }
            tokio::select! {
                _ = shutdown.changed() => {
                    yield Ok(retry_event(retry_after));
                    break;
                }
                msg = rx.recv() => match msg {
                    Ok(hint)
                        if hint.environment_id == env
                            && hint.subscriber_id.is_none_or(|s| s == sub) =>
                    {
                        yield Ok(hint_event(&hint.reason));
                    }
                    Ok(_) => {}
                    // Fan-in overflow may have dropped a hint. Emit one and
                    // let the client's refetch reconcile.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        yield Ok(hint_event("lagged"));
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        // Comment frame (`: ping`) is deliberately invisible to EventSource
        // listeners, unlike a real event.
        KeepAlive::new()
            .interval(state.cfg.sse_ping_interval)
            .text("ping"),
    ))
}

struct ConnectionGuard {
    map: Arc<Mutex<HashMap<(Uuid, Uuid), usize>>>,
    key: (Uuid, Uuid),
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        metrics::gauge!("dronte_sse_connections").decrement(1.0);
        let mut conns = self.map.lock().expect("sse connection map");
        if let Some(count) = conns.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                conns.remove(&self.key);
            }
        }
    }
}

fn hint_event(reason: &str) -> Event {
    Event::default()
        .id(encode_token(Utc::now()))
        .event("hint")
        .data(json!({ "reason": reason }).to_string())
}

/// Graceful-shutdown frame. The protocol-level `retry:` field serves native
/// EventSource consumers. The named `retry` event carries the same delay in
/// milliseconds as data, because the protocol field is invisible to
/// EventSource listeners and SDK clients run their own reconnect loop. The
/// named event is additive per the contract: "Unknown future event types must
/// be ignored by clients."
fn retry_event(retry_after: std::time::Duration) -> Event {
    Event::default()
        .event("retry")
        .data(retry_after.as_millis().to_string())
        .retry(retry_after)
}

/// Opaque resume token, hex-encoded app-clock microseconds. Cross-host clock
/// skew can suppress or duplicate the resume hint. This is harmless because
/// clients refetch via REST on every reconnect anyway.
fn encode_token(t: DateTime<Utc>) -> String {
    format!("{:x}", t.timestamp_micros())
}

fn decode_token(token: &str) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_micros(i64::from_str_radix(token, 16).ok()?)
}

/// Reports whether anything changed after the token, from the same inputs
/// as the list ETag.
async fn changed_since(
    state: &AppState,
    auth: &SubscriberAuth,
    since: DateTime<Utc>,
) -> Result<bool, ApiError> {
    let changed = sqlx::query_scalar!(
        r#"SELECT (c.updated_at > $4)
                OR EXISTS (SELECT 1 FROM notifications n
                     WHERE n.environment_id = $1 AND n.subscriber_id = $2
                       AND n.visible_at <= now() AND n.visible_at > $4)
                OR EXISTS (SELECT 1 FROM broadcasts b
                     WHERE b.environment_id = $1
                       AND b.created_at >= $3 AND b.created_at > $4)
               AS "changed!"
           FROM subscriber_counters c
           WHERE c.environment_id = $1 AND c.subscriber_id = $2"#,
        auth.environment_id,
        auth.subscriber_id,
        auth.subscriber_created_at,
        since,
    )
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_tokens_round_trip() {
        let now = Utc::now();
        let decoded = decode_token(&encode_token(now)).unwrap();
        assert_eq!(decoded.timestamp_micros(), now.timestamp_micros());
        assert!(decode_token("not hex!").is_none());
    }
}

//! The single error envelope from specs/openapi.yaml:
//! `{"error": {"code", "message"}}` with conventional status codes.

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    /// The contract says "429 carries `Retry-After`". Set on every 429.
    pub retry_after_seconds: Option<u64>,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    /// Authenticated but lacking the capability the endpoint requires.
    /// Distinct from `unauthorized` (no/invalid session) so the SPA can tell
    /// "log in" apart from "your role cannot do this".
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    /// A guard-rail refusal (self-mutation, last-admin lockout, duplicate
    /// identity) where the request was well-formed but the state forbids it.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    pub fn too_many_connections(message: impl Into<String>, retry_after_seconds: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "too_many_connections",
            message: message.into(),
            retry_after_seconds: Some(retry_after_seconds),
        }
    }

    pub fn rate_limited(retry_after: std::time::Duration) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
            message: "rate limit exceeded".to_owned(),
            // Ceil to a whole second; Retry-After: 0 invites an instant retry.
            retry_after_seconds: Some((retry_after.as_secs_f64().ceil() as u64).max(1)),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "error": { "code": self.code, "message": self.message } });
        let mut response = (self.status, Json(body)).into_response();
        if let Some(seconds) = self.retry_after_seconds
            && let Ok(value) = header::HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

/// Internal failures (database, serialization) become opaque 500s. The detail
/// goes to tracing, never to the client.
impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        let err: anyhow::Error = err.into();
        tracing::error!(error = ?err, "internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: "internal server error".to_owned(),
            retry_after_seconds: None,
        }
    }
}

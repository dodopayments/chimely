//! Extractors that reject with the contract's error envelope.
//!
//! axum's built-in `Json`/`Query` rejections answer malformed input with
//! plain-text 400/415/422 bodies. The contract allows exactly one error
//! shape (`{"error": {"code", "message"}}`) and no 415/422, so every handler
//! extracts through these wrappers instead.
//!
//! Rejection messages are static. axum/serde rejection prose quotes
//! caller-supplied scalars on a type mismatch, so it never reaches the
//! response body.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, FromRequestParts, OptionalFromRequest, Request};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// `body_text()` embeds the serde error, which quotes a caller-supplied
/// scalar on a type mismatch. Each rejection class maps to a fixed message.
fn json_rejection_message(rejection: &JsonRejection) -> &'static str {
    match rejection {
        JsonRejection::JsonSyntaxError(_) => "request body is not valid JSON",
        JsonRejection::JsonDataError(_) => "request body does not match the expected schema",
        JsonRejection::MissingJsonContentType(_) => "expected content-type: application/json",
        _ => "invalid request body",
    }
}

pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        match <axum::Json<T> as FromRequest<S>>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::bad_request(json_rejection_message(&rejection))),
        }
    }
}

impl<S, T> OptionalFromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Option<Self>, ApiError> {
        match <axum::Json<T> as OptionalFromRequest<S>>::from_request(req, state).await {
            Ok(Some(axum::Json(value))) => Ok(Some(Self(value))),
            Ok(None) => Ok(None),
            Err(rejection) => Err(ApiError::bad_request(json_rejection_message(&rejection))),
        }
    }
}

pub struct ApiQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(_) => Err(ApiError::bad_request(
                "query string does not match the expected parameters",
            )),
        }
    }
}

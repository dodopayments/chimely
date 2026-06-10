//! The v1 HTTP surface (specs/openapi.yaml is the convergence target).

pub mod contract;
pub mod inbox;
pub mod management;
pub mod preferences;
pub mod sse;

use chrono::{DateTime, SecondsFormat, Utc};

/// RFC 3339 UTC with microsecond precision. Matches Postgres timestamptz
/// precision so a snapshot replay can never differ from a live render.
pub(crate) fn format_ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

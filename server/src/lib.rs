//! Dronte server library. The `dronte` binary (`src/main.rs`) is a thin
//! wrapper. Everything lives here so integration tests (testcontainers) can
//! drive the router, the worker loop, and maintenance jobs in-process.

pub mod api;
pub mod auth;
pub mod bootstrap;
pub mod config;
pub mod db;
pub mod dlq;
pub mod error;
pub mod extract;
pub mod http;
pub mod ids;
pub mod jobs;
pub mod metrics_sampler;
pub mod openapi;
pub mod partitions;
pub mod pubsub;
pub mod ratelimit;
pub mod roles;
pub mod state;
pub mod telemetry;
pub mod timeline;
pub mod worker;

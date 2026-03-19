// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// native-sovd — SOVD REST API via axum
// axum router, tower middleware, JSON API (ISO 17978-3 / SOVD)
// ─────────────────────────────────────────────────────────────────────────────
#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(warnings)]
#![allow(
    clippy::wildcard_imports,
    clippy::enum_glob_use,
    clippy::items_after_statements,
    clippy::doc_link_with_quotes,
    clippy::result_large_err
)]

pub mod auth;
pub mod dlt;
pub mod mdns;
pub mod oem_sample;
pub mod openapi;
pub mod routes;
pub mod state;

// OEM profiles are auto-detected by build.rs: any `src/oem_*.rs` file
// (except oem_sample.rs) triggers a `has_oem_<name>` cfg flag.
// This means proprietary profiles compile automatically when their source
// file is present — no Cargo feature flags needed.
#[cfg(has_oem_mbds)]
pub mod oem_mbds;

pub use auth::AuthConfig;
pub use dlt::{DltConfig, DltLayer};
pub use mdns::{MdnsConfig, MdnsHandle};
pub use oem_sample::SampleOemProfile;
#[cfg(has_oem_mbds)]
pub use oem_mbds::{MbdsProfile, MbdsProfileConfig};
pub use routes::build_router;
pub use state::{AppState, DiagState, RuntimeState, SecurityState};

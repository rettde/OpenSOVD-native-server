// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// native-sovd — SOVD REST API via axum
// axum router, tower middleware, JSON API (ISO 17978-3 / SOVD)
// ─────────────────────────────────────────────────────────────────────────────
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
pub mod openapi;
pub mod routes;
pub mod state;

pub use auth::AuthConfig;
pub use routes::build_router;
pub use state::AppState;

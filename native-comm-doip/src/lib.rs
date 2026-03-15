// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// native-comm-doip — DoIP communication using doip-codec + doip-definitions
// ─────────────────────────────────────────────────────────────────────────────
#![forbid(unsafe_code)]
#![allow(
    clippy::redundant_closure_for_method_calls,
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::manual_let_else,
    clippy::single_match_else,
    clippy::unnested_or_patterns,
    clippy::needless_continue
)]

pub mod config;
pub mod connection;
pub mod discovery;

pub use config::DoipConfig;
pub use connection::DoipConnection;
pub use discovery::discover_vehicles;

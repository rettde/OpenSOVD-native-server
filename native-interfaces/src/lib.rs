// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// native-interfaces — Shared types, traits and error definitions
// ─────────────────────────────────────────────────────────────────────────────
#![forbid(unsafe_code)]
#![deny(warnings)]

pub mod backend;
pub mod diag;
pub mod error;
pub mod oem;
pub mod sovd;

// Re-export key types at crate root
pub use backend::ComponentBackend;
pub use oem::{DefaultProfile, OemProfile};
pub use diag::{
    service_ids, DiagTransport, DiagnosticSession, EcuConnectionState, ServicePayload,
    TesterPresentMode, TesterPresentType, UdsResponse,
};
pub use error::{ConnectionError, DiagServiceError, DoipGatewaySetupError, SomeIpError};

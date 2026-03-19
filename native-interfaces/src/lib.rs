// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// native-interfaces — Shared types, traits and error definitions
// ─────────────────────────────────────────────────────────────────────────────
#![forbid(unsafe_code)]
#![deny(warnings)]

pub mod backend;
pub mod bridge;
pub mod diag;
pub mod error;
pub mod oem;
pub mod secrets;
pub mod sovd;
pub mod storage;
pub mod tenant;

// Re-export key types at crate root
pub use backend::{ComponentBackend, EntityBackend, ExtendedDiagBackend};
pub use bridge::{BridgeError, BridgeTransport};
pub use diag::{
    service_ids, DiagTransport, DiagnosticSession, EcuConnectionState, ServicePayload,
    TesterPresentMode, TesterPresentType, UdsResponse,
};
pub use error::{ConnectionError, DiagServiceError, DoipGatewaySetupError, SomeIpError};
pub use oem::{DefaultProfile, OemProfile};
pub use secrets::{EnvSecretProvider, SecretProvider, StaticSecretProvider};
pub use storage::{InMemoryStorage, StorageBackend};
pub use tenant::{MultiTenantConfig, TenantContext, TenantIsolation};

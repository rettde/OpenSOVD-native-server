// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// native-interfaces — Shared types, traits and error definitions
// ─────────────────────────────────────────────────────────────────────────────
#![forbid(unsafe_code)]
#![deny(warnings)]

pub mod audit_sink;
pub mod backend;
pub mod bridge;
pub mod data_catalog;
pub mod diag;
pub mod error;
pub mod feature_flags;
pub mod oem;
pub mod rbac;
pub mod secrets;
pub mod sovd;
pub mod storage;
pub mod tenant;

// Re-export key types at crate root
pub use backend::{ComponentBackend, EntityBackend, ExtendedDiagBackend};
pub use bridge::{BridgeError, BridgeTransport};
pub use data_catalog::{
    DataCatalogProvider, DataSemantics, NormalRange, StaticDataCatalogProvider,
};
pub use diag::{
    service_ids, DiagTransport, DiagnosticSession, EcuConnectionState, ServicePayload,
    TesterPresentMode, TesterPresentType, UdsResponse,
};
pub use error::{ConnectionError, DiagServiceError, DoipGatewaySetupError, SomeIpError};
pub use feature_flags::{FeatureFlagConfig, FeatureFlags, SharedFeatureFlags};
pub use audit_sink::{AuditForwardingConfig, AuditSink, CallbackAuditSink};
pub use oem::{DefaultProfile, OemProfile};
pub use rbac::{RbacConfig, RbacPolicy, RbacRole};
pub use secrets::{EnvSecretProvider, SecretProvider, StaticSecretProvider};
pub use storage::{InMemoryStorage, StorageBackend};
pub use tenant::{MultiTenantConfig, TenantContext, TenantIsolation};

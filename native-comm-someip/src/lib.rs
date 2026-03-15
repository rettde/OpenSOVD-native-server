// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// native-comm-someip — vSomeIP FFI bindings for SOME/IP communication
// NOTE: This is the ONLY crate where unsafe is permitted (vSomeIP C++ FFI).
// All other crates use #![forbid(unsafe_code)].
#![allow(unsafe_code)]
#![allow(clippy::cast_precision_loss)]
//
// Architecture:
//   - When feature "vsomeip-ffi" is enabled, uses actual libvsomeip3 via C FFI
//   - Without the feature, provides a stub implementation for compilation
//     on systems without vSomeIP installed
//
// vSomeIP is the reference SOME/IP implementation used in Adaptive AUTOSAR.
// This crate wraps it for Rust, following the Eclipse OpenSOVD ecosystem
// approach of using established C++ middleware via FFI rather than
// reimplementing protocols in pure Rust.
// ─────────────────────────────────────────────────────────────────────────────

pub mod config;
pub mod service;

#[cfg(feature = "vsomeip-ffi")]
pub mod ffi;
#[cfg(feature = "vsomeip-ffi")]
pub mod runtime;

pub use config::SomeIpConfig;
pub use service::{SomeIpRuntime, SomeIpServiceProxy};

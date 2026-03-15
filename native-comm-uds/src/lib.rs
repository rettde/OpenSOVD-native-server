// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// native-comm-uds — UDS communication layer
// UdsManager with TesterPresent, session mgmt, security access,
// data transfer, routine control
// ─────────────────────────────────────────────────────────────────────────────
#![forbid(unsafe_code)]
#![allow(
    clippy::redundant_closure_for_method_calls,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::manual_let_else,
    clippy::single_match_else,
    clippy::unnested_or_patterns,
    clippy::match_same_arms,
    clippy::cast_lossless
)]

pub mod manager;
pub mod tester_present;

pub use manager::{CommControlType, DtcInfo, DtcSettingType, IoControlParameter, UdsManager};
pub use tester_present::TesterPresentTask;

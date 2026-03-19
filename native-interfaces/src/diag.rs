// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Diagnostic communication types — UDS Service IDs, sessions, response types
// ISO 14229-1 (UDS) aligned
// ─────────────────────────────────────────────────────────────────────────────

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ConnectionError;

/// Diagnostic transport abstraction — decouples UDS from the underlying
/// transport (DoIP, CAN, mock, etc.).
///
/// Implementations:
///   - `SovdHttpBackend` (native-core) — proxies to external CDA/SOVD backends
///   - Test mocks for unit testing without network
#[async_trait]
pub trait DiagTransport: Send + Sync {
    /// Send a raw diagnostic request and receive the UDS response.
    async fn send_diagnostic(&self, data: &[u8]) -> Result<UdsResponse, ConnectionError>;
}

/// UDS Service IDs (ISO 14229-1)
pub mod service_ids {
    pub const SESSION_CONTROL: u8 = 0x10;
    pub const ECU_RESET: u8 = 0x11;
    pub const CLEAR_DIAGNOSTIC_INFORMATION: u8 = 0x14;
    pub const READ_DTC_INFORMATION: u8 = 0x19;
    pub const READ_DATA_BY_IDENTIFIER: u8 = 0x22;
    pub const SECURITY_ACCESS: u8 = 0x27;
    pub const COMMUNICATION_CONTROL: u8 = 0x28;
    pub const AUTHENTICATION: u8 = 0x29;
    pub const WRITE_DATA_BY_IDENTIFIER: u8 = 0x2E;
    pub const INPUT_OUTPUT_CONTROL_BY_IDENTIFIER: u8 = 0x2F;
    pub const ROUTINE_CONTROL: u8 = 0x31;
    pub const REQUEST_DOWNLOAD: u8 = 0x34;
    pub const TRANSFER_DATA: u8 = 0x36;
    pub const REQUEST_TRANSFER_EXIT: u8 = 0x37;
    pub const TESTER_PRESENT: u8 = 0x3E;
    pub const CONTROL_DTC_SETTING: u8 = 0x85;
}

/// UDS response payload
#[derive(Debug, Clone)]
pub struct ServicePayload {
    pub data: Vec<u8>,
    pub source_address: u16,
    pub target_address: u16,
}

/// UDS response type
#[derive(Debug, Clone)]
pub enum UdsResponse {
    Message(ServicePayload),
    ResponsePending(u16),
    BusyRepeatRequest(u16),
    TemporarilyNotAvailable(u16),
    TesterPresentNRC(u8),
}

/// `TesterPresent` mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TesterPresentMode {
    Start,
    Stop,
}

/// `TesterPresent` type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TesterPresentType {
    Functional(String),
    Ecu(String),
}

/// Diagnostic session types (ISO 14229-1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum DiagnosticSession {
    Default = 0x01,
    Programming = 0x02,
    Extended = 0x03,
}

/// ECU connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EcuConnectionState {
    Disconnected,
    Connected,
    RoutingActivated,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn service_ids_constants() {
        assert_eq!(service_ids::SESSION_CONTROL, 0x10);
        assert_eq!(service_ids::TESTER_PRESENT, 0x3E);
        assert_eq!(service_ids::READ_DATA_BY_IDENTIFIER, 0x22);
        assert_eq!(service_ids::WRITE_DATA_BY_IDENTIFIER, 0x2E);
        assert_eq!(service_ids::READ_DTC_INFORMATION, 0x19);
        assert_eq!(service_ids::CLEAR_DIAGNOSTIC_INFORMATION, 0x14);
        assert_eq!(service_ids::ROUTINE_CONTROL, 0x31);
        assert_eq!(service_ids::REQUEST_DOWNLOAD, 0x34);
        assert_eq!(service_ids::TRANSFER_DATA, 0x36);
        assert_eq!(service_ids::REQUEST_TRANSFER_EXIT, 0x37);
    }

    #[test]
    fn diagnostic_session_repr() {
        assert_eq!(DiagnosticSession::Default as u8, 0x01);
        assert_eq!(DiagnosticSession::Programming as u8, 0x02);
        assert_eq!(DiagnosticSession::Extended as u8, 0x03);
    }

    #[test]
    fn ecu_connection_state_serializes() {
        let json = serde_json::to_value(EcuConnectionState::RoutingActivated).unwrap();
        assert_eq!(json, "RoutingActivated");
        let deser: EcuConnectionState = serde_json::from_value(json).unwrap();
        assert_eq!(deser, EcuConnectionState::RoutingActivated);
    }
}

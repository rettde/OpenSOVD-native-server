// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Error types — diagnostic service, DoIP, and SOME/IP errors
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashSet;

/// Central diagnostic service error
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiagServiceError {
    #[error("Not found: {0:?}")]
    NotFound(Option<String>),
    #[error("Request not supported: {0}")]
    RequestNotSupported(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Bad payload: {0}")]
    BadPayload(String),
    #[error("Payload too short, expected at least {expected} bytes, got {actual} bytes")]
    NotEnoughData { expected: usize, actual: usize },
    #[error("{0}")]
    InvalidState(String),
    #[error("{0}")]
    InvalidAddress(String),
    #[error("Sending message failed {0}")]
    SendFailed(String),
    #[error("Received Nack, code={0:?}")]
    Nack(u8),
    #[error("Unexpected response. {0:?}")]
    UnexpectedResponse(Option<String>),
    #[error("No response {0}")]
    NoResponse(String),
    #[error("Connection closed {0}")]
    ConnectionClosed(String),
    #[error("Ecu {0} offline")]
    EcuOffline(String),
    #[error("Timeout")]
    Timeout,
    #[error("Access denied: {0}")]
    AccessDenied(String),
    #[error("Resource error: {0}")]
    ResourceError(String),
    #[error("Invalid parameter. Possible values are: {possible_values:?}")]
    InvalidParameter { possible_values: HashSet<String> },
}

/// DoIP gateway setup error
#[derive(Debug, thiserror::Error)]
pub enum DoipGatewaySetupError {
    #[error("Invalid address: `{0}`")]
    InvalidAddress(String),
    #[error("Socket error: `{0}`")]
    SocketCreationFailed(String),
    #[error("Port error: `{0}`")]
    PortBindFailed(String),
    #[error("Configuration error: `{0}`")]
    InvalidConfiguration(String),
    #[error("Resource error: `{0}`")]
    ResourceError(String),
    #[error("Server error: `{0}`")]
    ServerError(String),
}

/// DoIP connection error
#[derive(Debug, Clone, thiserror::Error)]
pub enum ConnectionError {
    #[error("Connection closed.")]
    Closed,
    #[error("Decoding error: `{0}`")]
    Decoding(String),
    #[error("Invalid message: `{0}`")]
    InvalidMessage(String),
    #[error("Connection timeout: `{0}`")]
    Timeout(String),
    #[error("Connection failed: `{0}`")]
    ConnectionFailed(String),
    #[error("Routing error: `{0}`")]
    RoutingError(String),
    #[error("Send failed: `{0}`")]
    SendFailed(String),
}

/// vSomeIP specific errors
#[derive(Debug, Clone, thiserror::Error)]
pub enum SomeIpError {
    #[error("vSomeIP not available: {0}")]
    NotAvailable(String),
    #[error("Service discovery failed: {0}")]
    DiscoveryFailed(String),
    #[error("Request failed: service=0x{service_id:04X}, method=0x{method_id:04X}: {details}")]
    RequestFailed {
        service_id: u16,
        method_id: u16,
        details: String,
    },
    #[error("Subscription error: {0}")]
    SubscriptionError(String),
}

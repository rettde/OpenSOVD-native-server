// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// DoIP configuration
// ─────────────────────────────────────────────────────────────────────────────

use serde::{Deserialize, Serialize};

/// DoIP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoipConfig {
    /// Tester (client) IP address
    pub tester_address: String,
    /// Subnet mask for broadcast discovery
    pub tester_subnet: String,
    /// DoIP gateway port (default: 13400)
    pub gateway_port: u16,
    /// TLS port (0 = disabled, >0 = use TLS on this port)
    pub tls_port: u16,
    /// Send timeout in milliseconds
    pub send_timeout_ms: u64,
    /// Source address of this tester
    pub source_address: u16,
    /// Path to CA certificate file for verifying the DoIP gateway (PEM)
    #[serde(default)]
    pub tls_ca_cert: Option<String>,
    /// Path to client certificate file for mutual TLS (PEM)
    #[serde(default)]
    pub tls_client_cert: Option<String>,
    /// Path to client private key file for mutual TLS (PEM)
    #[serde(default)]
    pub tls_client_key: Option<String>,
    /// Skip TLS certificate verification (ONLY for development!)
    #[serde(default)]
    pub tls_insecure: bool,
}

impl Default for DoipConfig {
    fn default() -> Self {
        Self {
            tester_address: "127.0.0.1".to_owned(),
            tester_subnet: "255.255.0.0".to_owned(),
            gateway_port: 13400,
            tls_port: 0,
            send_timeout_ms: 5000,
            source_address: 0x0E00,
            tls_ca_cert: None,
            tls_client_cert: None,
            tls_client_key: None,
            tls_insecure: false,
        }
    }
}

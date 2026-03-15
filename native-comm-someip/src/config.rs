// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// vSomeIP configuration
// ─────────────────────────────────────────────────────────────────────────────

use serde::{Deserialize, Serialize};

/// SOME/IP service endpoint configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SomeIpConfig {
    /// Application name registered with vSomeIP
    pub application_name: String,
    /// Path to vsomeip JSON configuration file
    pub vsomeip_config_path: Option<String>,
    /// Services to offer (as a native SOVD provider)
    pub offered_services: Vec<ServiceDefinition>,
    /// Services to consume (from other Adaptive AUTOSAR apps)
    pub consumed_services: Vec<ServiceDefinition>,
}

impl Default for SomeIpConfig {
    fn default() -> Self {
        Self {
            application_name: "opensovd-native-server".to_owned(),
            vsomeip_config_path: None,
            offered_services: vec![],
            consumed_services: vec![],
        }
    }
}

/// SOME/IP service definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDefinition {
    pub service_id: u16,
    pub instance_id: u16,
    pub major_version: u8,
    pub minor_version: u32,
    /// Methods exposed/consumed by this service
    pub methods: Vec<MethodDefinition>,
    /// Events/eventgroups for pub/sub
    pub eventgroups: Vec<EventGroupDefinition>,
}

/// SOME/IP method definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodDefinition {
    pub method_id: u16,
    pub name: String,
    pub is_fire_and_forget: bool,
}

/// SOME/IP eventgroup definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventGroupDefinition {
    pub eventgroup_id: u16,
    pub name: String,
    pub events: Vec<u16>,
}

// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// mDNS/DNS-SD Discovery (MBDS S-SOVD §4.2, SOVD §4.2)
//
// Registers the SOVD server as a DNS-SD service on the local network so that
// diagnostic clients can discover it via `_sovd._tcp.local.`.
// ─────────────────────────────────────────────────────────────────────────────

use mdns_sd::{ServiceDaemon, ServiceInfo};
use tracing::{info, warn};

/// SOVD DNS-SD service type (SOVD §4.2)
const SERVICE_TYPE: &str = "_sovd._tcp.local.";

/// mDNS/DNS-SD configuration
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MdnsConfig {
    /// Enable mDNS/DNS-SD discovery (default: false)
    #[serde(default)]
    pub enabled: bool,
    /// Instance name advertised via mDNS (default: "OpenSOVD-native-server")
    #[serde(default = "default_instance_name")]
    pub instance_name: String,
    /// Hostname for the mDNS record (should match TLS Common Name)
    #[serde(default = "default_hostname")]
    pub hostname: String,
}

fn default_instance_name() -> String {
    "OpenSOVD-native-server".to_owned()
}

fn default_hostname() -> String {
    "opensovd.local.".to_owned()
}

impl Default for MdnsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            instance_name: default_instance_name(),
            hostname: default_hostname(),
        }
    }
}

/// Handle to the running mDNS daemon — drop to unregister
pub struct MdnsHandle {
    daemon: ServiceDaemon,
    fullname: String,
}

impl MdnsHandle {
    /// Register the SOVD server on the local network via mDNS/DNS-SD.
    ///
    /// Returns `None` if mDNS is disabled or registration fails (non-fatal).
    pub fn register(config: &MdnsConfig, port: u16) -> Option<Self> {
        if !config.enabled {
            return None;
        }

        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                warn!("mDNS daemon init failed (non-fatal): {e}");
                return None;
            }
        };

        let properties = [
            ("sovdVersion", "1.1.0"),
            ("path", "/sovd/v1"),
            ("tls", "optional"),
        ];

        let service_info = match ServiceInfo::new(
            SERVICE_TYPE,
            &config.instance_name,
            &config.hostname,
            "",
            port,
            &properties[..],
        ) {
            Ok(info) => info,
            Err(e) => {
                warn!("mDNS service info creation failed: {e}");
                return None;
            }
        };

        let fullname = service_info.get_fullname().to_owned();

        if let Err(e) = daemon.register(service_info) {
            warn!("mDNS registration failed (non-fatal): {e}");
            return None;
        }

        info!(
            instance = %config.instance_name,
            hostname = %config.hostname,
            port = port,
            service_type = SERVICE_TYPE,
            "mDNS/DNS-SD: SOVD service registered"
        );

        Some(Self { daemon, fullname })
    }

    /// Unregister the service (called on shutdown)
    pub fn unregister(&self) {
        if let Err(e) = self.daemon.unregister(&self.fullname) {
            warn!("mDNS unregister failed: {e}");
        } else {
            info!("mDNS/DNS-SD: SOVD service unregistered");
        }
    }
}

impl Drop for MdnsHandle {
    fn drop(&mut self) {
        self.unregister();
    }
}

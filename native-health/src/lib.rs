// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// native-health — System health monitoring
// ─────────────────────────────────────────────────────────────────────────────
#![forbid(unsafe_code)]
#![allow(
    clippy::cast_precision_loss,
    clippy::redundant_closure_for_method_calls
)]

use sysinfo::System;

/// Health monitor providing CPU, memory, and system metrics
pub struct HealthMonitor {
    system: std::sync::Mutex<System>,
}

impl HealthMonitor {
    pub fn new() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        Self {
            system: std::sync::Mutex::new(sys),
        }
    }

    /// Collect current system health info as JSON
    pub fn system_info(&self) -> serde_json::Value {
        let Ok(mut sys) = self.system.lock() else {
            return serde_json::json!({ "status": "error", "message": "health mutex poisoned" });
        };
        sys.refresh_all();

        let cpu_usage: f32 = if sys.cpus().is_empty() {
            0.0
        } else {
            sys.cpus().iter().map(|c| c.cpu_usage()).sum::<f32>() / sys.cpus().len() as f32
        };

        serde_json::json!({
            "status": "ok",
            "system": {
                "cpu_count": sys.cpus().len(),
                "cpu_usage_percent": (cpu_usage * 100.0).round() / 100.0,
                "total_memory_bytes": sys.total_memory(),
                "used_memory_bytes": sys.used_memory(),
                "available_memory_bytes": sys.available_memory(),
                "memory_usage_percent": if sys.total_memory() > 0 {
                    ((sys.used_memory() as f64 / sys.total_memory() as f64) * 10000.0).round() / 100.0
                } else {
                    0.0
                },
                "system_name": System::name().unwrap_or_default(),
                "os_version": System::os_version().unwrap_or_default(),
                "host_name": System::host_name().unwrap_or_default(),
                "uptime_secs": System::uptime(),
            }
        })
    }
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn health_monitor_creates_successfully() {
        let _monitor = HealthMonitor::new();
    }

    #[test]
    fn system_info_returns_valid_json() {
        let monitor = HealthMonitor::new();
        let info = monitor.system_info();
        assert!(info.is_object());
        assert_eq!(info["status"], "ok");
    }

    #[test]
    fn system_info_has_system_section() {
        let monitor = HealthMonitor::new();
        let info = monitor.system_info();
        let sys = &info["system"];
        assert!(sys.is_object());
        assert!(sys.get("cpu_count").is_some());
        assert!(sys.get("cpu_usage_percent").is_some());
        assert!(sys.get("total_memory_bytes").is_some());
        assert!(sys.get("used_memory_bytes").is_some());
        assert!(sys.get("available_memory_bytes").is_some());
        assert!(sys.get("memory_usage_percent").is_some());
        assert!(sys.get("system_name").is_some());
        assert!(sys.get("os_version").is_some());
        assert!(sys.get("host_name").is_some());
        assert!(sys.get("uptime_secs").is_some());
    }

    #[test]
    fn system_info_memory_values_are_non_negative() {
        let monitor = HealthMonitor::new();
        let info = monitor.system_info();
        let sys = &info["system"];
        assert!(sys["total_memory_bytes"].as_u64().unwrap() > 0);
        assert!(sys["memory_usage_percent"].as_f64().unwrap() >= 0.0);
        assert!(sys["memory_usage_percent"].as_f64().unwrap() <= 100.0);
    }

    #[test]
    fn system_info_uptime_positive() {
        let monitor = HealthMonitor::new();
        let info = monitor.system_info();
        assert!(info["system"]["uptime_secs"].as_u64().unwrap() > 0);
    }

    #[test]
    fn default_creates_monitor() {
        let monitor = HealthMonitor::default();
        let info = monitor.system_info();
        assert_eq!(info["status"], "ok");
    }
}

// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// ComponentRouter — SOVD Gateway pattern
//
// Aggregates multiple ComponentBackend instances and dispatches requests
// to the backend that manages the target component. This implements the
// "SOVD Gateway" role from the OpenSOVD architecture:
//
//   Client → SOVD Server → ComponentRouter (Gateway)
//                               ├── SovdHttpBackend    (external CDA)
//                               └── SovdHttpBackend    (native SOVD ECU / demo-ecu)
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use async_trait::async_trait;
use tracing::debug;

use native_interfaces::{
    sovd::{
        SovdBulkDataItem, SovdBulkWriteItem, SovdCapabilities, SovdComponent, SovdComponentConfig,
        SovdDataCatalogEntry, SovdFault, SovdGroup, SovdMode, SovdOperation,
    },
    ComponentBackend, DiagServiceError,
};

/// Gateway router that dispatches SOVD requests to the correct backend.
///
/// Backends are tried in registration order. The first backend that
/// `handles_component(id)` returns true for wins the dispatch.
pub struct ComponentRouter {
    backends: Vec<Arc<dyn ComponentBackend>>,
}

impl ComponentRouter {
    #[must_use]
    pub fn new(backends: Vec<Arc<dyn ComponentBackend>>) -> Self {
        debug!(
            count = backends.len(),
            names = ?backends.iter().map(|b| b.name()).collect::<Vec<_>>(),
            "ComponentRouter initialized"
        );
        Self { backends }
    }

    /// Find the backend responsible for a component
    fn backend_for(
        &self,
        component_id: &str,
    ) -> Result<&Arc<dyn ComponentBackend>, DiagServiceError> {
        self.backends
            .iter()
            .find(|b| b.handles_component(component_id))
            .ok_or_else(|| {
                DiagServiceError::NotFound(Some(format!(
                    "No backend manages component '{component_id}'"
                )))
            })
    }

    /// Get all registered backends
    #[must_use]
    pub fn backends(&self) -> &[Arc<dyn ComponentBackend>] {
        &self.backends
    }
}

#[async_trait]
impl ComponentBackend for ComponentRouter {
    fn name(&self) -> &'static str {
        "ComponentRouter (Gateway)"
    }

    // ── Discovery — aggregate from all backends ─────────────────────────────

    fn list_components(&self) -> Vec<SovdComponent> {
        self.backends
            .iter()
            .flat_map(|b| b.list_components())
            .collect()
    }

    fn get_component(&self, component_id: &str) -> Option<SovdComponent> {
        self.backends
            .iter()
            .find_map(|b| b.get_component(component_id))
    }

    fn handles_component(&self, component_id: &str) -> bool {
        self.backends
            .iter()
            .any(|b| b.handles_component(component_id))
    }

    // ── Connection lifecycle — dispatch to owning backend ───────────────────

    async fn connect(&self, component_id: &str) -> Result<(), DiagServiceError> {
        self.backend_for(component_id)?.connect(component_id).await
    }

    async fn disconnect(&self, component_id: &str) -> Result<(), DiagServiceError> {
        self.backend_for(component_id)?
            .disconnect(component_id)
            .await
    }

    // ── Data ────────────────────────────────────────────────────────────────

    fn list_data(&self, component_id: &str) -> Result<Vec<SovdDataCatalogEntry>, DiagServiceError> {
        self.backend_for(component_id)?.list_data(component_id)
    }

    async fn read_data(
        &self,
        component_id: &str,
        data_id: &str,
    ) -> Result<serde_json::Value, DiagServiceError> {
        self.backend_for(component_id)?
            .read_data(component_id, data_id)
            .await
    }

    async fn write_data(
        &self,
        component_id: &str,
        data_id: &str,
        value: &[u8],
    ) -> Result<(), DiagServiceError> {
        self.backend_for(component_id)?
            .write_data(component_id, data_id, value)
            .await
    }

    // ── Faults ──────────────────────────────────────────────────────────────

    async fn read_faults(&self, component_id: &str) -> Result<Vec<SovdFault>, DiagServiceError> {
        self.backend_for(component_id)?
            .read_faults(component_id)
            .await
    }

    async fn clear_faults(&self, component_id: &str) -> Result<(), DiagServiceError> {
        self.backend_for(component_id)?
            .clear_faults(component_id)
            .await
    }

    // ── Operations ──────────────────────────────────────────────────────────

    fn list_operations(&self, component_id: &str) -> Result<Vec<SovdOperation>, DiagServiceError> {
        self.backend_for(component_id)?
            .list_operations(component_id)
    }

    async fn execute_operation(
        &self,
        component_id: &str,
        operation_id: &str,
        params: Option<&[u8]>,
    ) -> Result<serde_json::Value, DiagServiceError> {
        self.backend_for(component_id)?
            .execute_operation(component_id, operation_id, params)
            .await
    }

    // ── Capabilities ────────────────────────────────────────────────────────

    fn get_capabilities(&self, component_id: &str) -> Result<SovdCapabilities, DiagServiceError> {
        self.backend_for(component_id)?
            .get_capabilities(component_id)
    }

    // ── Mode ────────────────────────────────────────────────────────────────

    fn get_mode(&self, component_id: &str) -> Result<SovdMode, DiagServiceError> {
        self.backend_for(component_id)?.get_mode(component_id)
    }

    async fn set_mode(&self, component_id: &str, mode: &str) -> Result<(), DiagServiceError> {
        self.backend_for(component_id)?
            .set_mode(component_id, mode)
            .await
    }

    // ── Configuration ───────────────────────────────────────────────────────

    async fn read_config(
        &self,
        component_id: &str,
    ) -> Result<SovdComponentConfig, DiagServiceError> {
        self.backend_for(component_id)?
            .read_config(component_id)
            .await
    }

    async fn write_config(
        &self,
        component_id: &str,
        param_name: &str,
        value: &[u8],
    ) -> Result<(), DiagServiceError> {
        self.backend_for(component_id)?
            .write_config(component_id, param_name, value)
            .await
    }

    // ── Bulk Data ───────────────────────────────────────────────────────────

    async fn bulk_read(
        &self,
        component_id: &str,
        data_ids: &[String],
    ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
        self.backend_for(component_id)?
            .bulk_read(component_id, data_ids)
            .await
    }

    async fn bulk_write(
        &self,
        component_id: &str,
        items: &[SovdBulkWriteItem],
    ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
        self.backend_for(component_id)?
            .bulk_write(component_id, items)
            .await
    }

    // ── Groups — aggregate from all backends ────────────────────────────────

    fn list_groups(&self) -> Vec<SovdGroup> {
        self.backends.iter().flat_map(|b| b.list_groups()).collect()
    }

    fn get_group(&self, group_id: &str) -> Option<SovdGroup> {
        self.backends.iter().find_map(|b| b.get_group(group_id))
    }

    // ── Extended diagnostics — dispatch to owning backend ───────────────────

    async fn io_control(
        &self,
        component_id: &str,
        data_id: &str,
        control: &str,
        value: Option<&[u8]>,
    ) -> Result<serde_json::Value, DiagServiceError> {
        self.backend_for(component_id)?
            .io_control(component_id, data_id, control, value)
            .await
    }

    async fn communication_control(
        &self,
        component_id: &str,
        control_type: &str,
        communication_type: u8,
    ) -> Result<(), DiagServiceError> {
        self.backend_for(component_id)?
            .communication_control(component_id, control_type, communication_type)
            .await
    }

    async fn dtc_setting(&self, component_id: &str, setting: &str) -> Result<(), DiagServiceError> {
        self.backend_for(component_id)?
            .dtc_setting(component_id, setting)
            .await
    }

    async fn read_memory(
        &self,
        component_id: &str,
        address: u32,
        size: u32,
    ) -> Result<Vec<u8>, DiagServiceError> {
        self.backend_for(component_id)?
            .read_memory(component_id, address, size)
            .await
    }

    async fn write_memory(
        &self,
        component_id: &str,
        address: u32,
        data: &[u8],
    ) -> Result<(), DiagServiceError> {
        self.backend_for(component_id)?
            .write_memory(component_id, address, data)
            .await
    }

    async fn flash(
        &self,
        component_id: &str,
        firmware: &[u8],
        memory_address: u32,
    ) -> Result<serde_json::Value, DiagServiceError> {
        self.backend_for(component_id)?
            .flash(component_id, firmware, memory_address)
            .await
    }

    // ── Keepalive — aggregate from all backends ─────────────────────────────

    fn active_keepalives(&self) -> Vec<String> {
        self.backends
            .iter()
            .flat_map(|b| b.active_keepalives())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_interfaces::sovd::SovdConnectionState;

    /// Minimal mock backend for testing the router
    struct MockBackend {
        components: Vec<SovdComponent>,
    }

    impl MockBackend {
        fn new(ids: &[&str]) -> Self {
            Self {
                components: ids
                    .iter()
                    .map(|id| SovdComponent {
                        id: id.to_string(),
                        name: id.to_string(),
                        category: "ecu".to_string(),
                        description: None,
                        connection_state: SovdConnectionState::Disconnected,
                    })
                    .collect(),
            }
        }
    }

    #[async_trait]
    impl ComponentBackend for MockBackend {
        fn name(&self) -> &str {
            "mock"
        }
        fn list_components(&self) -> Vec<SovdComponent> {
            self.components.clone()
        }
        fn get_component(&self, id: &str) -> Option<SovdComponent> {
            self.components.iter().find(|c| c.id == id).cloned()
        }
        async fn connect(&self, _: &str) -> Result<(), DiagServiceError> {
            Ok(())
        }
        async fn disconnect(&self, _: &str) -> Result<(), DiagServiceError> {
            Ok(())
        }
        fn list_data(&self, _: &str) -> Result<Vec<SovdDataCatalogEntry>, DiagServiceError> {
            Ok(vec![])
        }
        async fn read_data(&self, _: &str, _: &str) -> Result<serde_json::Value, DiagServiceError> {
            Ok(serde_json::json!({}))
        }
        async fn write_data(&self, _: &str, _: &str, _: &[u8]) -> Result<(), DiagServiceError> {
            Ok(())
        }
        async fn read_faults(&self, _: &str) -> Result<Vec<SovdFault>, DiagServiceError> {
            Ok(vec![])
        }
        async fn clear_faults(&self, _: &str) -> Result<(), DiagServiceError> {
            Ok(())
        }
        fn list_operations(&self, _: &str) -> Result<Vec<SovdOperation>, DiagServiceError> {
            Ok(vec![])
        }
        async fn execute_operation(
            &self,
            _: &str,
            _: &str,
            _: Option<&[u8]>,
        ) -> Result<serde_json::Value, DiagServiceError> {
            Ok(serde_json::json!({}))
        }
        fn get_capabilities(&self, _: &str) -> Result<SovdCapabilities, DiagServiceError> {
            Ok(SovdCapabilities {
                component_id: String::new(),
                supported_categories: vec![],
                data_count: 0,
                operation_count: 0,
                features: vec![],
            })
        }
        fn get_mode(&self, id: &str) -> Result<SovdMode, DiagServiceError> {
            Ok(SovdMode {
                component_id: id.to_string(),
                current_mode: "default".into(),
                available_modes: vec![],
            })
        }
        async fn set_mode(&self, _: &str, _: &str) -> Result<(), DiagServiceError> {
            Ok(())
        }
        async fn read_config(&self, id: &str) -> Result<SovdComponentConfig, DiagServiceError> {
            Ok(SovdComponentConfig {
                component_id: id.to_string(),
                parameters: serde_json::json!({}),
            })
        }
        async fn write_config(&self, _: &str, _: &str, _: &[u8]) -> Result<(), DiagServiceError> {
            Ok(())
        }
        async fn bulk_read(
            &self,
            _: &str,
            _: &[String],
        ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
            Ok(vec![])
        }
        async fn bulk_write(
            &self,
            _: &str,
            _: &[SovdBulkWriteItem],
        ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
            Ok(vec![])
        }
        fn list_groups(&self) -> Vec<SovdGroup> {
            vec![]
        }
        fn get_group(&self, _: &str) -> Option<SovdGroup> {
            None
        }
        async fn io_control(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Option<&[u8]>,
        ) -> Result<serde_json::Value, DiagServiceError> {
            Ok(serde_json::json!({}))
        }
        async fn communication_control(
            &self,
            _: &str,
            _: &str,
            _: u8,
        ) -> Result<(), DiagServiceError> {
            Ok(())
        }
        async fn dtc_setting(&self, _: &str, _: &str) -> Result<(), DiagServiceError> {
            Ok(())
        }
        async fn read_memory(&self, _: &str, _: u32, _: u32) -> Result<Vec<u8>, DiagServiceError> {
            Ok(vec![])
        }
        async fn write_memory(&self, _: &str, _: u32, _: &[u8]) -> Result<(), DiagServiceError> {
            Ok(())
        }
        async fn flash(
            &self,
            _: &str,
            _: &[u8],
            _: u32,
        ) -> Result<serde_json::Value, DiagServiceError> {
            Ok(serde_json::json!({}))
        }
    }

    #[test]
    fn router_aggregates_components() {
        let b1: Arc<dyn ComponentBackend> = Arc::new(MockBackend::new(&["hpc", "adas"]));
        let b2: Arc<dyn ComponentBackend> = Arc::new(MockBackend::new(&["brake"]));
        let router = ComponentRouter::new(vec![b1, b2]);

        let components = router.list_components();
        assert_eq!(components.len(), 3);
    }

    #[test]
    fn router_dispatches_to_correct_backend() {
        let b1: Arc<dyn ComponentBackend> = Arc::new(MockBackend::new(&["hpc"]));
        let b2: Arc<dyn ComponentBackend> = Arc::new(MockBackend::new(&["brake"]));
        let router = ComponentRouter::new(vec![b1, b2]);

        assert!(router.handles_component("hpc"));
        assert!(router.handles_component("brake"));
        assert!(!router.handles_component("nonexistent"));
    }

    #[test]
    fn router_not_found_for_unknown_component() {
        let router = ComponentRouter::new(vec![]);
        assert!(router.backend_for("x").is_err());
    }

    #[tokio::test]
    async fn router_connect_dispatches() {
        let b: Arc<dyn ComponentBackend> = Arc::new(MockBackend::new(&["hpc"]));
        let router = ComponentRouter::new(vec![b]);
        assert!(router.connect("hpc").await.is_ok());
        assert!(router.connect("unknown").await.is_err());
    }
}

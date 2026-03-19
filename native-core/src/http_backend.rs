// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// SovdHttpBackend — SOVD HTTP Client (standard-conformant gateway backend)
//
// Forwards SOVD REST requests to an external SOVD server (e.g. the CDA).
// This implements the OpenSOVD architecture where:
//
//   SOVD Server → SOVD Gateway → [HTTP] → CDA (external)
//                                          └→ /sovd/v1/components/...
//
// The CDA already exposes a full SOVD REST API. This backend simply proxies
// requests to it, avoiding any UDS/DoIP logic in the SOVD Server itself.
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use native_interfaces::{
    sovd::{
        Collection, SovdBulkDataCategory, SovdBulkDataItem, SovdBulkReadRequest, SovdBulkWriteItem,
        SovdCapabilities, SovdComponent, SovdComponentConfig, SovdConnectionState,
        SovdDataCatalogEntry, SovdFault, SovdGroup, SovdMode, SovdOperation,
    },
    ComponentBackend, DiagServiceError,
};

/// Configuration for connecting to an external SOVD server (e.g. CDA)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdHttpBackendConfig {
    /// Base URL of the external SOVD server, e.g. "http://cda:20002"
    pub base_url: String,
    /// SOVD API path prefix, e.g. "/sovd/v1" or "/vehicle/v15"
    #[serde(default = "default_api_prefix")]
    pub api_prefix: String,
    /// Human-readable name for this backend
    #[serde(default = "default_backend_name")]
    pub name: String,
    /// HTTP request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Optional bearer token for authentication
    #[serde(default)]
    pub bearer_token: Option<String>,
    /// Component IDs managed by this backend (discovered if empty)
    #[serde(default)]
    pub component_ids: Vec<String>,
}

fn default_api_prefix() -> String {
    "/sovd/v1".to_owned()
}
fn default_backend_name() -> String {
    "SovdHttpBackend".to_owned()
}
fn default_timeout() -> u64 {
    30
}

impl Default for SovdHttpBackendConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:20002".to_owned(),
            api_prefix: default_api_prefix(),
            name: default_backend_name(),
            timeout_secs: default_timeout(),
            bearer_token: None,
            component_ids: vec![],
        }
    }
}

/// HTTP client that forwards SOVD requests to an external CDA or SOVD server.
pub struct SovdHttpBackend {
    config: SovdHttpBackendConfig,
    client: reqwest::Client,
    /// Cached component list (refreshed on connect/discover)
    components_cache: Arc<RwLock<Vec<SovdComponent>>>,
    groups_cache: Arc<RwLock<Vec<SovdGroup>>>,
}

impl SovdHttpBackend {
    /// # Errors
    /// Returns an error string if the HTTP client cannot be built.
    pub fn new(config: SovdHttpBackendConfig) -> Result<Self, String> {
        let mut builder =
            reqwest::Client::builder().timeout(Duration::from_secs(config.timeout_secs));

        if let Some(ref token) = config.bearer_token {
            let mut headers = reqwest::header::HeaderMap::new();
            let val = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| format!("Invalid bearer token: {e}"))?;
            headers.insert(reqwest::header::AUTHORIZATION, val);
            builder = builder.default_headers(headers);
        }

        let client = builder
            .build()
            .map_err(|e| format!("HTTP client build: {e}"))?;

        info!(
            name = %config.name,
            base_url = %config.base_url,
            api_prefix = %config.api_prefix,
            "SovdHttpBackend created"
        );

        Ok(Self {
            config,
            client,
            components_cache: Arc::new(RwLock::new(vec![])),
            groups_cache: Arc::new(RwLock::new(vec![])),
        })
    }

    /// Full URL for a SOVD API path
    fn url(&self, path: &str) -> String {
        format!("{}{}{}", self.config.base_url, self.config.api_prefix, path)
    }

    /// Discover components from the external server and cache them.
    ///
    /// # Errors
    /// Returns `DiagServiceError` if the HTTP request fails or the response cannot be parsed.
    pub async fn discover(&self) -> Result<(), DiagServiceError> {
        let url = self.url("/components");
        debug!(url = %url, "Discovering components from external server");

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| DiagServiceError::SendFailed(format!("HTTP GET {url}: {e}")))?;

        if !resp.status().is_success() {
            return Err(DiagServiceError::SendFailed(format!(
                "HTTP GET {url} returned {}",
                resp.status()
            )));
        }

        let collection: Collection<SovdComponent> = resp
            .json()
            .await
            .map_err(|e| DiagServiceError::BadPayload(format!("Parse components: {e}")))?;

        {
            let mut cache = self.components_cache.write();
            *cache = collection.value;
            info!(
                name = %self.config.name,
                count = cache.len(),
                "Discovered components from external server"
            );
        }

        // Also discover groups
        let groups_url = self.url("/groups");
        match self.client.get(&groups_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(groups_col) = resp.json::<Collection<SovdGroup>>().await {
                    let mut gcache = self.groups_cache.write();
                    *gcache = groups_col.value;
                }
            }
            _ => debug!("No groups endpoint on external server (optional)"),
        }

        Ok(())
    }

    /// GET request returning JSON
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, DiagServiceError> {
        let url = self.url(path);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| DiagServiceError::SendFailed(format!("HTTP GET {url}: {e}")))?;
        Self::handle_response(resp, &url).await
    }

    /// POST request with JSON body returning JSON
    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<T, DiagServiceError> {
        let url = self.url(path);
        let resp = self
            .client
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| DiagServiceError::SendFailed(format!("HTTP POST {url}: {e}")))?;
        Self::handle_response(resp, &url).await
    }

    /// PUT request with JSON body returning status
    async fn put_json(&self, path: &str, body: &impl Serialize) -> Result<(), DiagServiceError> {
        let url = self.url(path);
        let resp = self
            .client
            .put(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| DiagServiceError::SendFailed(format!("HTTP PUT {url}: {e}")))?;
        Self::handle_status(resp, &url).await
    }

    /// DELETE request returning status
    async fn delete(&self, path: &str) -> Result<(), DiagServiceError> {
        let url = self.url(path);
        let resp = self
            .client
            .delete(&url)
            .send()
            .await
            .map_err(|e| DiagServiceError::SendFailed(format!("HTTP DELETE {url}: {e}")))?;
        Self::handle_status(resp, &url).await
    }

    /// POST request returning status only
    async fn post_empty(&self, path: &str) -> Result<(), DiagServiceError> {
        let url = self.url(path);
        let resp = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| DiagServiceError::SendFailed(format!("HTTP POST {url}: {e}")))?;
        Self::handle_status(resp, &url).await
    }

    /// Handle HTTP response → parse JSON or map error
    async fn handle_response<T: serde::de::DeserializeOwned>(
        resp: reqwest::Response,
        url: &str,
    ) -> Result<T, DiagServiceError> {
        let status = resp.status();
        if status.is_success() {
            resp.json::<T>().await.map_err(|e| {
                DiagServiceError::BadPayload(format!("Parse response from {url}: {e}"))
            })
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(Self::map_http_error(status.as_u16(), url, &body))
        }
    }

    /// Handle HTTP response → check status only
    async fn handle_status(resp: reqwest::Response, url: &str) -> Result<(), DiagServiceError> {
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(Self::map_http_error(status.as_u16(), url, &body))
        }
    }

    /// Map HTTP status codes to DiagServiceError
    fn map_http_error(status: u16, url: &str, body: &str) -> DiagServiceError {
        match status {
            400 => DiagServiceError::InvalidRequest(format!("{url}: {body}")),
            403 => DiagServiceError::AccessDenied(format!("{url}: {body}")),
            404 => DiagServiceError::NotFound(Some(format!("{url}: {body}"))),
            408 | 504 => DiagServiceError::Timeout,
            502 => DiagServiceError::SendFailed(format!("Bad gateway {url}: {body}")),
            _ => DiagServiceError::SendFailed(format!("HTTP {status} from {url}: {body}")),
        }
    }
}

#[async_trait]
impl ComponentBackend for SovdHttpBackend {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn list_components(&self) -> Vec<SovdComponent> {
        let cache = self.components_cache.read();
        if cache.is_empty() && !self.config.component_ids.is_empty() {
            // Return stub components from config if not yet discovered
            return self
                .config
                .component_ids
                .iter()
                .map(|id| SovdComponent {
                    id: id.clone(),
                    name: id.clone(),
                    category: "ecu".to_owned(),
                    description: Some(format!("via {}", self.config.name)),
                    connection_state: SovdConnectionState::Disconnected,
                    software_version: None,
                    hardware_variant: None,
                    installation_variant: None,
                })
                .collect();
        }
        cache.clone()
    }

    fn get_component(&self, component_id: &str) -> Option<SovdComponent> {
        let cache = self.components_cache.read();
        if let Some(c) = cache.iter().find(|c| c.id == component_id) {
            return Some(c.clone());
        }
        // Fallback: check configured component_ids
        if self.config.component_ids.contains(&component_id.to_owned()) {
            return Some(SovdComponent {
                id: component_id.to_owned(),
                name: component_id.to_owned(),
                category: "ecu".to_owned(),
                description: Some(format!("via {}", self.config.name)),
                connection_state: SovdConnectionState::Disconnected,
                software_version: None,
                hardware_variant: None,
                installation_variant: None,
            });
        }
        None
    }

    async fn connect(&self, component_id: &str) -> Result<(), DiagServiceError> {
        self.post_empty(&format!("/components/{component_id}/connect"))
            .await
    }

    async fn disconnect(&self, component_id: &str) -> Result<(), DiagServiceError> {
        self.post_empty(&format!("/components/{component_id}/disconnect"))
            .await
    }

    fn list_data(&self, component_id: &str) -> Result<Vec<SovdDataCatalogEntry>, DiagServiceError> {
        // Sync method — return empty, caller should use async version via routes
        // In practice, the router could cache this from discovery
        warn!(
            "list_data called synchronously on HTTP backend — returning empty for '{component_id}'"
        );
        Ok(vec![])
    }

    async fn read_data(
        &self,
        component_id: &str,
        data_id: &str,
    ) -> Result<serde_json::Value, DiagServiceError> {
        self.get_json(&format!("/components/{component_id}/data/{data_id}"))
            .await
    }

    async fn write_data(
        &self,
        component_id: &str,
        data_id: &str,
        value: &[u8],
    ) -> Result<(), DiagServiceError> {
        self.put_json(
            &format!("/components/{component_id}/data/{data_id}"),
            &serde_json::json!({ "value": hex::encode(value) }),
        )
        .await
    }

    async fn read_faults(&self, component_id: &str) -> Result<Vec<SovdFault>, DiagServiceError> {
        let collection: Collection<SovdFault> = self
            .get_json(&format!("/components/{component_id}/faults"))
            .await?;
        Ok(collection.value)
    }

    async fn clear_faults(&self, component_id: &str) -> Result<(), DiagServiceError> {
        self.delete(&format!("/components/{component_id}/faults"))
            .await
    }

    fn list_operations(&self, component_id: &str) -> Result<Vec<SovdOperation>, DiagServiceError> {
        warn!("list_operations called synchronously on HTTP backend — returning empty for '{component_id}'");
        Ok(vec![])
    }

    async fn execute_operation(
        &self,
        component_id: &str,
        operation_id: &str,
        params: Option<&[u8]>,
    ) -> Result<serde_json::Value, DiagServiceError> {
        let body = serde_json::json!({
            "params": params.map(hex::encode),
        });
        self.post_json(
            &format!("/components/{component_id}/operations/{operation_id}"),
            &body,
        )
        .await
    }

    fn get_capabilities(&self, component_id: &str) -> Result<SovdCapabilities, DiagServiceError> {
        // Sync — return minimal capabilities, real data comes from async route
        Ok(SovdCapabilities {
            component_id: component_id.to_owned(),
            supported_categories: vec!["ecu".to_owned()],
            data_count: 0,
            operation_count: 0,
            features: vec![],
        })
    }

    fn get_mode(&self, component_id: &str) -> Result<SovdMode, DiagServiceError> {
        Ok(SovdMode {
            component_id: component_id.to_owned(),
            current_mode: "unknown".to_owned(),
            available_modes: vec!["default".into(), "extended".into(), "programming".into()],
            mode_descriptors: vec![],
            active_since: None,
        })
    }

    async fn set_mode(&self, component_id: &str, mode: &str) -> Result<(), DiagServiceError> {
        self.post_json::<serde_json::Value>(
            &format!("/components/{component_id}/modes"),
            &serde_json::json!({ "mode": mode }),
        )
        .await?;
        Ok(())
    }

    async fn read_config(
        &self,
        component_id: &str,
    ) -> Result<SovdComponentConfig, DiagServiceError> {
        self.get_json(&format!("/components/{component_id}/configurations"))
            .await
    }

    async fn write_config(
        &self,
        component_id: &str,
        param_name: &str,
        value: &[u8],
    ) -> Result<(), DiagServiceError> {
        self.put_json(
            &format!("/components/{component_id}/configurations"),
            &serde_json::json!({ "name": param_name, "value": hex::encode(value) }),
        )
        .await
    }

    async fn bulk_read(
        &self,
        component_id: &str,
        data_ids: &[String],
        category: Option<SovdBulkDataCategory>,
    ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
        let body = SovdBulkReadRequest {
            data_ids: data_ids.to_vec(),
            category,
        };
        let collection: Collection<SovdBulkDataItem> = self
            .post_json(&format!("/components/{component_id}/data/bulk-read"), &body)
            .await?;
        Ok(collection.value)
    }

    async fn bulk_write(
        &self,
        component_id: &str,
        items: &[SovdBulkWriteItem],
    ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
        let collection: Collection<SovdBulkDataItem> = self
            .post_json(
                &format!("/components/{component_id}/data/bulk-write"),
                &items,
            )
            .await?;
        Ok(collection.value)
    }

    fn list_groups(&self) -> Vec<SovdGroup> {
        self.groups_cache.read().clone()
    }

    fn get_group(&self, group_id: &str) -> Option<SovdGroup> {
        self.groups_cache
            .read()
            .iter()
            .find(|g| g.id == group_id)
            .cloned()
    }
}

// ── ExtendedDiagBackend — UDS vendor extension methods via HTTP ──────────

#[async_trait]
impl native_interfaces::ExtendedDiagBackend for SovdHttpBackend {
    fn handles_component(&self, component_id: &str) -> bool {
        ComponentBackend::handles_component(self, component_id)
    }

    async fn io_control(
        &self,
        component_id: &str,
        data_id: &str,
        control: &str,
        value: Option<&[u8]>,
    ) -> Result<serde_json::Value, DiagServiceError> {
        self.post_json(
            &format!("/components/{component_id}/io/{data_id}"),
            &serde_json::json!({
                "control": control,
                "value": value.map(hex::encode),
            }),
        )
        .await
    }

    async fn communication_control(
        &self,
        component_id: &str,
        control_type: &str,
        communication_type: u8,
    ) -> Result<(), DiagServiceError> {
        self.post_json::<serde_json::Value>(
            &format!("/components/{component_id}/comm-control"),
            &serde_json::json!({
                "control_type": control_type,
                "communication_type": format!("{communication_type:02X}"),
            }),
        )
        .await?;
        Ok(())
    }

    async fn dtc_setting(&self, component_id: &str, setting: &str) -> Result<(), DiagServiceError> {
        self.post_json::<serde_json::Value>(
            &format!("/components/{component_id}/dtc-setting"),
            &serde_json::json!({ "setting": setting }),
        )
        .await?;
        Ok(())
    }

    async fn read_memory(
        &self,
        component_id: &str,
        address: u32,
        size: u32,
    ) -> Result<Vec<u8>, DiagServiceError> {
        let val: serde_json::Value = self
            .get_json(&format!(
                "/components/{component_id}/memory?address=0x{address:08X}&size={size}"
            ))
            .await?;
        let hex_str = val.get("value").and_then(|v| v.as_str()).unwrap_or("");
        hex::decode(hex_str)
            .map_err(|e| DiagServiceError::BadPayload(format!("Invalid memory hex: {e}")))
    }

    async fn write_memory(
        &self,
        component_id: &str,
        address: u32,
        data: &[u8],
    ) -> Result<(), DiagServiceError> {
        self.put_json(
            &format!("/components/{component_id}/memory"),
            &serde_json::json!({
                "address": format!("0x{address:08X}"),
                "value": hex::encode(data),
            }),
        )
        .await
    }

    async fn flash(
        &self,
        component_id: &str,
        firmware: &[u8],
        memory_address: u32,
    ) -> Result<serde_json::Value, DiagServiceError> {
        use base64::Engine;
        let firmware_b64 = base64::engine::general_purpose::STANDARD.encode(firmware);
        self.post_json(
            &format!("/components/{component_id}/flash"),
            &serde_json::json!({
                "firmware_data": firmware_b64,
                "memory_address": memory_address,
            }),
        )
        .await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use native_interfaces::ComponentBackend;

    fn test_config() -> SovdHttpBackendConfig {
        SovdHttpBackendConfig {
            base_url: "http://127.0.0.1:9999".to_owned(),
            api_prefix: "/sovd/v1".to_owned(),
            name: "Test CDA".to_owned(),
            timeout_secs: 5,
            bearer_token: None,
            component_ids: vec!["brake-ecu".to_owned(), "eps-ecu".to_owned()],
        }
    }

    #[test]
    fn new_creates_backend_with_config() {
        let backend = SovdHttpBackend::new(test_config()).unwrap();
        assert_eq!(backend.name(), "Test CDA");
    }

    #[test]
    fn url_builds_correctly() {
        let backend = SovdHttpBackend::new(test_config()).unwrap();
        assert_eq!(
            backend.url("/components"),
            "http://127.0.0.1:9999/sovd/v1/components"
        );
        assert_eq!(
            backend.url("/components/brake-ecu/data"),
            "http://127.0.0.1:9999/sovd/v1/components/brake-ecu/data"
        );
    }

    #[test]
    fn map_http_error_maps_status_codes() {
        let err = SovdHttpBackend::map_http_error(400, "/test", "bad");
        assert!(matches!(err, DiagServiceError::InvalidRequest(_)));

        let err = SovdHttpBackend::map_http_error(403, "/test", "denied");
        assert!(matches!(err, DiagServiceError::AccessDenied(_)));

        let err = SovdHttpBackend::map_http_error(404, "/test", "gone");
        assert!(matches!(err, DiagServiceError::NotFound(_)));

        let err = SovdHttpBackend::map_http_error(408, "/test", "timeout");
        assert!(matches!(err, DiagServiceError::Timeout));

        let err = SovdHttpBackend::map_http_error(504, "/test", "gw timeout");
        assert!(matches!(err, DiagServiceError::Timeout));

        let err = SovdHttpBackend::map_http_error(502, "/test", "bad gw");
        assert!(matches!(err, DiagServiceError::SendFailed(_)));

        let err = SovdHttpBackend::map_http_error(500, "/test", "internal");
        assert!(matches!(err, DiagServiceError::SendFailed(_)));
    }

    #[test]
    fn list_components_returns_stubs_from_config() {
        let backend = SovdHttpBackend::new(test_config()).unwrap();
        let components = backend.list_components();
        assert_eq!(components.len(), 2);
        assert_eq!(components[0].id, "brake-ecu");
        assert_eq!(components[1].id, "eps-ecu");
        assert!(components[0]
            .description
            .as_ref()
            .unwrap()
            .contains("Test CDA"));
    }

    #[test]
    fn handles_component_for_configured_ids() {
        let backend = SovdHttpBackend::new(test_config()).unwrap();
        assert!(backend.handles_component("brake-ecu"));
        assert!(backend.handles_component("eps-ecu"));
        assert!(!backend.handles_component("unknown"));
    }

    #[test]
    fn get_component_returns_stub_for_configured_id() {
        let backend = SovdHttpBackend::new(test_config()).unwrap();
        let comp = backend.get_component("brake-ecu");
        assert!(comp.is_some());
        assert_eq!(comp.unwrap().id, "brake-ecu");

        assert!(backend.get_component("nonexistent").is_none());
    }

    #[test]
    fn list_components_returns_empty_without_config_ids() {
        let config = SovdHttpBackendConfig {
            component_ids: vec![],
            ..test_config()
        };
        let backend = SovdHttpBackend::new(config).unwrap();
        assert!(backend.list_components().is_empty());
    }

    #[test]
    fn default_config_has_sane_defaults() {
        let config = SovdHttpBackendConfig::default();
        assert_eq!(config.api_prefix, "/sovd/v1");
        assert_eq!(config.timeout_secs, 30);
        assert!(config.bearer_token.is_none());
        assert!(config.component_ids.is_empty());
    }

    // ── Integration test with mock CDA (axum) ──────────────────────────────

    #[tokio::test]
    async fn discover_populates_cache_from_mock_cda() {
        use axum::{routing::get, Json, Router};

        // Spin up a minimal mock CDA server
        let mock_cda = Router::new()
            .route("/sovd/v1/components", get(|| async {
                Json(serde_json::json!({
                    "value": [
                        { "id": "ecu-a", "name": "ECU A", "category": "ecu", "connectionState": "connected" },
                        { "id": "ecu-b", "name": "ECU B", "category": "ecu", "connectionState": "disconnected" }
                    ],
                    "@odata.count": 2
                }))
            }))
            .route("/sovd/v1/groups", get(|| async {
                Json(serde_json::json!({
                    "value": [{ "id": "powertrain", "name": "Powertrain", "description": "Engine", "componentIds": ["ecu-a"] }],
                    "@odata.count": 1
                }))
            }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, mock_cda).await.unwrap();
        });

        // Create backend pointing to mock CDA
        let config = SovdHttpBackendConfig {
            base_url: format!("http://{addr}"),
            component_ids: vec![],
            ..test_config()
        };
        let backend = SovdHttpBackend::new(config).unwrap();

        // Before discover: no cached components
        assert!(backend.list_components().is_empty());

        // Discover
        backend.discover().await.unwrap();

        // After discover: components populated from mock CDA
        let components = backend.list_components();
        assert_eq!(components.len(), 2);
        assert_eq!(components[0].id, "ecu-a");
        assert_eq!(components[1].id, "ecu-b");
        assert!(backend.handles_component("ecu-a"));
        assert!(!backend.handles_component("nonexistent"));

        // Groups also discovered
        let groups = backend.list_groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "powertrain");
    }

    #[tokio::test]
    async fn discover_handles_unreachable_server() {
        let config = SovdHttpBackendConfig {
            base_url: "http://127.0.0.1:1".to_owned(), // nobody listening
            timeout_secs: 1,
            ..test_config()
        };
        let backend = SovdHttpBackend::new(config).unwrap();
        let result = backend.discover().await;
        assert!(result.is_err());
    }
}

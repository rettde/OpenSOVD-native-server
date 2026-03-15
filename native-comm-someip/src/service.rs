// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// vSomeIP runtime + service proxy
//
// Architecture:
//   - SomeIpRuntime: manages the vSomeIP application lifecycle
//   - SomeIpServiceProxy: proxy to a remote SOME/IP service
//
// When the "vsomeip-ffi" feature is disabled, these are stub implementations
// that allow compilation without libvsomeip3 installed.
// When enabled, the real vsomeip3 C++ library is used via FFI.
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex, RwLock};
use tracing::{debug, info, warn};

use native_interfaces::SomeIpError;

use super::config::{ServiceDefinition, SomeIpConfig};

#[cfg(feature = "vsomeip-ffi")]
use crate::runtime::VsomeipApplication;

/// SOME/IP message payload
#[derive(Debug, Clone)]
pub struct SomeIpMessage {
    pub service_id: u16,
    pub method_id: u16,
    pub client_id: u16,
    pub session_id: u16,
    pub payload: Vec<u8>,
}

/// Event notification from a subscribed eventgroup
#[derive(Debug, Clone)]
pub struct SomeIpEvent {
    pub service_id: u16,
    pub event_id: u16,
    pub payload: Vec<u8>,
}

/// Service availability change
#[derive(Debug, Clone)]
pub struct ServiceAvailability {
    pub service_id: u16,
    pub instance_id: u16,
    pub available: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// SomeIpRuntime — manages the vSomeIP application lifecycle
// ─────────────────────────────────────────────────────────────────────────────

type ProxyMap = HashMap<(u16, u16), Arc<SomeIpServiceProxy>>;

pub struct SomeIpRuntime {
    config: SomeIpConfig,
    availability_tx: broadcast::Sender<ServiceAvailability>,
    proxies: Arc<RwLock<ProxyMap>>,
    running: Arc<Mutex<bool>>,
    #[cfg(feature = "vsomeip-ffi")]
    vsomeip_app: Arc<std::sync::Mutex<Option<Arc<VsomeipApplication>>>>,
}

impl SomeIpRuntime {
    pub fn new(config: SomeIpConfig) -> Self {
        let (availability_tx, _) = broadcast::channel(64);
        Self {
            config,
            availability_tx,
            proxies: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(Mutex::new(false)),
            #[cfg(feature = "vsomeip-ffi")]
            vsomeip_app: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Initialize the vSomeIP runtime.
    /// With vsomeip-ffi: calls vsomeip::runtime::get()->create_application()
    /// Without: logs a warning and operates in stub mode.
    #[tracing::instrument(skip(self), fields(app = %self.config.application_name))]
    pub async fn init(&self) -> Result<(), SomeIpError> {
        #[cfg(feature = "vsomeip-ffi")]
        {
            let vsomeip = VsomeipApplication::new(
                &self.config.application_name,
                self.availability_tx.clone(),
            )
            .map_err(|e| SomeIpError::NotAvailable(e))?;

            vsomeip.init().map_err(|e| SomeIpError::NotAvailable(e))?;

            *self.vsomeip_app.lock().unwrap() = Some(Arc::new(vsomeip));
            *self.running.lock().await = true;
            info!("SOME/IP runtime initialized (vsomeip-ffi)");
            return Ok(());
        }

        #[cfg(not(feature = "vsomeip-ffi"))]
        {
            warn!(
                "vSomeIP FFI not enabled — running in stub mode. \
                 Enable feature 'vsomeip-ffi' and install libvsomeip3 for real SOME/IP."
            );
            *self.running.lock().await = true;
            info!("SOME/IP runtime initialized (stub mode)");
            Ok(())
        }
    }

    /// Start the vSomeIP event loop (non-blocking, spawns background task)
    pub async fn start(&self) -> Result<(), SomeIpError> {
        if !*self.running.lock().await {
            return Err(SomeIpError::NotAvailable(
                "Runtime not initialized".to_owned(),
            ));
        }

        #[cfg(feature = "vsomeip-ffi")]
        {
            let vsomeip = self.get_vsomeip_app()?;

            // Offer services
            for svc in &self.config.offered_services {
                vsomeip.offer_service(
                    svc.service_id,
                    svc.instance_id,
                    svc.major_version,
                    svc.minor_version,
                );
                // Register message handler to receive requests for offered services
                vsomeip.register_message_handler(
                    svc.service_id,
                    svc.instance_id,
                    0xFFFF, // all methods
                );
                // Register events for eventgroups
                for eg in &svc.eventgroups {
                    for &event_id in &eg.events {
                        vsomeip.offer_event(
                            svc.service_id,
                            svc.instance_id,
                            event_id,
                            &[eg.eventgroup_id],
                            false,
                        );
                    }
                }
            }

            // Request consumed services
            for svc in &self.config.consumed_services {
                vsomeip.request_service(
                    svc.service_id,
                    svc.instance_id,
                    svc.major_version,
                    svc.minor_version,
                );
                vsomeip.register_availability_handler(svc.service_id, svc.instance_id);
                // Register message handler for responses
                vsomeip.register_message_handler(svc.service_id, svc.instance_id, 0xFFFF);
                // Subscribe to eventgroups
                for eg in &svc.eventgroups {
                    vsomeip.subscribe(
                        svc.service_id,
                        svc.instance_id,
                        eg.eventgroup_id,
                        svc.major_version,
                    );
                }

                let proxy = Arc::new(SomeIpServiceProxy::new_with_vsomeip(
                    svc.clone(),
                    Arc::clone(&vsomeip),
                ));
                self.proxies
                    .write()
                    .await
                    .insert((svc.service_id, svc.instance_id), proxy);
            }

            // Start the vsomeip event loop
            vsomeip.start().map_err(|e| SomeIpError::NotAvailable(e))?;

            info!("SOME/IP runtime started (vsomeip-ffi)");
            return Ok(());
        }

        #[cfg(not(feature = "vsomeip-ffi"))]
        {
            // Register offered services (stub — log only)
            for svc in &self.config.offered_services {
                info!(
                    service = %format!("0x{:04X}", svc.service_id),
                    instance = %format!("0x{:04X}", svc.instance_id),
                    "Offering SOME/IP service"
                );
            }

            // Request consumed services (stub)
            for svc in &self.config.consumed_services {
                info!(
                    service = %format!("0x{:04X}", svc.service_id),
                    instance = %format!("0x{:04X}", svc.instance_id),
                    "Requesting SOME/IP service"
                );
                let proxy = Arc::new(SomeIpServiceProxy::new(svc.clone()));
                self.proxies
                    .write()
                    .await
                    .insert((svc.service_id, svc.instance_id), proxy);
            }

            info!("SOME/IP runtime started");
            Ok(())
        }
    }

    /// Stop the vSomeIP runtime
    pub async fn stop(&self) {
        #[cfg(feature = "vsomeip-ffi")]
        {
            if let Some(vsomeip) = self.vsomeip_app.lock().unwrap().take() {
                vsomeip.stop();
            }
        }
        *self.running.lock().await = false;
        info!("SOME/IP runtime stopped");
    }

    /// Get a proxy to a consumed service
    pub async fn get_proxy(
        &self,
        service_id: u16,
        instance_id: u16,
    ) -> Option<Arc<SomeIpServiceProxy>> {
        self.proxies
            .read()
            .await
            .get(&(service_id, instance_id))
            .cloned()
    }

    /// Subscribe to service availability changes
    pub fn subscribe_availability(&self) -> broadcast::Receiver<ServiceAvailability> {
        self.availability_tx.subscribe()
    }

    pub fn config(&self) -> &SomeIpConfig {
        &self.config
    }

    pub async fn is_running(&self) -> bool {
        *self.running.lock().await
    }

    #[cfg(feature = "vsomeip-ffi")]
    fn get_vsomeip_app(&self) -> Result<Arc<VsomeipApplication>, SomeIpError> {
        self.vsomeip_app
            .lock()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or_else(|| SomeIpError::NotAvailable("vsomeip not initialized".to_owned()))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SomeIpServiceProxy — proxy to a remote SOME/IP service
// ─────────────────────────────────────────────────────────────────────────────

pub struct SomeIpServiceProxy {
    definition: ServiceDefinition,
    available: Arc<RwLock<bool>>,
    event_tx: broadcast::Sender<SomeIpEvent>,
    #[cfg(feature = "vsomeip-ffi")]
    vsomeip_app: Option<Arc<VsomeipApplication>>,
}

impl SomeIpServiceProxy {
    pub fn new(definition: ServiceDefinition) -> Self {
        let (event_tx, _) = broadcast::channel(64);
        Self {
            definition,
            available: Arc::new(RwLock::new(false)),
            event_tx,
            #[cfg(feature = "vsomeip-ffi")]
            vsomeip_app: None,
        }
    }

    #[cfg(feature = "vsomeip-ffi")]
    pub fn new_with_vsomeip(
        definition: ServiceDefinition,
        vsomeip_app: Arc<VsomeipApplication>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(64);
        Self {
            definition,
            available: Arc::new(RwLock::new(false)),
            event_tx,
            vsomeip_app: Some(vsomeip_app),
        }
    }

    /// Mark this service as available/unavailable (called from availability callback)
    pub async fn set_available(&self, available: bool) {
        *self.available.write().await = available;
    }

    /// Send a request to the remote service and wait for a response
    #[tracing::instrument(skip(self, payload), fields(
        service = %format!("0x{:04X}", self.definition.service_id),
        method = %format!("0x{method_id:04X}"),
    ))]
    pub async fn request(&self, method_id: u16, payload: &[u8]) -> Result<Vec<u8>, SomeIpError> {
        if !*self.available.read().await {
            return Err(SomeIpError::NotAvailable(format!(
                "Service 0x{:04X} not available",
                self.definition.service_id
            )));
        }

        #[cfg(feature = "vsomeip-ffi")]
        {
            let app = self.vsomeip_app.as_ref().ok_or_else(|| {
                SomeIpError::NotAvailable("No vsomeip application bound".to_owned())
            })?;

            let result = app
                .request(
                    self.definition.service_id,
                    self.definition.instance_id,
                    method_id,
                    payload,
                    true, // TCP (reliable)
                )
                .await
                .map_err(|e| SomeIpError::RequestFailed {
                    service_id: self.definition.service_id,
                    method_id,
                    details: e,
                })?;

            return Ok(result);
        }

        #[cfg(not(feature = "vsomeip-ffi"))]
        {
            debug!(payload_len = payload.len(), "SOME/IP request (stub)");
            Ok(vec![])
        }
    }

    /// Fire-and-forget message to the remote service
    pub async fn fire_and_forget(&self, method_id: u16, payload: &[u8]) -> Result<(), SomeIpError> {
        if !*self.available.read().await {
            return Err(SomeIpError::NotAvailable(format!(
                "Service 0x{:04X} not available",
                self.definition.service_id
            )));
        }

        #[cfg(feature = "vsomeip-ffi")]
        {
            let app = self.vsomeip_app.as_ref().ok_or_else(|| {
                SomeIpError::NotAvailable("No vsomeip application bound".to_owned())
            })?;

            app.fire_and_forget(
                self.definition.service_id,
                self.definition.instance_id,
                method_id,
                payload,
            )
            .map_err(|e| SomeIpError::RequestFailed {
                service_id: self.definition.service_id,
                method_id,
                details: e,
            })?;

            return Ok(());
        }

        #[cfg(not(feature = "vsomeip-ffi"))]
        {
            debug!(
                service = %format!("0x{:04X}", self.definition.service_id),
                method = %format!("0x{method_id:04X}"),
                payload_len = payload.len(),
                "SOME/IP fire-and-forget (stub)"
            );
            Ok(())
        }
    }

    /// Subscribe to events from this service
    pub fn subscribe_events(&self) -> broadcast::Receiver<SomeIpEvent> {
        self.event_tx.subscribe()
    }

    pub async fn is_available(&self) -> bool {
        *self.available.read().await
    }

    pub fn service_id(&self) -> u16 {
        self.definition.service_id
    }

    pub fn instance_id(&self) -> u16 {
        self.definition.instance_id
    }
}

// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// runtime.rs — Safe Rust wrapper around the vsomeip C FFI
//
// Only compiled when feature "vsomeip-ffi" is enabled.
// Provides VsomeipApplication — a Send+Sync wrapper managing the vsomeip
// application lifecycle, message dispatch, and callback bridging.
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{debug, error, info, warn};

use crate::ffi;
use crate::service::{ServiceAvailability, SomeIpEvent, SomeIpMessage};

// ── Message type constants (matching vsomeip::message_type_e) ───────────────

pub const MT_REQUEST: u8 = 0x00;
pub const MT_REQUEST_NO_RETURN: u8 = 0x01;
pub const MT_NOTIFICATION: u8 = 0x02;
pub const MT_RESPONSE: u8 = 0x80;

// ── Pending request tracking ────────────────────────────────────────────────

struct PendingRequest {
    tx: oneshot::Sender<Result<Vec<u8>, String>>,
}

// ── Callback context (shared between Rust and C callbacks) ──────────────────

struct CallbackContext {
    /// Pending request/response pairs keyed by (client_id, session_id)
    pending: Mutex<HashMap<(u16, u16), PendingRequest>>,
    /// Channel for incoming requests (server mode)
    incoming_tx: mpsc::UnboundedSender<SomeIpMessage>,
    /// Channel for event notifications
    event_tx: broadcast::Sender<SomeIpEvent>,
    /// Channel for availability changes
    availability_tx: broadcast::Sender<ServiceAvailability>,
    /// Monotonic session counter for outgoing requests
    session_counter: Mutex<u16>,
}

impl CallbackContext {
    fn next_session(&self) -> u16 {
        let mut counter = self.session_counter.lock().unwrap();
        *counter = counter.wrapping_add(1);
        if *counter == 0 {
            *counter = 1;
        }
        *counter
    }
}

// ── C callback trampolines ─────────────────────────────────────────────────

/// Trampoline called from C++ for incoming messages.
/// Dispatches to pending requests (responses) or incoming channel (requests/notifications).
unsafe extern "C" fn message_trampoline(
    context: *mut c_void,
    service_id: u16,
    instance_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    message_type: u8,
    return_code: u8,
    payload: *const u8,
    payload_len: u32,
) {
    let ctx = &*(context as *const CallbackContext);

    let data = if !payload.is_null() && payload_len > 0 {
        std::slice::from_raw_parts(payload, payload_len as usize).to_vec()
    } else {
        vec![]
    };

    match message_type {
        MT_RESPONSE => {
            // Match to a pending request
            let mut pending = ctx.pending.lock().unwrap();
            if let Some(req) = pending.remove(&(client_id, session_id)) {
                if return_code == 0 {
                    let _ = req.tx.send(Ok(data));
                } else {
                    let _ = req.tx.send(Err(format!(
                        "SOME/IP error: return_code=0x{return_code:02X}"
                    )));
                }
            } else {
                debug!(
                    client_id,
                    session_id, "Received response for unknown pending request"
                );
            }
        }
        MT_NOTIFICATION => {
            let _ = ctx.event_tx.send(SomeIpEvent {
                service_id,
                event_id: method_id,
                payload: data,
            });
        }
        MT_REQUEST | MT_REQUEST_NO_RETURN => {
            let _ = ctx.incoming_tx.send(SomeIpMessage {
                service_id,
                method_id,
                client_id,
                session_id,
                payload: data,
            });
        }
        _ => {
            warn!(
                message_type,
                service_id, method_id, "Unknown SOME/IP message type"
            );
        }
    }
}

/// Trampoline called from C++ for availability changes.
unsafe extern "C" fn availability_trampoline(
    context: *mut c_void,
    service_id: u16,
    instance_id: u16,
    available: c_int,
) {
    let ctx = &*(context as *const CallbackContext);
    let is_available = available != 0;

    info!(
        service = %format!("0x{service_id:04X}"),
        instance = %format!("0x{instance_id:04X}"),
        available = is_available,
        "Service availability changed"
    );

    let _ = ctx.availability_tx.send(ServiceAvailability {
        service_id,
        instance_id,
        available: is_available,
    });
}

// ── VsomeipApplication ─────────────────────────────────────────────────────

/// Safe wrapper around the vsomeip FFI application handle.
///
/// Manages:
/// - Application lifecycle (create → init → start → stop → destroy)
/// - Callback bridging (C++ callbacks → Rust channels)
/// - Pending request tracking for request/response patterns
/// - Background thread for the vsomeip event loop
pub struct VsomeipApplication {
    app: *mut ffi::VsomeipApp,
    context: Arc<CallbackContext>,
    incoming_rx: Mutex<Option<mpsc::UnboundedReceiver<SomeIpMessage>>>,
    event_loop_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

// SAFETY: The vsomeip_app_t* is guarded by internal mutexes on the C++ side.
// All FFI calls are thread-safe as per vsomeip3 documentation.
unsafe impl Send for VsomeipApplication {}
unsafe impl Sync for VsomeipApplication {}

impl VsomeipApplication {
    /// Create a new vsomeip application.
    pub fn new(
        app_name: &str,
        availability_tx: broadcast::Sender<ServiceAvailability>,
    ) -> Result<Self, String> {
        let c_name = CString::new(app_name).map_err(|e| format!("Invalid app name: {e}"))?;

        let app = unsafe { ffi::vsomeip_create(c_name.as_ptr()) };
        if app.is_null() {
            return Err("vsomeip_create() returned null — is libvsomeip3 installed?".into());
        }

        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
        let (event_tx, _) = broadcast::channel(256);

        let context = Arc::new(CallbackContext {
            pending: Mutex::new(HashMap::new()),
            incoming_tx,
            event_tx,
            availability_tx,
            session_counter: Mutex::new(0),
        });

        Ok(Self {
            app,
            context,
            incoming_rx: Mutex::new(Some(incoming_rx)),
            event_loop_handle: Mutex::new(None),
        })
    }

    /// Initialize the vsomeip application.
    pub fn init(&self) -> Result<(), String> {
        let rc = unsafe { ffi::vsomeip_init(self.app) };
        if rc != 0 {
            return Err("vsomeip_init() failed — check vsomeip configuration".into());
        }
        info!("vsomeip application initialized");
        Ok(())
    }

    /// Start the vsomeip event loop in a background thread.
    /// The event loop runs until `stop()` is called.
    pub fn start(&self) -> Result<(), String> {
        let app_ptr = self.app as usize; // Safe to send across threads

        let handle = std::thread::Builder::new()
            .name("vsomeip-event-loop".into())
            .spawn(move || {
                info!("vsomeip event loop starting");
                unsafe {
                    ffi::vsomeip_start(app_ptr as *mut ffi::VsomeipApp);
                }
                info!("vsomeip event loop stopped");
            })
            .map_err(|e| format!("Failed to spawn vsomeip thread: {e}"))?;

        *self.event_loop_handle.lock().unwrap() = Some(handle);
        Ok(())
    }

    /// Stop the vsomeip event loop.
    pub fn stop(&self) {
        unsafe {
            ffi::vsomeip_stop(self.app);
        }

        // Wait for the event loop thread to finish
        if let Some(handle) = self.event_loop_handle.lock().unwrap().take() {
            if let Err(e) = handle.join() {
                error!("vsomeip event loop thread panicked: {e:?}");
            }
        }
    }

    /// Take the incoming message receiver (can only be called once).
    pub fn take_incoming_rx(&self) -> Option<mpsc::UnboundedReceiver<SomeIpMessage>> {
        self.incoming_rx.lock().unwrap().take()
    }

    /// Subscribe to event notifications.
    pub fn subscribe_events(&self) -> broadcast::Receiver<SomeIpEvent> {
        self.context.event_tx.subscribe()
    }

    // ── Service offering ────────────────────────────────────────────────

    /// Offer a service (server mode).
    pub fn offer_service(
        &self,
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        minor_version: u32,
    ) {
        unsafe {
            ffi::vsomeip_offer_service(
                self.app,
                service_id,
                instance_id,
                major_version,
                minor_version,
            );
        }
        info!(
            service = %format!("0x{service_id:04X}"),
            instance = %format!("0x{instance_id:04X}"),
            "Offering SOME/IP service"
        );
    }

    /// Stop offering a service.
    pub fn stop_offer_service(&self, service_id: u16, instance_id: u16) {
        unsafe {
            ffi::vsomeip_stop_offer_service(self.app, service_id, instance_id);
        }
    }

    // ── Service consumption ─────────────────────────────────────────────

    /// Request a remote service (client mode).
    pub fn request_service(
        &self,
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        minor_version: u32,
    ) {
        unsafe {
            ffi::vsomeip_request_service(
                self.app,
                service_id,
                instance_id,
                major_version,
                minor_version,
            );
        }
        info!(
            service = %format!("0x{service_id:04X}"),
            instance = %format!("0x{instance_id:04X}"),
            "Requesting SOME/IP service"
        );
    }

    /// Release a previously requested service.
    pub fn release_service(&self, service_id: u16, instance_id: u16) {
        unsafe {
            ffi::vsomeip_release_service(self.app, service_id, instance_id);
        }
    }

    // ── Message handler registration ────────────────────────────────────

    /// Register a message handler for (service, instance, method).
    /// Use 0xFFFF for method_id to catch all methods.
    pub fn register_message_handler(&self, service_id: u16, instance_id: u16, method_id: u16) {
        let ctx_ptr = Arc::as_ptr(&self.context) as *mut c_void;
        unsafe {
            ffi::vsomeip_register_message_handler(
                self.app,
                service_id,
                instance_id,
                method_id,
                message_trampoline,
                ctx_ptr,
            );
        }
    }

    /// Unregister a message handler.
    pub fn unregister_message_handler(&self, service_id: u16, instance_id: u16, method_id: u16) {
        unsafe {
            ffi::vsomeip_unregister_message_handler(self.app, service_id, instance_id, method_id);
        }
    }

    // ── Availability handler registration ───────────────────────────────

    /// Register an availability handler for a service.
    pub fn register_availability_handler(&self, service_id: u16, instance_id: u16) {
        let ctx_ptr = Arc::as_ptr(&self.context) as *mut c_void;
        unsafe {
            ffi::vsomeip_register_availability_handler(
                self.app,
                service_id,
                instance_id,
                availability_trampoline,
                ctx_ptr,
            );
        }
    }

    /// Unregister an availability handler.
    pub fn unregister_availability_handler(&self, service_id: u16, instance_id: u16) {
        unsafe {
            ffi::vsomeip_unregister_availability_handler(self.app, service_id, instance_id);
        }
    }

    // ── Event subscription ──────────────────────────────────────────────

    /// Subscribe to an eventgroup.
    pub fn subscribe(
        &self,
        service_id: u16,
        instance_id: u16,
        eventgroup_id: u16,
        major_version: u8,
    ) {
        unsafe {
            ffi::vsomeip_subscribe(
                self.app,
                service_id,
                instance_id,
                eventgroup_id,
                major_version,
            );
        }
    }

    /// Unsubscribe from an eventgroup.
    pub fn unsubscribe(&self, service_id: u16, instance_id: u16, eventgroup_id: u16) {
        unsafe {
            ffi::vsomeip_unsubscribe(self.app, service_id, instance_id, eventgroup_id);
        }
    }

    // ── Sending ─────────────────────────────────────────────────────────

    /// Send a request and wait for the response.
    pub async fn request(
        &self,
        service_id: u16,
        instance_id: u16,
        method_id: u16,
        payload: &[u8],
        reliable: bool,
    ) -> Result<Vec<u8>, String> {
        let session = self.context.next_session();

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.context.pending.lock().unwrap();
            // client_id is assigned by vsomeip, we use 0 as placeholder
            // The response callback will match on the actual client_id+session_id
            pending.insert((0, session), PendingRequest { tx });
        }

        let rc = unsafe {
            ffi::vsomeip_send_request(
                self.app,
                service_id,
                instance_id,
                method_id,
                MT_REQUEST,
                if payload.is_empty() {
                    std::ptr::null()
                } else {
                    payload.as_ptr()
                },
                payload.len() as u32,
                if reliable { 1 } else { 0 },
            )
        };

        if rc != 0 {
            // Remove pending entry
            self.context.pending.lock().unwrap().remove(&(0, session));
            return Err("vsomeip_send_request() failed".into());
        }

        // Wait for response with timeout
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("Response channel closed".into()),
            Err(_) => {
                self.context.pending.lock().unwrap().remove(&(0, session));
                Err("Request timed out (5s)".into())
            }
        }
    }

    /// Send a fire-and-forget message.
    pub fn fire_and_forget(
        &self,
        service_id: u16,
        instance_id: u16,
        method_id: u16,
        payload: &[u8],
    ) -> Result<(), String> {
        let rc = unsafe {
            ffi::vsomeip_send_request(
                self.app,
                service_id,
                instance_id,
                method_id,
                MT_REQUEST_NO_RETURN,
                if payload.is_empty() {
                    std::ptr::null()
                } else {
                    payload.as_ptr()
                },
                payload.len() as u32,
                0, // UDP for fire-and-forget
            )
        };

        if rc != 0 {
            Err("vsomeip_send_request() failed".into())
        } else {
            Ok(())
        }
    }

    /// Send a response to an incoming request.
    pub fn send_response(
        &self,
        service_id: u16,
        instance_id: u16,
        method_id: u16,
        client_id: u16,
        session_id: u16,
        return_code: u8,
        payload: &[u8],
    ) -> Result<(), String> {
        let rc = unsafe {
            ffi::vsomeip_send_response(
                self.app,
                service_id,
                instance_id,
                method_id,
                client_id,
                session_id,
                return_code,
                if payload.is_empty() {
                    std::ptr::null()
                } else {
                    payload.as_ptr()
                },
                payload.len() as u32,
            )
        };

        if rc != 0 {
            Err("vsomeip_send_response() failed".into())
        } else {
            Ok(())
        }
    }

    /// Send an event notification.
    pub fn notify(
        &self,
        service_id: u16,
        instance_id: u16,
        event_id: u16,
        payload: &[u8],
    ) -> Result<(), String> {
        let rc = unsafe {
            ffi::vsomeip_notify(
                self.app,
                service_id,
                instance_id,
                event_id,
                if payload.is_empty() {
                    std::ptr::null()
                } else {
                    payload.as_ptr()
                },
                payload.len() as u32,
            )
        };

        if rc != 0 {
            Err("vsomeip_notify() failed".into())
        } else {
            Ok(())
        }
    }

    // ── Event registration ──────────────────────────────────────────────

    /// Register an event for an offered service.
    pub fn offer_event(
        &self,
        service_id: u16,
        instance_id: u16,
        event_id: u16,
        eventgroup_ids: &[u16],
        is_field: bool,
    ) {
        unsafe {
            ffi::vsomeip_offer_event(
                self.app,
                service_id,
                instance_id,
                event_id,
                if eventgroup_ids.is_empty() {
                    std::ptr::null()
                } else {
                    eventgroup_ids.as_ptr()
                },
                eventgroup_ids.len() as u32,
                if is_field { 1 } else { 0 },
            );
        }
    }

    /// Stop offering an event.
    pub fn stop_offer_event(&self, service_id: u16, instance_id: u16, event_id: u16) {
        unsafe {
            ffi::vsomeip_stop_offer_event(self.app, service_id, instance_id, event_id);
        }
    }
}

impl Drop for VsomeipApplication {
    fn drop(&mut self) {
        if !self.app.is_null() {
            info!("Destroying vsomeip application");
            unsafe {
                ffi::vsomeip_destroy(self.app);
            }
            self.app = std::ptr::null_mut();
        }
    }
}

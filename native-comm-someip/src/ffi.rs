// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// ffi.rs — Rust FFI declarations matching vsomeip_wrapper.h
//
// Only compiled when feature "vsomeip-ffi" is enabled.
// All functions are unsafe extern "C" — safe wrappers are in runtime.rs.
// ─────────────────────────────────────────────────────────────────────────────

use std::os::raw::{c_char, c_int, c_void};

/// Opaque handle to a vsomeip application (C++ side)
#[repr(C)]
pub struct VsomeipApp {
    _opaque: [u8; 0],
}

/// Callback for incoming SOME/IP messages.
pub type MessageHandler = unsafe extern "C" fn(
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
);

/// Callback for service availability changes.
pub type AvailabilityHandler =
    unsafe extern "C" fn(context: *mut c_void, service_id: u16, instance_id: u16, available: c_int);

extern "C" {
    // ── Lifecycle ───────────────────────────────────────────────────────
    pub fn vsomeip_create(app_name: *const c_char) -> *mut VsomeipApp;
    pub fn vsomeip_init(app: *mut VsomeipApp) -> c_int;
    pub fn vsomeip_start(app: *mut VsomeipApp);
    pub fn vsomeip_stop(app: *mut VsomeipApp);
    pub fn vsomeip_destroy(app: *mut VsomeipApp);

    // ── Service offering ────────────────────────────────────────────────
    pub fn vsomeip_offer_service(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        minor_version: u32,
    );
    pub fn vsomeip_stop_offer_service(app: *mut VsomeipApp, service_id: u16, instance_id: u16);

    // ── Service consumption ─────────────────────────────────────────────
    pub fn vsomeip_request_service(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        minor_version: u32,
    );
    pub fn vsomeip_release_service(app: *mut VsomeipApp, service_id: u16, instance_id: u16);

    // ── Message handlers ────────────────────────────────────────────────
    pub fn vsomeip_register_message_handler(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        method_id: u16,
        handler: MessageHandler,
        context: *mut c_void,
    );
    pub fn vsomeip_unregister_message_handler(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        method_id: u16,
    );

    // ── Availability handlers ───────────────────────────────────────────
    pub fn vsomeip_register_availability_handler(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        handler: AvailabilityHandler,
        context: *mut c_void,
    );
    pub fn vsomeip_unregister_availability_handler(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
    );

    // ── Event subscription ──────────────────────────────────────────────
    pub fn vsomeip_subscribe(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        eventgroup_id: u16,
        major_version: u8,
    );
    pub fn vsomeip_unsubscribe(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        eventgroup_id: u16,
    );

    // ── Sending messages ────────────────────────────────────────────────
    pub fn vsomeip_send_request(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        method_id: u16,
        message_type: u8,
        payload: *const u8,
        payload_len: u32,
        reliable: c_int,
    ) -> c_int;

    pub fn vsomeip_send_response(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        method_id: u16,
        client_id: u16,
        session_id: u16,
        return_code: u8,
        payload: *const u8,
        payload_len: u32,
    ) -> c_int;

    pub fn vsomeip_notify(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        event_id: u16,
        payload: *const u8,
        payload_len: u32,
    ) -> c_int;

    // ── Event registration ──────────────────────────────────────────────
    pub fn vsomeip_offer_event(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        event_id: u16,
        eventgroup_ids: *const u16,
        eventgroup_count: u32,
        is_field: c_int,
    );
    pub fn vsomeip_stop_offer_event(
        app: *mut VsomeipApp,
        service_id: u16,
        instance_id: u16,
        event_id: u16,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// vsomeip_wrapper.h — C API wrapping vsomeip3 C++ classes
//
// Provides opaque handles and C functions callable from Rust FFI.
// All callbacks use function-pointer + void* context for Rust compatibility.
// ─────────────────────────────────────────────────────────────────────────────

#ifndef VSOMEIP_WRAPPER_H
#define VSOMEIP_WRAPPER_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// ── Opaque handles ──────────────────────────────────────────────────────────

typedef struct vsomeip_app_t vsomeip_app_t;

// ── Callback typedefs ───────────────────────────────────────────────────────

/// Called when a message is received.
/// service_id, instance_id, method_id identify the source.
/// payload/payload_len contain the message data.
/// client_id and session_id identify the sender session.
/// message_type: 0=REQUEST, 1=REQUEST_NO_RETURN, 2=NOTIFICATION, 3=RESPONSE, 4=ERROR
/// return_code: 0=OK, ...
typedef void (*vsomeip_message_handler_t)(
    void* context,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t method_id,
    uint16_t client_id,
    uint16_t session_id,
    uint8_t  message_type,
    uint8_t  return_code,
    const uint8_t* payload,
    uint32_t payload_len
);

/// Called when service availability changes.
typedef void (*vsomeip_availability_handler_t)(
    void* context,
    uint16_t service_id,
    uint16_t instance_id,
    int      available  // 1 = available, 0 = unavailable
);

// ── Lifecycle ───────────────────────────────────────────────────────────────

/// Create a vsomeip application with the given name.
/// Returns NULL on failure.
vsomeip_app_t* vsomeip_create(const char* app_name);

/// Initialize the application. Must be called before start.
/// Returns 0 on success, -1 on failure.
int vsomeip_init(vsomeip_app_t* app);

/// Start the vsomeip event loop. This is BLOCKING and must be called from
/// a dedicated thread. Returns when vsomeip_stop() is called.
void vsomeip_start(vsomeip_app_t* app);

/// Stop the vsomeip event loop. Safe to call from any thread.
void vsomeip_stop(vsomeip_app_t* app);

/// Destroy the application and free resources.
void vsomeip_destroy(vsomeip_app_t* app);

// ── Service offering ────────────────────────────────────────────────────────

/// Offer a service (server side).
void vsomeip_offer_service(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint8_t  major_version,
    uint32_t minor_version
);

/// Stop offering a service.
void vsomeip_stop_offer_service(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id
);

// ── Service consumption ─────────────────────────────────────────────────────

/// Request a remote service (client side).
void vsomeip_request_service(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint8_t  major_version,
    uint32_t minor_version
);

/// Release a previously requested service.
void vsomeip_release_service(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id
);

// ── Message handlers ────────────────────────────────────────────────────────

/// Register a handler for incoming messages on (service, instance, method).
/// Use 0xFFFF for method to receive all methods.
void vsomeip_register_message_handler(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t method_id,
    vsomeip_message_handler_t handler,
    void* context
);

/// Unregister a previously registered message handler.
void vsomeip_unregister_message_handler(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t method_id
);

// ── Availability handlers ───────────────────────────────────────────────────

/// Register a handler for service availability changes.
void vsomeip_register_availability_handler(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    vsomeip_availability_handler_t handler,
    void* context
);

/// Unregister a previously registered availability handler.
void vsomeip_unregister_availability_handler(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id
);

// ── Event subscription ──────────────────────────────────────────────────────

/// Subscribe to an eventgroup.
void vsomeip_subscribe(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t eventgroup_id,
    uint8_t  major_version
);

/// Unsubscribe from an eventgroup.
void vsomeip_unsubscribe(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t eventgroup_id
);

// ── Sending messages ────────────────────────────────────────────────────────

/// Send a request message. Returns 0 on success, -1 on failure.
/// message_type: 0=REQUEST, 1=REQUEST_NO_RETURN
int vsomeip_send_request(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t method_id,
    uint8_t  message_type,
    const uint8_t* payload,
    uint32_t payload_len,
    int reliable  // 1 = TCP, 0 = UDP
);

/// Send a response to a received request.
/// The client_id and session_id must match the original request.
int vsomeip_send_response(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t method_id,
    uint16_t client_id,
    uint16_t session_id,
    uint8_t  return_code,
    const uint8_t* payload,
    uint32_t payload_len
);

/// Send a notification/event.
int vsomeip_notify(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t event_id,
    const uint8_t* payload,
    uint32_t payload_len
);

// ── Event registration ──────────────────────────────────────────────────────

/// Register an event for an offered service (required before notify).
void vsomeip_offer_event(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t event_id,
    const uint16_t* eventgroup_ids,
    uint32_t eventgroup_count,
    int is_field  // 1 = field (has initial value), 0 = event
);

/// Stop offering an event.
void vsomeip_stop_offer_event(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t event_id
);

#ifdef __cplusplus
}
#endif

#endif // VSOMEIP_WRAPPER_H

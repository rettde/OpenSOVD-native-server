// ─────────────────────────────────────────────────────────────────────────────
// vsomeip_wrapper.cpp — C wrapper around vsomeip3 C++ API
//
// Delegates all calls to vsomeip::runtime and vsomeip::application.
// Callback bridging: C++ std::function → C function pointer + void* context.
// ─────────────────────────────────────────────────────────────────────────────

#include "vsomeip_wrapper.h"

#include <vsomeip/vsomeip.hpp>
#include <memory>
#include <mutex>
#include <map>
#include <vector>
#include <set>
#include <cstring>

// ── Internal state ──────────────────────────────────────────────────────────

struct callback_entry {
    vsomeip_message_handler_t handler;
    void* context;
};

struct availability_entry {
    vsomeip_availability_handler_t handler;
    void* context;
};

struct vsomeip_app_t {
    std::shared_ptr<vsomeip::application> app;
    std::mutex mutex;

    // Store callbacks so they stay alive and can be dispatched
    // Key: (service_id, instance_id, method_id)
    std::map<std::tuple<uint16_t, uint16_t, uint16_t>, callback_entry> message_handlers;
    // Key: (service_id, instance_id)
    std::map<std::pair<uint16_t, uint16_t>, availability_entry> availability_handlers;
};

// ── Lifecycle ───────────────────────────────────────────────────────────────

extern "C" vsomeip_app_t* vsomeip_create(const char* app_name) {
    try {
        auto runtime = vsomeip::runtime::get();
        if (!runtime) return nullptr;

        auto app = runtime->create_application(app_name ? app_name : "");
        if (!app) return nullptr;

        auto* wrapper = new vsomeip_app_t();
        wrapper->app = app;
        return wrapper;
    } catch (...) {
        return nullptr;
    }
}

extern "C" int vsomeip_init(vsomeip_app_t* app) {
    if (!app || !app->app) return -1;
    try {
        return app->app->init() ? 0 : -1;
    } catch (...) {
        return -1;
    }
}

extern "C" void vsomeip_start(vsomeip_app_t* app) {
    if (!app || !app->app) return;
    try {
        app->app->start();
    } catch (...) {
        // start() returned (via stop or error)
    }
}

extern "C" void vsomeip_stop(vsomeip_app_t* app) {
    if (!app || !app->app) return;
    try {
        app->app->stop();
    } catch (...) {}
}

extern "C" void vsomeip_destroy(vsomeip_app_t* app) {
    if (!app) return;
    try {
        if (app->app) {
            app->app->stop();
            app->app.reset();
        }
        delete app;
    } catch (...) {
        delete app;
    }
}

// ── Service offering ────────────────────────────────────────────────────────

extern "C" void vsomeip_offer_service(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint8_t  major_version,
    uint32_t minor_version
) {
    if (!app || !app->app) return;
    app->app->offer_service(service_id, instance_id, major_version, minor_version);
}

extern "C" void vsomeip_stop_offer_service(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id
) {
    if (!app || !app->app) return;
    app->app->stop_offer_service(service_id, instance_id);
}

// ── Service consumption ─────────────────────────────────────────────────────

extern "C" void vsomeip_request_service(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint8_t  major_version,
    uint32_t minor_version
) {
    if (!app || !app->app) return;
    app->app->request_service(service_id, instance_id, major_version, minor_version);
}

extern "C" void vsomeip_release_service(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id
) {
    if (!app || !app->app) return;
    app->app->release_service(service_id, instance_id);
}

// ── Message handlers ────────────────────────────────────────────────────────

extern "C" void vsomeip_register_message_handler(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t method_id,
    vsomeip_message_handler_t handler,
    void* context
) {
    if (!app || !app->app || !handler) return;

    {
        std::lock_guard<std::mutex> lock(app->mutex);
        app->message_handlers[{service_id, instance_id, method_id}] = {handler, context};
    }

    // Capture raw pointer + callback for the C++ lambda
    vsomeip_app_t* app_ptr = app;

    app->app->register_message_handler(
        service_id, instance_id, method_id,
        [app_ptr, service_id, instance_id, method_id](
            const std::shared_ptr<vsomeip::message>& msg
        ) {
            callback_entry entry;
            {
                std::lock_guard<std::mutex> lock(app_ptr->mutex);
                auto it = app_ptr->message_handlers.find({service_id, instance_id, method_id});
                if (it == app_ptr->message_handlers.end()) return;
                entry = it->second;
            }

            const uint8_t* payload_data = nullptr;
            uint32_t payload_len = 0;
            auto pl = msg->get_payload();
            if (pl) {
                payload_data = pl->get_data();
                payload_len = pl->get_length();
            }

            entry.handler(
                entry.context,
                msg->get_service(),
                msg->get_instance(),
                msg->get_method(),
                msg->get_client(),
                msg->get_session(),
                static_cast<uint8_t>(msg->get_message_type()),
                static_cast<uint8_t>(msg->get_return_code()),
                payload_data,
                payload_len
            );
        }
    );
}

extern "C" void vsomeip_unregister_message_handler(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t method_id
) {
    if (!app || !app->app) return;
    app->app->unregister_message_handler(service_id, instance_id, method_id);

    std::lock_guard<std::mutex> lock(app->mutex);
    app->message_handlers.erase({service_id, instance_id, method_id});
}

// ── Availability handlers ───────────────────────────────────────────────────

extern "C" void vsomeip_register_availability_handler(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    vsomeip_availability_handler_t handler,
    void* context
) {
    if (!app || !app->app || !handler) return;

    {
        std::lock_guard<std::mutex> lock(app->mutex);
        app->availability_handlers[{service_id, instance_id}] = {handler, context};
    }

    vsomeip_app_t* app_ptr = app;

    app->app->register_availability_handler(
        service_id, instance_id,
        [app_ptr, service_id, instance_id](
            vsomeip::service_t _svc,
            vsomeip::instance_t _inst,
            bool available
        ) {
            availability_entry entry;
            {
                std::lock_guard<std::mutex> lock(app_ptr->mutex);
                auto it = app_ptr->availability_handlers.find({service_id, instance_id});
                if (it == app_ptr->availability_handlers.end()) return;
                entry = it->second;
            }

            entry.handler(
                entry.context,
                _svc,
                _inst,
                available ? 1 : 0
            );
        }
    );
}

extern "C" void vsomeip_unregister_availability_handler(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id
) {
    if (!app || !app->app) return;
    app->app->unregister_availability_handler(service_id, instance_id);

    std::lock_guard<std::mutex> lock(app->mutex);
    app->availability_handlers.erase({service_id, instance_id});
}

// ── Event subscription ──────────────────────────────────────────────────────

extern "C" void vsomeip_subscribe(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t eventgroup_id,
    uint8_t  major_version
) {
    if (!app || !app->app) return;
    app->app->subscribe(service_id, instance_id, eventgroup_id, major_version);
}

extern "C" void vsomeip_unsubscribe(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t eventgroup_id
) {
    if (!app || !app->app) return;
    app->app->unsubscribe(service_id, instance_id, eventgroup_id);
}

// ── Sending messages ────────────────────────────────────────────────────────

extern "C" int vsomeip_send_request(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t method_id,
    uint8_t  message_type,
    const uint8_t* payload,
    uint32_t payload_len,
    int reliable
) {
    if (!app || !app->app) return -1;

    try {
        auto runtime = vsomeip::runtime::get();
        auto msg = runtime->create_request(reliable != 0);

        msg->set_service(service_id);
        msg->set_instance(instance_id);
        msg->set_method(method_id);
        msg->set_message_type(static_cast<vsomeip::message_type_e>(message_type));

        if (payload && payload_len > 0) {
            auto pl = runtime->create_payload();
            pl->set_data(payload, payload_len);
            msg->set_payload(pl);
        }

        app->app->send(msg);
        return 0;
    } catch (...) {
        return -1;
    }
}

extern "C" int vsomeip_send_response(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t method_id,
    uint16_t client_id,
    uint16_t session_id,
    uint8_t  return_code,
    const uint8_t* payload,
    uint32_t payload_len
) {
    if (!app || !app->app) return -1;

    try {
        auto runtime = vsomeip::runtime::get();
        auto msg = runtime->create_response(nullptr);

        // Manually set all fields for the response
        msg->set_service(service_id);
        msg->set_instance(instance_id);
        msg->set_method(method_id);
        msg->set_client(client_id);
        msg->set_session(session_id);
        msg->set_message_type(vsomeip::message_type_e::MT_RESPONSE);
        msg->set_return_code(static_cast<vsomeip::return_code_e>(return_code));

        if (payload && payload_len > 0) {
            auto pl = runtime->create_payload();
            pl->set_data(payload, payload_len);
            msg->set_payload(pl);
        }

        app->app->send(msg);
        return 0;
    } catch (...) {
        return -1;
    }
}

extern "C" int vsomeip_notify(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t event_id,
    const uint8_t* payload,
    uint32_t payload_len
) {
    if (!app || !app->app) return -1;

    try {
        auto runtime = vsomeip::runtime::get();
        auto pl = runtime->create_payload();

        if (payload && payload_len > 0) {
            pl->set_data(payload, payload_len);
        }

        app->app->notify(service_id, instance_id, event_id, pl);
        return 0;
    } catch (...) {
        return -1;
    }
}

// ── Event registration ──────────────────────────────────────────────────────

extern "C" void vsomeip_offer_event(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t event_id,
    const uint16_t* eventgroup_ids,
    uint32_t eventgroup_count,
    int is_field
) {
    if (!app || !app->app) return;

    std::set<vsomeip::eventgroup_t> groups;
    for (uint32_t i = 0; i < eventgroup_count; i++) {
        groups.insert(eventgroup_ids[i]);
    }

    app->app->offer_event(
        service_id,
        instance_id,
        event_id,
        groups,
        vsomeip::event_type_e::ET_FIELD
    );
}

extern "C" void vsomeip_stop_offer_event(
    vsomeip_app_t* app,
    uint16_t service_id,
    uint16_t instance_id,
    uint16_t event_id
) {
    if (!app || !app->app) return;
    app->app->stop_offer_event(service_id, instance_id, event_id);
}

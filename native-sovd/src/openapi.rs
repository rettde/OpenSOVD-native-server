// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// OpenAPI 3.1 spec — JSON build (ISO 17978-3 / SOVD)
// ─────────────────────────────────────────────────────────────────────────────

/// Build the full OpenAPI 3.1 spec as JSON for the SOVD API.
pub fn build_openapi_json() -> serde_json::Value {
    serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "OpenSOVD-native-server API",
            "version": "1.1.0",
            "description": "ISO 17978-3 REST API — Service-Oriented Vehicle Diagnostics.",
            "license": { "name": "Apache-2.0", "url": "https://www.apache.org/licenses/LICENSE-2.0" }
        },
        "servers": [{ "url": "/sovd/v1", "description": "SOVD API v1" }],
        "tags": [
            { "name": "Discovery", "description": "Server info and metadata" },
            { "name": "Health", "description": "Health monitoring" },
            { "name": "Components", "description": "Component management (§7.1)" },
            { "name": "Data", "description": "Data read/write (§7.5)" },
            { "name": "Faults", "description": "Fault management (§7.6)" },
            { "name": "Operations", "description": "Operation execution (§7.7)" },
            { "name": "Groups", "description": "Component groups (§7.2)" },
            { "name": "Capabilities", "description": "Component capabilities (§7.3)" },
            { "name": "Locking", "description": "Resource locking (§7.4)" },
            { "name": "Mode", "description": "Mode/session management (§7.6)" },
            { "name": "Configuration", "description": "Configuration (§7.8)" },
            { "name": "Proximity", "description": "Proximity challenge (§7.9)" },
            { "name": "Logs", "description": "Diagnostic logs (§7.10)" },
            { "name": "UDS", "description": "Vendor extensions (x-uds)" },
        ],
        "paths": build_paths(),
        "components": {
            "schemas": {
                "ODataError": {
                    "type": "object",
                    "required": ["error"],
                    "properties": {
                        "error": {
                            "type": "object",
                            "required": ["code", "message"],
                            "properties": {
                                "code": { "type": "string", "example": "SOVD-ERR-404" },
                                "message": { "type": "string" },
                                "target": { "type": "string" },
                                "details": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "code": { "type": "string" },
                                            "message": { "type": "string" },
                                            "target": { "type": "string" }
                                        }
                                    }
                                },
                                "innererror": { "type": "string" }
                            }
                        }
                    }
                }
            },
            "securitySchemes": {
                "ApiKeyAuth": { "type": "apiKey", "in": "header", "name": "X-API-Key" },
                "BearerAuth": { "type": "http", "scheme": "bearer", "bearerFormat": "JWT" }
            }
        },
        "security": [{ "ApiKeyAuth": [] }, { "BearerAuth": [] }]
    })
}

fn op(tag: &str, summary: &str, op_id: &str) -> serde_json::Value {
    serde_json::json!({
        "tags": [tag],
        "summary": summary,
        "operationId": op_id,
        "responses": {
            "200": { "description": "Success", "content": { "application/json": { "schema": { "type": "object" } } } },
            "404": { "description": "Not found" }
        }
    })
}

fn post(tag: &str, summary: &str, op_id: &str) -> serde_json::Value {
    serde_json::json!({
        "tags": [tag],
        "summary": summary,
        "operationId": op_id,
        "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
        "responses": {
            "200": { "description": "Success" },
            "400": { "description": "Bad request" }
        }
    })
}

fn put(tag: &str, summary: &str, op_id: &str) -> serde_json::Value {
    serde_json::json!({
        "tags": [tag],
        "summary": summary,
        "operationId": op_id,
        "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
        "responses": {
            "204": { "description": "Updated" },
            "400": { "description": "Bad request" },
            "409": { "description": "Conflict (locked)" }
        }
    })
}

fn del(tag: &str, summary: &str, op_id: &str) -> serde_json::Value {
    serde_json::json!({
        "tags": [tag],
        "summary": summary,
        "operationId": op_id,
        "responses": { "204": { "description": "Deleted" }, "404": { "description": "Not found" } }
    })
}

fn build_paths() -> serde_json::Value {
    serde_json::json!({
        "/": { "get": op("Discovery", "SOVD server info (§5.1)", "server_info") },
        "/$metadata": { "get": op("Discovery", "OData entity data model (§5.2)", "odata_metadata") },
        "/health": { "get": op("Health", "Health check", "health_check") },

        "/components": { "get": op("Components", "List all components (§7.1)", "list_components") },
        "/components/{component_id}": { "get": op("Components", "Get component by ID (§7.1)", "get_component") },

        "/components/{component_id}/data": { "get": op("Data", "List data catalog (§7.5)", "list_data") },
        "/components/{component_id}/data/{data_id}": {
            "get": op("Data", "Read data value — returns ETag (§7.5)", "read_data"),
            "put": put("Data", "Write data value (§7.5)", "write_data"),
            "patch": put("Data", "Partial data update — merge fields (§7.5)", "patch_data")
        },
        "/components/{component_id}/data/bulk-read": { "post": post("Data", "Bulk read data (§7.5)", "bulk_read") },
        "/components/{component_id}/data/bulk-write": { "post": post("Data", "Bulk write data (§7.5)", "bulk_write") },

        "/components/{component_id}/faults": {
            "get": op("Faults", "List faults (§7.6)", "list_faults"),
            "delete": del("Faults", "Clear all faults (§7.6)", "clear_faults")
        },
        "/components/{component_id}/faults/{fault_id}": {
            "get": op("Faults", "Get fault by ID (§7.6)", "get_fault_by_id"),
            "delete": del("Faults", "Clear single fault (§7.6)", "clear_single_fault")
        },
        "/components/{component_id}/faults/subscribe": {
            "get": { "tags": ["Faults"], "summary": "Subscribe to fault changes via SSE (§7.11)", "operationId": "subscribe_faults",
                     "responses": { "200": { "description": "SSE event stream" } } }
        },

        "/components/{component_id}/operations": { "get": op("Operations", "List operations (§7.7)", "list_operations") },
        "/components/{component_id}/operations/{op_id}": {
            "post": { "tags": ["Operations"], "summary": "Execute operation — returns 202 Accepted (§7.7)", "operationId": "execute_operation",
                      "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                      "responses": { "202": { "description": "Accepted — poll execution resource" } } }
        },
        "/components/{component_id}/operations/{op_id}/executions": { "get": op("Operations", "List executions (§7.7)", "list_executions") },
        "/components/{component_id}/operations/{op_id}/executions/{exec_id}": {
            "get": op("Operations", "Get execution status (§7.7)", "get_execution"),
            "delete": del("Operations", "Cancel execution (§7.7)", "cancel_execution")
        },

        "/groups": { "get": op("Groups", "List groups (§7.2)", "list_groups") },
        "/groups/{group_id}": { "get": op("Groups", "Get group (§7.2)", "get_group") },
        "/groups/{group_id}/components": { "get": op("Groups", "Get group components (§7.2)", "get_group_components") },

        "/components/{component_id}/capabilities": { "get": op("Capabilities", "Get capabilities (§7.3)", "get_capabilities") },

        "/components/{component_id}/lock": {
            "post": post("Locking", "Acquire lock (§7.4)", "acquire_lock"),
            "get": op("Locking", "Get lock status (§7.4)", "get_lock"),
            "delete": del("Locking", "Release lock (§7.4)", "release_lock")
        },

        "/components/{component_id}/mode": {
            "get": op("Mode", "Get component mode (§7.6)", "get_mode"),
            "post": post("Mode", "Set component mode (§7.6)", "set_mode")
        },

        "/components/{component_id}/config": {
            "get": op("Configuration", "Read configuration (§7.8)", "read_config"),
            "put": put("Configuration", "Write configuration (§7.8)", "write_config")
        },

        "/components/{component_id}/proximityChallenge": { "post": post("Proximity", "Create proximity challenge (§7.9)", "proximity_challenge") },
        "/components/{component_id}/proximityChallenge/{challenge_id}": { "get": op("Proximity", "Get challenge status (§7.9)", "get_proximity_challenge") },

        "/components/{component_id}/logs": { "get": op("Logs", "Get diagnostic logs (§7.10)", "get_logs") },

        // Vendor extensions (x-uds)
        "/x-uds/components/{component_id}/connect": { "post": post("UDS", "Connect (UDS)", "uds_connect") },
        "/x-uds/components/{component_id}/disconnect": { "post": post("UDS", "Disconnect (UDS)", "uds_disconnect") },
        "/x-uds/components/{component_id}/io/{data_id}": { "post": post("UDS", "IO control (UDS)", "uds_io_control") },
        "/x-uds/components/{component_id}/comm-control": { "post": post("UDS", "Comm control (UDS)", "uds_comm_control") },
        "/x-uds/components/{component_id}/dtc-setting": { "post": post("UDS", "DTC setting (UDS)", "uds_dtc_setting") },
        "/x-uds/components/{component_id}/flash": { "post": post("UDS", "Start flash (UDS)", "uds_flash") },
        "/x-uds/diag/keepalive": { "get": op("UDS", "Keepalive status (UDS)", "uds_keepalive") },
        "/x-uds/components/{component_id}/memory": {
            "get": op("UDS", "Read memory (UDS)", "uds_read_memory"),
            "put": put("UDS", "Write memory (UDS)", "uds_write_memory")
        }
    })
}

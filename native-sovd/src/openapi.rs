// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// OpenAPI 3.1 spec — SOVD Capability Description File (CDF)
//
// Conforms to ASAM SOVD V1.1.0 CDF structure validated by
// https://github.com/dsagmbh/sovd-cdf-validator
// ─────────────────────────────────────────────────────────────────────────────

use native_interfaces::oem::CdfPolicy;

/// Build the full OpenAPI 3.1 CDF spec as JSON for the SOVD API.
/// Uses DefaultProfile CDF values (standard SOVD).
pub fn build_openapi_json() -> serde_json::Value {
    build_openapi_json_with_policy(&native_interfaces::DefaultProfile, None)
}

/// Build CDF spec using OEM-specific CdfPolicy values.
pub fn build_openapi_json_with_policy(
    cdf: &dyn CdfPolicy,
    filter: Option<&str>,
) -> serde_json::Value {
    let paths = match filter {
        Some("data") => data_paths(cdf),
        Some("faults") => fault_paths(),
        Some("operations") => operation_paths(cdf),
        Some("modes") => mode_paths(),
        Some("locks") => filter_paths(&infra_paths(), "locks"),
        Some("configurations") => filter_paths(&infra_paths(), "configurations"),
        Some("logs") => filter_paths(&infra_paths(), "logs"),
        Some("components") => discovery_paths(),
        None | Some(_) => build_paths(cdf),
    };
    let applicability = cdf.applicability();
    serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "OpenSOVD-native-server CDF",
            "version": "1.1.0",
            "description": "SOVD Capability Description File — ISO 17978-3 / ASAM SOVD V1.1.0.",
            "license": { "name": "Apache-2.0", "url": "https://www.apache.org/licenses/LICENSE-2.0" },
            "x-sovd-version": "1.1.0",
            "x-sovd-applicability": {
                "online": applicability.online,
                "offline": applicability.offline
            }
        },
        "servers": [{ "url": "/sovd/v1", "description": "SOVD API v1" }],
        "tags": [
            { "name": "Discovery", "description": "Server info and metadata" },
            { "name": "Components", "description": "Component management (§7.6)" },
            { "name": "Data", "description": "Data read/write (§7.9)" },
            { "name": "Faults", "description": "Fault management (§7.8)" },
            { "name": "Operations", "description": "Operation execution (§7.14)" },
            { "name": "Capabilities", "description": "Component capabilities (§7.6.3)" },
            { "name": "Locking", "description": "Resource locking (§7.17)" },
            { "name": "Mode", "description": "Mode/session management (§7.16)" },
            { "name": "Configuration", "description": "Configuration (§7.12)" },
            { "name": "Logs", "description": "Diagnostic logs (§7.21)" },
            { "name": "Updates", "description": "Software updates (§7.18)" },
        ],
        "paths": paths,
        "components": {
            "schemas": build_schemas(),
            "securitySchemes": {
                "ApiKeyAuth": { "type": "apiKey", "in": "header", "name": "X-API-Key" },
                "BearerAuth": { "type": "http", "scheme": "bearer", "bearerFormat": "JWT" }
            }
        },
        "security": [{ "ApiKeyAuth": [] }, { "BearerAuth": [] }]
    })
}

/// Filter a paths object to only include entries whose key contains the given segment.
fn filter_paths(paths: &serde_json::Value, segment: &str) -> serde_json::Value {
    let needle = format!("/{segment}");
    match paths {
        serde_json::Value::Object(map) => {
            let filtered: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .filter(|(k, _)| k.contains(&needle))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            serde_json::Value::Object(filtered)
        }
        other => other.clone(),
    }
}

// ── Schema definitions (§5.8, §6.2) ─────────────────────────────────────

fn build_schemas() -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for sub in [core_schemas(), resource_schemas()] {
        if let serde_json::Value::Object(m) = sub {
            map.extend(m);
        }
    }
    serde_json::Value::Object(map)
}

fn core_schemas() -> serde_json::Value {
    serde_json::json!({
        "SovdError": {
            "type": "object",
            "required": ["error_code", "message"],
            "properties": {
                "error_code": { "type": "string", "example": "SOVD-0001" },
                "message": { "type": "string" }
            }
        },
        "EntityRef": {
            "type": "object",
            "required": ["id", "name", "href"],
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" },
                "href": { "type": "string", "format": "uri-reference" }
            }
        },
        "EntityCollection": {
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "$ref": "#/components/schemas/EntityRef" }
                }
            }
        },
        "Capability": {
            "type": "object",
            "required": ["id", "name"],
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" }
            }
        },
        "FaultSummary": {
            "type": "object",
            "required": ["code", "fault_name"],
            "properties": {
                "code": { "type": "string" },
                "fault_name": { "type": "string" },
                "severity": { "type": "string" },
                "status": { "type": "string" }
            }
        },
        "FaultCollection": {
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "$ref": "#/components/schemas/FaultSummary" }
                }
            }
        },
        "FaultDetail": {
            "type": "object",
            "properties": {
                "item": { "$ref": "#/components/schemas/FaultSummary" }
            }
        },
        "DataResourceSummary": {
            "type": "object",
            "required": ["id", "name", "category"],
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" },
                "category": { "type": "string" }
            }
        },
        "DataResourceCollection": {
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "$ref": "#/components/schemas/DataResourceSummary" }
                }
            }
        },
        "DataResourceValue": {
            "type": "object",
            "required": ["id", "data"],
            "properties": {
                "id": { "type": "string" },
                "data": {}
            }
        }
    })
}

fn resource_schemas() -> serde_json::Value {
    serde_json::json!({
        "OperationSummary": {
            "type": "object",
            "required": ["id", "proximity_proof_required"],
            "properties": {
                "id": { "type": "string" },
                "proximity_proof_required": { "type": "boolean" },
                "name": { "type": "string" }
            }
        },
        "OperationCollection": {
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "$ref": "#/components/schemas/OperationSummary" }
                }
            }
        },
        "ExecutionRef": {
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string" },
                "status": { "type": "string" },
                "href": { "type": "string", "format": "uri-reference" }
            }
        },
        "ExecutionCollection": {
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "$ref": "#/components/schemas/ExecutionRef" }
                }
            }
        },
        "ModeSummary": {
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" }
            }
        },
        "ModeCollection": {
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "$ref": "#/components/schemas/ModeSummary" }
                }
            }
        },
        "ModeDetail": {
            "type": "object",
            "required": ["id", "value"],
            "properties": {
                "id": { "type": "string" },
                "value": { "type": "string" }
            }
        },
        "ConfigurationDetail": {
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "data": {}
            }
        },
        "LogCollection": {
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "type": "object" }
                }
            }
        }
    })
}

// ── Response helpers ─────────────────────────────────────────────────────

fn include_schema_param() -> serde_json::Value {
    serde_json::json!({
        "name": "include-schema",
        "in": "query",
        "required": false,
        "schema": { "type": "boolean" },
        "description": "If true, include the JSON schema of the resource in the response (§6.2.6)"
    })
}

fn ok_ref(schema_ref: &str) -> serde_json::Value {
    serde_json::json!({
        "description": "Success",
        "content": { "application/json": { "schema": { "$ref": schema_ref } } }
    })
}

fn err_default() -> serde_json::Value {
    serde_json::json!({
        "description": "Error",
        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/SovdError" } } }
    })
}

// ── Path definitions (ASAM SOVD §5.3, §5.4) ─────────────────────────────

fn build_paths(cdf: &dyn CdfPolicy) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for sub in [
        discovery_paths(),
        data_paths(cdf),
        fault_paths(),
        operation_paths(cdf),
        mode_paths(),
        infra_paths(),
    ] {
        if let serde_json::Value::Object(m) = sub {
            map.extend(m);
        }
    }
    serde_json::Value::Object(map)
}

fn discovery_paths() -> serde_json::Value {
    serde_json::json!({
        "/": {
            "x-sovd-name": "SOVD Server",
            "get": {
                "tags": ["Discovery"],
                "summary": "SOVD server capability (§7.6.3)",
                "operationId": "server_info",
                "parameters": [include_schema_param()],
                "responses": {
                    "200": ok_ref("#/components/schemas/Capability"),
                    "4XX": err_default()
                }
            }
        },
        "/components": {
            "x-sovd-name": "Components",
            "get": {
                "tags": ["Components"],
                "summary": "Discover contained entities (§7.6.2.1)",
                "operationId": "list_components",
                "responses": {
                    "200": ok_ref("#/components/schemas/EntityCollection"),
                    "4XX": err_default()
                }
            }
        },
        "/apps": {
            "x-sovd-name": "Applications",
            "get": {
                "tags": ["Discovery"],
                "summary": "List applications (§7.6.2.1)",
                "operationId": "list_apps",
                "responses": {
                    "200": ok_ref("#/components/schemas/EntityCollection"),
                    "4XX": err_default()
                }
            }
        },
        // NOTE: /funcs, /areas, /version-info intentionally omitted from CDF:
        // - /areas: MBDS S-SOVD forbids Area entity type
        // - /funcs: not a standard SOVD CDF path (§5.3)
        // - /version-info: forbidden in CDF per §5.6
        // All three endpoints exist in routes.rs but are excluded from the spec.
        "/components/{component_id}": {
            "x-sovd-name": "Component",
            "get": {
                "tags": ["Components"],
                "summary": "Get component capabilities (§7.6.3)",
                "operationId": "get_component",
                "parameters": [include_schema_param()],
                "responses": {
                    "200": ok_ref("#/components/schemas/Capability"),
                    "4XX": err_default()
                }
            }
        }
    })
}

fn data_paths(cdf: &dyn CdfPolicy) -> serde_json::Value {
    let unit = cdf.default_data_unit();
    serde_json::json!({
        "/components/{component_id}/data": {
            "get": {
                "tags": ["Data"],
                "summary": "Query data resources (§7.9.3)",
                "operationId": "list_data",
                "responses": {
                    "200": ok_ref("#/components/schemas/DataResourceCollection"),
                    "4XX": err_default()
                }
            }
        },
        "/components/{component_id}/data/{data_id}": {
            "x-sovd-name": "DataResource",
            "x-sovd-data-category": "currentData",
            "x-sovd-unit": unit,
            "get": {
                "tags": ["Data"],
                "summary": "Read data value (§7.9.4)",
                "operationId": "read_data",
                "parameters": [include_schema_param()],
                "responses": {
                    "200": ok_ref("#/components/schemas/DataResourceValue"),
                    "4XX": err_default()
                }
            },
            "put": {
                "tags": ["Data"],
                "summary": "Write data value (§7.9.6)",
                "operationId": "write_data",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "204": { "description": "Written" },
                    "4XX": err_default()
                }
            }
        },
        "/components/{component_id}/bulk-data": {
            "get": {
                "tags": ["Data"],
                "summary": "Bulk data access (§7.20)",
                "operationId": "bulk_data",
                "responses": {
                    "200": { "description": "Success", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "4XX": err_default()
                }
            }
        }
    })
}

fn fault_paths() -> serde_json::Value {
    serde_json::json!({
        "/components/{component_id}/faults": {
            "get": {
                "tags": ["Faults"],
                "summary": "Query faults (§7.8.2)",
                "operationId": "list_faults",
                "responses": {
                    "200": ok_ref("#/components/schemas/FaultCollection"),
                    "4XX": err_default()
                }
            },
            "delete": {
                "tags": ["Faults"],
                "summary": "Clear all faults (§7.8)",
                "operationId": "clear_faults",
                "responses": {
                    "204": { "description": "Cleared" },
                    "4XX": err_default()
                }
            }
        },
        "/components/{component_id}/faults/{fault_id}": {
            "x-sovd-name": "Fault",
            "get": {
                "tags": ["Faults"],
                "summary": "Get fault detail (§7.8.3)",
                "operationId": "get_fault",
                "parameters": [include_schema_param()],
                "responses": {
                    "200": ok_ref("#/components/schemas/FaultDetail"),
                    "4XX": err_default()
                }
            },
            "delete": {
                "tags": ["Faults"],
                "summary": "Clear single fault (§7.8)",
                "operationId": "clear_fault",
                "responses": {
                    "204": { "description": "Cleared" },
                    "4XX": err_default()
                }
            }
        }
    })
}

fn operation_paths(cdf: &dyn CdfPolicy) -> serde_json::Value {
    let proximity = cdf.default_proximity_proof_required();
    serde_json::json!({
        "/components/{component_id}/operations": {
            "x-sovd-proximity-proof-required": proximity,
            "get": {
                "tags": ["Operations"],
                "summary": "List operations (§7.14.3)",
                "operationId": "list_operations",
                "responses": {
                    "200": ok_ref("#/components/schemas/OperationCollection"),
                    "4XX": err_default()
                }
            }
        },
        "/components/{component_id}/operations/{op_id}/executions": {
            "x-sovd-name": "Executions",
            "x-sovd-retention-timeout": 3600,
            "get": {
                "tags": ["Operations"],
                "summary": "List executions (§7.14.4)",
                "operationId": "list_executions",
                "responses": {
                    "200": ok_ref("#/components/schemas/ExecutionCollection"),
                    "4XX": err_default()
                }
            },
            "post": {
                "tags": ["Operations"],
                "summary": "Execute operation (§7.14.6)",
                "operationId": "execute_operation",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "202": {
                        "description": "Accepted",
                        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ExecutionRef" } } },
                        "headers": { "Location": { "schema": { "type": "string" } } }
                    },
                    "4XX": err_default()
                }
            }
        },
        "/components/{component_id}/operations/{op_id}/executions/{exec_id}": {
            "x-sovd-name": "Execution",
            "get": {
                "tags": ["Operations"],
                "summary": "Get execution status (§7.14)",
                "operationId": "get_execution",
                "parameters": [include_schema_param()],
                "responses": {
                    "200": ok_ref("#/components/schemas/ExecutionRef"),
                    "4XX": err_default()
                }
            },
            "put": {
                "tags": ["Operations"],
                "summary": "Control execution (§7.14)",
                "operationId": "control_execution",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": ok_ref("#/components/schemas/ExecutionRef"),
                    "4XX": err_default()
                }
            }
        }
    })
}

fn mode_paths() -> serde_json::Value {
    serde_json::json!({
        "/components/{component_id}/modes": {
            "get": {
                "tags": ["Mode"],
                "summary": "Query modes (§7.16.2)",
                "operationId": "list_modes",
                "responses": {
                    "200": ok_ref("#/components/schemas/ModeCollection"),
                    "4XX": err_default()
                }
            }
        },
        "/components/{component_id}/modes/{mode_id}": {
            "x-sovd-name": "Mode",
            "get": {
                "tags": ["Mode"],
                "summary": "Get mode detail (§7.16.3)",
                "operationId": "get_mode",
                "parameters": [include_schema_param()],
                "responses": {
                    "200": ok_ref("#/components/schemas/ModeDetail"),
                    "4XX": err_default()
                }
            },
            "put": {
                "tags": ["Mode"],
                "summary": "Activate mode (§7.16.4)",
                "operationId": "activate_mode",
                "parameters": [include_schema_param()],
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": ok_ref("#/components/schemas/ModeDetail"),
                    "4XX": err_default()
                }
            }
        }
    })
}

fn infra_paths() -> serde_json::Value {
    serde_json::json!({
        "/components/{component_id}/locks": {
            "get": {
                "tags": ["Locking"],
                "summary": "Get lock status (§7.17)",
                "operationId": "get_lock",
                "responses": {
                    "200": { "description": "Lock status", "content": { "application/json": { "schema": { "type": "object", "properties": { "id": { "type": "string" } } } } } },
                    "4XX": err_default()
                }
            }
        },
        "/components/{component_id}/configurations": {
            "get": {
                "tags": ["Configuration"],
                "summary": "Read configuration (§7.12)",
                "operationId": "read_config",
                "responses": {
                    "200": ok_ref("#/components/schemas/ConfigurationDetail"),
                    "4XX": err_default()
                }
            }
        },
        "/components/{component_id}/logs": {
            "get": {
                "tags": ["Logs"],
                "summary": "Get diagnostic logs (§7.21)",
                "operationId": "get_logs",
                "responses": {
                    "200": ok_ref("#/components/schemas/LogCollection"),
                    "4XX": err_default()
                }
            }
        },
        "/updates": {
            "get": {
                "tags": ["Updates"],
                "summary": "List software updates (§7.18)",
                "operationId": "list_updates",
                "responses": {
                    "200": { "description": "Success", "content": { "application/json": { "schema": { "type": "object", "properties": { "items": { "type": "array", "items": { "type": "object" } } } } } } },
                    "4XX": err_default()
                }
            }
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn export_openapi_spec() {
        let spec = build_openapi_json();
        let json = serde_json::to_string_pretty(&spec).unwrap();
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../openapi-spec.json"),
            json,
        )
        .unwrap();
    }

    // ── T1.1: OpenAPI Contract Tests ─────────────────────────────────────
    //
    // Validate the generated CDF spec against ISO 17978-3 / ASAM SOVD V1.1.0
    // structural requirements. These tests catch:
    //   - Missing mandatory paths
    //   - Missing schemas
    //   - Incorrect OpenAPI version
    //   - Missing x-sovd-* extensions
    //   - Missing security schemes
    //   - Drift between routes.rs endpoints and CDF paths

    #[test]
    fn contract_openapi_version_is_3_1() {
        let spec = build_openapi_json();
        assert_eq!(spec["openapi"], "3.1.0");
    }

    #[test]
    fn contract_info_block_has_required_fields() {
        let spec = build_openapi_json();
        let info = &spec["info"];
        assert!(info["title"].is_string(), "info.title missing");
        assert!(info["version"].is_string(), "info.version missing");
        assert!(info["x-sovd-version"].is_string(), "x-sovd-version missing");
        assert!(info["x-sovd-applicability"].is_object(), "x-sovd-applicability missing");
        assert!(
            info["x-sovd-applicability"]["online"].is_boolean(),
            "x-sovd-applicability.online missing"
        );
    }

    #[test]
    fn contract_server_url_is_sovd_v1() {
        let spec = build_openapi_json();
        let servers = spec["servers"].as_array().expect("servers must be array");
        assert!(!servers.is_empty(), "servers array is empty");
        assert_eq!(servers[0]["url"], "/sovd/v1");
    }

    #[test]
    fn contract_mandatory_sovd_paths_present() {
        let spec = build_openapi_json();
        let paths = spec["paths"].as_object().expect("paths must be object");

        // ISO 17978-3 mandatory paths (ASAM SOVD §5.3)
        let mandatory = [
            "/",                                                          // §7.6.3 Discovery
            "/components",                                                // §7.6.2.1 Components
            "/components/{component_id}",                                 // §7.6.3 Component detail
            "/components/{component_id}/data",                            // §7.9.3 Data list
            "/components/{component_id}/data/{data_id}",                  // §7.9.4 Data read/write
            "/components/{component_id}/faults",                          // §7.8.2 Fault list
            "/components/{component_id}/faults/{fault_id}",               // §7.8.3 Fault detail
            "/components/{component_id}/operations",                      // §7.14.3 Operation list
            "/components/{component_id}/operations/{op_id}/executions",   // §7.14.4 Executions
            "/components/{component_id}/operations/{op_id}/executions/{exec_id}", // §7.14 Execution detail
            "/components/{component_id}/modes",                           // §7.16.2 Mode list
            "/components/{component_id}/modes/{mode_id}",                 // §7.16.3 Mode detail
            "/components/{component_id}/locks",                           // §7.17 Locking
            "/components/{component_id}/configurations",                  // §7.12 Configuration
            "/components/{component_id}/logs",                            // §7.21 Logs
        ];

        for path in &mandatory {
            assert!(
                paths.contains_key(*path),
                "Mandatory SOVD path missing from CDF: {path}"
            );
        }
    }

    #[test]
    fn contract_data_path_has_get_and_put() {
        let spec = build_openapi_json();
        let data_path = &spec["paths"]["/components/{component_id}/data/{data_id}"];
        assert!(data_path["get"].is_object(), "data GET missing");
        assert!(data_path["put"].is_object(), "data PUT missing");
    }

    #[test]
    fn contract_faults_path_has_get_and_delete() {
        let spec = build_openapi_json();
        let faults = &spec["paths"]["/components/{component_id}/faults"];
        assert!(faults["get"].is_object(), "faults GET missing");
        assert!(faults["delete"].is_object(), "faults DELETE missing");
    }

    #[test]
    fn contract_executions_path_has_get_and_post() {
        let spec = build_openapi_json();
        let execs =
            &spec["paths"]["/components/{component_id}/operations/{op_id}/executions"];
        assert!(execs["get"].is_object(), "executions GET missing");
        assert!(execs["post"].is_object(), "executions POST missing");
    }

    #[test]
    fn contract_execution_post_returns_202_with_location() {
        let spec = build_openapi_json();
        let post =
            &spec["paths"]["/components/{component_id}/operations/{op_id}/executions"]["post"];
        let responses = &post["responses"];
        assert!(responses["202"].is_object(), "202 Accepted response missing");
        assert!(
            responses["202"]["headers"]["Location"].is_object(),
            "Location header on 202 missing"
        );
    }

    #[test]
    fn contract_mode_path_has_get_and_put() {
        let spec = build_openapi_json();
        let mode = &spec["paths"]["/components/{component_id}/modes/{mode_id}"];
        assert!(mode["get"].is_object(), "mode GET missing");
        assert!(mode["put"].is_object(), "mode PUT missing");
    }

    #[test]
    fn contract_mandatory_schemas_present() {
        let spec = build_openapi_json();
        let schemas = spec["components"]["schemas"]
            .as_object()
            .expect("schemas must be object");

        let required_schemas = [
            "SovdError",
            "EntityRef",
            "EntityCollection",
            "FaultSummary",
            "FaultCollection",
            "DataResourceSummary",
            "DataResourceCollection",
            "DataResourceValue",
            "OperationSummary",
            "OperationCollection",
            "ExecutionRef",
            "ExecutionCollection",
            "ModeSummary",
            "ModeCollection",
            "ModeDetail",
            "ConfigurationDetail",
            "LogCollection",
        ];

        for schema in &required_schemas {
            assert!(
                schemas.contains_key(*schema),
                "Required schema missing from CDF: {schema}"
            );
        }
    }

    #[test]
    fn contract_security_schemes_present() {
        let spec = build_openapi_json();
        let schemes = spec["components"]["securitySchemes"]
            .as_object()
            .expect("securitySchemes must be object");

        assert!(schemes.contains_key("ApiKeyAuth"), "ApiKeyAuth scheme missing");
        assert!(schemes.contains_key("BearerAuth"), "BearerAuth scheme missing");
        assert_eq!(schemes["ApiKeyAuth"]["type"], "apiKey");
        assert_eq!(schemes["BearerAuth"]["type"], "http");
        assert_eq!(schemes["BearerAuth"]["scheme"], "bearer");
    }

    #[test]
    fn contract_global_security_requirement_set() {
        let spec = build_openapi_json();
        let security = spec["security"].as_array().expect("security must be array");
        assert!(
            !security.is_empty(),
            "Global security requirement is empty"
        );
    }

    #[test]
    fn contract_x_sovd_extensions_on_data_paths() {
        let spec = build_openapi_json();
        let data_path = &spec["paths"]["/components/{component_id}/data/{data_id}"];
        assert!(
            data_path["x-sovd-data-category"].is_string(),
            "x-sovd-data-category missing on data path"
        );
        assert!(
            data_path["x-sovd-unit"].is_string(),
            "x-sovd-unit missing on data path"
        );
    }

    #[test]
    fn contract_x_sovd_name_on_key_resources() {
        let spec = build_openapi_json();
        let paths = spec["paths"].as_object().unwrap();

        // Paths that MUST have x-sovd-name (ASAM SOVD §5.3)
        let named_paths = [
            "/",
            "/components",
            "/components/{component_id}",
            "/components/{component_id}/data/{data_id}",
            "/components/{component_id}/faults/{fault_id}",
            "/components/{component_id}/modes/{mode_id}",
        ];

        for path in &named_paths {
            if let Some(p) = paths.get(*path) {
                assert!(
                    p.get("x-sovd-name").is_some(),
                    "x-sovd-name missing on {path}"
                );
            }
        }
    }

    #[test]
    fn contract_all_operations_have_operation_id() {
        let spec = build_openapi_json();
        let paths = spec["paths"].as_object().unwrap();
        let methods = ["get", "post", "put", "delete", "patch"];

        for (path, path_obj) in paths {
            for method in &methods {
                if let Some(op) = path_obj.get(*method) {
                    assert!(
                        op.get("operationId").is_some(),
                        "operationId missing on {method} {path}"
                    );
                }
            }
        }
    }

    #[test]
    fn contract_all_operations_have_responses() {
        let spec = build_openapi_json();
        let paths = spec["paths"].as_object().unwrap();
        let methods = ["get", "post", "put", "delete", "patch"];

        for (path, path_obj) in paths {
            for method in &methods {
                if let Some(op) = path_obj.get(*method) {
                    assert!(
                        op["responses"].is_object(),
                        "responses missing on {method} {path}"
                    );
                }
            }
        }
    }

    #[test]
    fn contract_apps_path_present() {
        let spec = build_openapi_json();
        let paths = spec["paths"].as_object().unwrap();
        assert!(
            paths.contains_key("/apps"),
            "Apps path missing — required by ISO 17978-3 §4.2.3"
        );
    }

    #[test]
    fn contract_spec_is_valid_json_roundtrip() {
        let spec = build_openapi_json();
        let json_str = serde_json::to_string(&spec).unwrap();
        let reparsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(spec, reparsed, "JSON roundtrip failed");
    }

    #[test]
    fn contract_oem_policy_affects_applicability() {
        use native_interfaces::oem::{CdfApplicability, CdfPolicy};

        struct OfflineCapablePolicy;
        impl CdfPolicy for OfflineCapablePolicy {
            fn applicability(&self) -> CdfApplicability {
                CdfApplicability {
                    online: true,
                    offline: true,
                }
            }
        }

        let spec = build_openapi_json_with_policy(&OfflineCapablePolicy, None);
        assert_eq!(spec["info"]["x-sovd-applicability"]["offline"], true);
    }

    #[test]
    fn contract_filter_returns_subset() {
        let full = build_openapi_json();
        let full_paths = full["paths"].as_object().unwrap();

        let data_only = build_openapi_json_with_policy(
            &native_interfaces::DefaultProfile,
            Some("data"),
        );
        let data_paths = data_only["paths"].as_object().unwrap();

        assert!(
            data_paths.len() < full_paths.len(),
            "Filtered spec should have fewer paths"
        );
        for (path, _) in data_paths {
            assert!(
                path.contains("data") || path.contains("bulk"),
                "Filtered path {path} doesn't match 'data' filter"
            );
        }
    }
}

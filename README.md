# OpenSOVD-native-server

**ISO 17978-3 (SOVD) conformant diagnostic server in Rust — part of the [Eclipse OpenSOVD](https://github.com/eclipse-opensovd) ecosystem.**

[![CI](https://github.com/eclipse-opensovd/OpenSOVD-native-server/actions/workflows/ci.yml/badge.svg)](https://github.com/eclipse-opensovd/OpenSOVD-native-server/actions)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

---

## Role in the Eclipse OpenSOVD Ecosystem

This project implements the **SOVD Server** role as defined in the
[OpenSOVD high-level design](https://github.com/eclipse-opensovd/opensovd/blob/main/docs/design/design.md).
It is a standalone Rust implementation of the ISO 17978-3 (SOVD) REST API that integrates
with the other OpenSOVD components:

```
┌─────────────────────────────────────────────────────────────────┐
│                    Eclipse OpenSOVD Ecosystem                    │
│                                                                  │
│  ┌──────────────────────┐    ┌────────────────────────────────┐ │
│  │ opensovd-core (C++)  │    │ OpenSOVD-native-server (Rust)  │ │
│  │ Server / Client /    │    │ ★ THIS PROJECT                 │ │
│  │ Gateway              │    │ SOVD Server + Gateway           │ │
│  └──────────────────────┘    └──────────┬─────────────────────┘ │
│                                          │ SOVD HTTP             │
│  ┌──────────────────────┐    ┌──────────▼─────────────────────┐ │
│  │ classic-diagnostic-  │◄───│ SovdHttpBackend                │ │
│  │ adapter (CDA, Rust)  │    │ (forwards to CDA via REST)     │ │
│  │ SOVD → UDS/DoIP      │    └────────────────────────────────┘ │
│  └──────────────────────┘                                        │
│                                                                  │
│  ┌──────────────────────┐    ┌────────────────────────────────┐ │
│  │ cpp-bindings         │    │ COVESA/vsomeip (C++ FFI)       │ │
│  │ C++ client libs for  │    │ SOME/IP reference impl.        │ │
│  │ Fault Manager / SOVD │    │ → native-comm-someip wrapper   │ │
│  └──────────────────────┘    └────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### Relationship to existing components

| OpenSOVD Component | This project's approach |
|---|---|
| **[opensovd-core](https://github.com/eclipse-opensovd/opensovd-core)** (C++) | Parallel Rust implementation of the SOVD Server role. Not a fork — shares the same ISO 17978-3 API contract. |
| **[classic-diagnostic-adapter](https://github.com/eclipse-opensovd/classic-diagnostic-adapter)** (Rust) | Primary backend. In **gateway mode** (default), this server forwards SOVD requests to one or more CDA instances via HTTP. DoIP/UDS is CDA's responsibility. |
| **Fault Library** ([design](https://github.com/eclipse-opensovd/opensovd/blob/main/docs/design/design.md)) | `native-core/fault_bridge.rs` implements the `FaultSink` trait from the OpenSOVD fault-lib design, bridging decentral fault reporters into the local Diagnostic Fault Manager. |
| **[COVESA/vsomeip](https://github.com/COVESA/vsomeip)** | `native-comm-someip` provides Rust FFI bindings to `libvsomeip3`. Feature-gated (`vsomeip-ffi`), stub mode without it. |

### Operating Modes

| Mode | Build | Description |
|------|-------|-------------|
| **Gateway** (default) | `cargo build --no-default-features` | Pure SOVD gateway — forwards to external CDA/SOVD backends via HTTP. No DoIP/UDS dependencies. |
| **Standalone** | `cargo build` (feature `local-uds`) | Embedded UDS/DoIP for direct ECU communication without a separate CDA. Uses `doip-codec` + `doip-definitions`. |

> **Recommendation:** Use **gateway mode** in production. The standalone mode with embedded
> DoIP/UDS (`native-comm-doip`, `native-comm-uds`) is provided for development/testing
> and for deployments where a separate CDA is not available. In the standard OpenSOVD
> architecture, DoIP/UDS translation is the CDA's responsibility.

## Key Features

- **ISO 17978-3** — Full spec coverage (51/51 mandatory requirements, [audit](docs/sovd-compliance-audit.md))
- **OData** — `@odata.context`, `@odata.count`, `$top/$skip/$filter/$orderby/$select`, `$metadata`, ETag/`If-None-Match`
- **Authentication** — API-Key (timing-safe), JWT (HS256/RS256), OIDC with JWKS discovery
- **Component Locking** — Exclusive access with auth-based ownership (SOVD §7.4)
- **Async Operations** — 202 Accepted + Location header, execution tracking (SOVD §7.7)
- **SSE Events** — Real-time fault change notifications (SOVD §7.11)
- **Fault Bridge** — OpenSOVD fault-lib compatible `FaultSink` → Diagnostic Fault Manager
- **SOME/IP** — [COVESA/vsomeip](https://github.com/COVESA/vsomeip) FFI bindings (optional)
- **Health Monitoring** — CPU, memory, system metrics via `/sovd/v1/health`
- **Prometheus Metrics** — `sovd_http_requests_total`, `sovd_http_request_duration_seconds`
- **227 tests**, Clippy pedantic clean, `#![forbid(unsafe_code)]` (except vSomeIP FFI)

## Workspace Structure

```
OpenSOVD-native-server/
├── native-interfaces/       # Shared types, traits, ComponentBackend
├── native-sovd/             # SOVD REST API (axum + tower)
├── native-core/             # Diagnostic core: router, fault mgr, lock mgr, fault bridge
├── native-health/           # System health monitoring
├── native-server/           # Main binary
├── native-comm-someip/      # COVESA/vsomeip FFI (SOME/IP)
├── native-comm-doip/        # DoIP transport (feature: local-uds)
├── native-comm-uds/         # UDS client (feature: local-uds)
└── config/                  # TOML configuration
```

## SOVD v1 API Endpoints

| Method   | Path                                              | SOVD § | Description                    |
|----------|---------------------------------------------------|--------|--------------------------------|
| `GET`    | `/sovd/v1`                                        | §5     | Server info & discovery        |
| `GET`    | `/sovd/v1/components`                             | §7.1   | List components (paginated)    |
| `GET`    | `/sovd/v1/components/{id}`                        | §7.1   | Get component details          |
| `POST`   | `/sovd/v1/components/{id}/connect`                | §7.1   | Connect to ECU via DoIP        |
| `POST`   | `/sovd/v1/components/{id}/disconnect`             | §7.1   | Disconnect from ECU            |
| `GET`    | `/sovd/v1/components/{id}/capabilities`           | §7.3   | Component capabilities         |
| `POST`   | `/sovd/v1/components/{id}/lock`                   | §7.4   | Acquire exclusive lock         |
| `GET`    | `/sovd/v1/components/{id}/lock`                   | §7.4   | Get lock status                |
| `DELETE` | `/sovd/v1/components/{id}/lock`                   | §7.4   | Release lock                   |
| `GET`    | `/sovd/v1/components/{id}/data`                   | §7.5   | List data identifiers          |
| `GET`    | `/sovd/v1/components/{id}/data/{dataId}`          | §7.5   | Read data (UDS DID)            |
| `PUT`    | `/sovd/v1/components/{id}/data/{dataId}`          | §7.5   | Write data (UDS DID)           |
| `POST`   | `/sovd/v1/components/{id}/data/bulk-read`         | §7.5.3 | Bulk read multiple DIDs        |
| `POST`   | `/sovd/v1/components/{id}/data/bulk-write`        | §7.5.3 | Bulk write multiple DIDs       |
| `GET`    | `/sovd/v1/components/{id}/faults`                 | §7.5   | List faults (paginated)        |
| `GET`    | `/sovd/v1/components/{id}/faults/{faultId}`       | §7.5   | Get single fault               |
| `DELETE` | `/sovd/v1/components/{id}/faults`                 | §7.6   | Clear all faults               |
| `DELETE` | `/sovd/v1/components/{id}/faults/{faultId}`       | §7.6   | Clear single fault             |
| `GET`    | `/sovd/v1/components/{id}/mode`                   | §7.6   | Get diagnostic mode            |
| `POST`   | `/sovd/v1/components/{id}/mode`                   | §7.6   | Set diagnostic mode            |
| `GET`    | `/sovd/v1/components/{id}/operations`             | §7.7   | List operations (paginated)    |
| `POST`   | `/sovd/v1/components/{id}/operations/{opId}`      | §7.7   | Execute operation              |
| `GET`    | `/sovd/v1/components/{id}/operations/{opId}/executions` | §7.7 | List executions           |
| `GET`    | `.../executions/{execId}`                         | §7.7   | Get execution status           |
| `DELETE` | `.../executions/{execId}`                         | §7.7   | Cancel execution               |
| `GET`    | `/sovd/v1/components/{id}/config`                 | §7.8   | Read component config          |
| `PUT`    | `/sovd/v1/components/{id}/config`                 | §7.8   | Write component config         |
| `POST`   | `/sovd/v1/components/{id}/proximityChallenge`     | §7.9   | Create proximity challenge     |
| `GET`    | `.../proximityChallenge/{challengeId}`            | §7.9   | Get challenge status           |
| `GET`    | `/sovd/v1/components/{id}/logs`                   | §7.10  | Get diagnostic logs            |
| `GET`    | `/sovd/v1/components/{id}/faults/subscribe`       | §7.11  | SSE fault subscription         |
| `GET`    | `/sovd/v1/groups`                                 | §7.2   | List groups (paginated)        |
| `GET`    | `/sovd/v1/groups/{groupId}`                       | §7.2   | Get group details              |
| `GET`    | `/sovd/v1/groups/{groupId}/components`            | §7.2   | Get group members              |
| `POST`   | `/sovd/v1/components/{id}/flash`                  | —      | OTA firmware flash             |
| `GET`    | `/sovd/v1/diag/keepalive`                         | —      | TesterPresent keepalive status |
| `GET`    | `/sovd/v1/health`                                 | —      | System health check            |

> All collection endpoints support OData pagination via `$top` and `$skip` query parameters.

## Quick Start

### Prerequisites

- Rust 1.75+ (`rustup` recommended)
- For SOME/IP: `libvsomeip3` (optional, stub mode without it)
- For cross-compilation: appropriate target toolchain (e.g. `aarch64-linux-gnu-gcc`)

### Build & Run

```bash
# Build
cargo build --release

# Run (binary is in native-server crate)
cargo run --release -p opensovd-native-server

# Server starts at http://0.0.0.0:8080/sovd/v1
```

### Cross-Compile (example: AArch64)

```bash
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
# Binary at: target/aarch64-unknown-linux-gnu/release/opensovd-native-server
```

## Configuration

Configuration is loaded via [figment](https://crates.io/crates/figment) from TOML files and environment variables (matching CDA's `opensovd-cda.toml` pattern):

```bash
# Environment variable overrides (SOVD__ prefix, __ separator)
export SOVD__SERVER__PORT=9090
export SOVD__DOIP__GATEWAY_PORT=13400
export SOVD__LOGGING__LEVEL=debug
```

### Configuration File (`config/opensovd-native-server.toml`)

```toml
[server]
host = "0.0.0.0"
port = 8080

[doip]
tester_address = "127.0.0.1"
tester_subnet = "255.255.0.0"
gateway_port = 13400
source_address = 3584  # 0x0E00
# tls_ca_cert = "certs/ca.pem"       # Optional: DoIP TLS
# tls_client_cert = "certs/client.pem"
# tls_client_key = "certs/client-key.pem"

[auth]
enabled = false
# api_key = "my-secret-key"          # Static API key
# jwt_secret = "my-jwt-secret"       # JWT HS256 secret
# public_paths = ["/sovd/v1/health"] # Paths excluded from auth

[logging]
level = "info"

# Component-to-ECU mappings
[[components]]
sovd_component_id = "hpc-main"
sovd_name = "HPC Main Controller"
doip_target_address = 1
doip_source_address = 3584
group = "powertrain"
features = ["faults", "data", "operations"]

[[components.data_identifiers]]
did = "F190"
name = "VIN"
access = "read-only"

[[components.operations]]
routine_id = "FF00"
name = "Self Test"

# Logical component groups (SOVD §7.2)
[[groups]]
id = "powertrain"
name = "Powertrain"
description = "Engine and transmission ECUs"
```

## Example API Calls

```bash
# Server info
curl http://localhost:8080/sovd/v1 | jq

# List components (with pagination)
curl "http://localhost:8080/sovd/v1/components?\$top=10&\$skip=0" | jq

# Connect to ECU
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/connect

# Component capabilities
curl http://localhost:8080/sovd/v1/components/hpc-main/capabilities | jq

# List data identifiers
curl http://localhost:8080/sovd/v1/components/hpc-main/data | jq

# Read DID 0xF190 (VIN)
curl http://localhost:8080/sovd/v1/components/hpc-main/data/0xF190 | jq

# Read faults
curl http://localhost:8080/sovd/v1/components/hpc-main/faults | jq

# Clear single fault
curl -X DELETE http://localhost:8080/sovd/v1/components/hpc-main/faults/fault-123

# Acquire exclusive lock
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/lock \
  -H "Content-Type: application/json" \
  -d '{"lockedBy":"tester-1"}' | jq

# Release lock
curl -X DELETE http://localhost:8080/sovd/v1/components/hpc-main/lock

# Execute routine 0xFF00
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/operations/0xFF00 \
  -H "Content-Type: application/json" \
  -d '{}' | jq

# List operation executions
curl http://localhost:8080/sovd/v1/components/hpc-main/operations/0xFF00/executions | jq

# Get/set diagnostic mode
curl http://localhost:8080/sovd/v1/components/hpc-main/mode | jq
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/mode \
  -H "Content-Type: application/json" \
  -d '{"mode":"extended"}' | jq

# List groups and members
curl http://localhost:8080/sovd/v1/groups | jq
curl http://localhost:8080/sovd/v1/groups/powertrain/components | jq

# Diagnostic logs
curl http://localhost:8080/sovd/v1/components/hpc-main/logs | jq

# Proximity challenge
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/proximityChallenge \
  -H "Content-Type: application/json" \
  -d '{}' | jq

# OTA Firmware Flash (base64-encoded binary)
FIRMWARE_B64=$(base64 < firmware.bin)
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/flash \
  -H "Content-Type: application/json" \
  -d "{\"firmware_data\": \"${FIRMWARE_B64}\", \"memory_address\": 536870912}" | jq

# Health check
curl http://localhost:8080/sovd/v1/health | jq

# With API key authentication
curl -H "X-API-Key: my-secret-key" http://localhost:8080/sovd/v1/components | jq
```

## Key Dependencies

| Crate | Purpose | Notes |
|-------|---------|-------|
| [`axum`](https://crates.io/crates/axum) / [`tower`](https://crates.io/crates/tower) | HTTP server + middleware | Same stack as CDA |
| [`figment`](https://crates.io/crates/figment) | Configuration (TOML + env) | Same as CDA |
| [`tokio`](https://crates.io/crates/tokio) / [`tracing`](https://crates.io/crates/tracing) | Async runtime + logging | — |
| [`reqwest`](https://crates.io/crates/reqwest) | HTTP client for SovdHttpBackend → CDA | Gateway mode |
| [`jsonwebtoken`](https://crates.io/crates/jsonwebtoken) | JWT / OIDC authentication | — |
| [`dashmap`](https://crates.io/crates/dashmap) | Concurrent stores (faults, locks, executions) | — |
| [`sled`](https://crates.io/crates/sled) | Persistent fault storage (optional feature `persist`) | — |
| [`doip-codec`](https://github.com/theswiftfox/doip-codec) | DoIP transport (ISO 13400) | Feature `local-uds` only |
| [COVESA/vsomeip](https://github.com/COVESA/vsomeip) | SOME/IP via C FFI | Feature `vsomeip-ffi` only |

## Related Projects

- [eclipse-opensovd/opensovd](https://github.com/eclipse-opensovd/opensovd) — Main repository, design documents
- [eclipse-opensovd/opensovd-core](https://github.com/eclipse-opensovd/opensovd-core) — C++ Server, Client, Gateway
- [eclipse-opensovd/classic-diagnostic-adapter](https://github.com/eclipse-opensovd/classic-diagnostic-adapter) — CDA (Rust, SOVD→UDS/DoIP)
- [eclipse-opensovd/cpp-bindings](https://github.com/eclipse-opensovd/cpp-bindings) — C++ client libs for SOVD core components
- [COVESA/vsomeip](https://github.com/COVESA/vsomeip) — SOME/IP reference implementation
- [eclipse-score](https://github.com/eclipse-score) — S-CORE platform (Fault API, Persistency, Logging)

## License

[Apache-2.0](LICENSE)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). All contributors must sign the
[Eclipse Contributor Agreement (ECA)](https://www.eclipse.org/legal/ECA.php).

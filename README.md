# OpenSOVD-native-server

**ISO 17978-3 (SOVD) conformant diagnostic server in Rust — part of the [Eclipse OpenSOVD](https://github.com/eclipse-opensovd) ecosystem.**

[![CI](https://github.com/rettde/OpenSOVD-native-server/actions/workflows/ci.yml/badge.svg)](https://github.com/rettde/OpenSOVD-native-server/actions)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.88%2B-orange.svg)](https://www.rust-lang.org/)

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

### Architecture

The server is a **pure SOVD gateway** — it forwards ISO 17978-3 REST requests to
one or more backends (CDA instances, native SOVD applications) via HTTP.
DoIP/UDS translation is the CDA's responsibility, keeping this server focused on
the SOVD API contract, authentication, fault aggregation, and gateway routing.

```
SOVD Clients (HTTP/JSON)
        │
        ▼
┌───────────────────────────────────────────┐
│        OpenSOVD-native-server             │
│  ComponentRouter → SovdHttpBackend(s)     │
│  FaultManager / LockManager / DiagLog     │
└───────────────┬───────────────────────────┘
                │ SOVD REST
        ┌───────┴───────┐
        ▼               ▼
   CDA (UDS/DoIP)   demo-ecu (example)
```

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
- **OEM Plugin Architecture** — `OemProfile` trait with compile-time auto-detection of vendor profiles
- **Feature Flags** — Lock-free atomic runtime toggles for audit, history, rate limiting, bridge; admin REST API at `/x-admin/features`
- **Historical Storage** — Time-range queries on faults and audit entries with background compaction
- **Backup/Restore** — Full diagnostic state snapshot (faults + audit) via admin API with tamper-evident audit trail
- **TLS Hot-Reload** — Certificate file polling (30s) with graceful reload; supports TLS + mTLS
- **Multi-Tenant** — JWT `tenant_id` claim, namespace isolation, per-tenant policy
- **Cloud Bridge** — Vehicle↔cloud brokered session management
- **Data Catalog** — COVESA VSS ontology, semantic metadata, NDJSON batch export, schema introspection
- **Persistent Storage** — Optional `sled` embedded DB via `persist` feature flag
- **Vault Secrets** — Optional HashiCorp Vault KV v2 provider via `vault` feature flag
- **WebSocket Bridge** — Optional `tokio-tungstenite` cloud↔vehicle tunnel via `ws-bridge` feature flag
- **OTLP Tracing** — Optional OpenTelemetry export via `otlp` feature flag
- **489 tests**, 81% line coverage, Clippy pedantic clean, `#![forbid(unsafe_code)]` (except vSomeIP FFI)

## Workspace Structure

```
OpenSOVD-native-server/
├── native-interfaces/       # Shared types, traits, ComponentBackend
├── native-sovd/             # SOVD REST API (axum + tower)
├── native-core/             # Diagnostic core: router, fault mgr, lock mgr, fault bridge
├── native-health/           # System health monitoring
├── native-server/           # Main binary
├── native-comm-someip/      # COVESA/vsomeip FFI (SOME/IP)
├── examples/demo-ecu/       # Example: mock ECU backend (BMS + Climate)
├── config/                  # TOML configuration
├── deploy/                  # Dockerfile, Helm chart, systemd unit
├── docs/                    # Architecture, ADRs, roadmap, compliance audits
└── .github/workflows/       # CI pipeline (clippy, test, fmt, SBOM, Docker)
```

## Feature Flags (Cargo)

All optional features are disabled by default to keep the dependency footprint minimal.

| Feature | Crate | Description |
|---------|-------|-------------|
| `persist` | native-core | **Experimental.** Persistent fault/audit storage via embedded `sled` DB (see [STABILITY.md](STABILITY.md)) |
| `vault` | native-core | HashiCorp Vault KV v2 secret provider (auto-populates auth secrets) |
| `ws-bridge` | native-core | **Experimental.** WebSocket bridge transport via `tokio-tungstenite` (see [STABILITY.md](STABILITY.md)) |
| `otlp` | native-server | OpenTelemetry OTLP trace export (Jaeger, Tempo, etc.) |
| `systemd` | native-server | sd_notify integration: `READY=1`, `WATCHDOG=1`, `STOPPING=1` (Linux HPC) |
| `vsomeip-ffi` | native-comm-someip | Real COVESA/vsomeip C FFI bindings (requires `libvsomeip3`) |

```bash
# Build with specific features
cargo build -p opensovd-native-server --features vault,ws-bridge

# Build for Linux HPC with systemd watchdog
cargo build -p opensovd-native-server --features systemd --release

# Build with all optional features
cargo build -p opensovd-native-server --features vault,ws-bridge,otlp,persist,systemd
```

## SOVD v1 API Endpoints

### Core SOVD (ISO 17978-3)

| Method   | Path                                              | SOVD § | Description                    |
|----------|---------------------------------------------------|--------|--------------------------------|
| `GET`    | `/sovd/v1`                                        | §5     | Server info & discovery        |
| `GET`    | `/sovd/v1/components`                             | §7.1   | List components (paginated)    |
| `GET`    | `/sovd/v1/components/{id}`                        | §7.1   | Get component details          |
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
| `GET`    | `/sovd/v1/components/{id}/configurations`         | §7.12  | Read component config          |
| `PUT`    | `/sovd/v1/components/{id}/configurations`         | §7.12  | Write component config         |
| `POST`   | `/sovd/v1/components/{id}/proximity-challenge`    | §7.9   | Create proximity challenge     |
| `GET`    | `.../proximity-challenge/{challengeId}`           | §7.9   | Get challenge status           |
| `GET`    | `/sovd/v1/components/{id}/logs`                   | §7.10  | Get diagnostic logs            |
| `GET`    | `/sovd/v1/components/{id}/faults/subscribe`       | §7.11  | SSE fault subscription         |
| `GET`    | `/sovd/v1/groups`                                 | §7.2   | List groups (paginated)        |
| `GET`    | `/sovd/v1/groups/{groupId}`                       | §7.2   | Get group details              |
| `GET`    | `/sovd/v1/groups/{groupId}/components`            | §7.2   | Get group members              |
| `GET`    | `/sovd/v1/health`                                 | —      | System health check            |
| `GET`    | `/sovd/v1/system-info`                            | —      | Runtime system info            |
| `GET`    | `/sovd/v1/audit`                                  | —      | Audit trail log                |
| `GET`    | `/sovd/v1/audit/export`                           | —      | Signed audit export (hash chain) |
| `GET`    | `/sovd/v1/compliance-evidence`                    | —      | ISO 17978-3 / UNECE R155 evidence |

### Software Packages (§5.5.10)

| Method   | Path                                                          | Description                    |
|----------|---------------------------------------------------------------|--------------------------------|
| `GET`    | `/sovd/v1/components/{id}/software-packages`                  | List software packages         |
| `POST`   | `/sovd/v1/components/{id}/software-packages/{pkgId}`          | Install package                |
| `GET`    | `/sovd/v1/components/{id}/software-packages/{pkgId}/status`   | Package transfer status        |
| `POST`   | `/sovd/v1/components/{id}/software-packages/{pkgId}/activate` | Activate package               |
| `POST`   | `/sovd/v1/components/{id}/software-packages/{pkgId}/rollback` | Rollback package               |

### Apps, Funcs, Areas (ISO 17978-3 §4.2.3)

| Method   | Path                                              | Description                    |
|----------|---------------------------------------------------|--------------------------------|
| `GET`    | `/sovd/v1/apps`                                   | List applications              |
| `GET`    | `/sovd/v1/apps/{appId}`                           | Get application details        |
| `GET`    | `/sovd/v1/apps/{appId}/capabilities`              | App capabilities               |
| `GET`    | `/sovd/v1/apps/{appId}/data`                      | List app data                  |
| `GET`    | `/sovd/v1/apps/{appId}/data/{dataId}`             | Read app data                  |
| `GET`    | `/sovd/v1/apps/{appId}/operations`                | List app operations            |
| `POST`   | `/sovd/v1/apps/{appId}/operations/{opId}`         | Execute app operation          |
| `GET`    | `/sovd/v1/funcs`                                  | List functions                 |
| `GET`    | `/sovd/v1/funcs/{funcId}`                         | Get function details           |
| `GET`    | `/sovd/v1/funcs/{funcId}/data`                    | List func data                 |
| `GET`    | `/sovd/v1/funcs/{funcId}/data/{dataId}`           | Read func data                 |
| `GET`    | `/sovd/v1/areas`                                  | List areas                     |
| `GET`    | `/sovd/v1/areas/{areaId}`                         | Get area details               |

### RXSWIN Tracking — UNECE R156 (F15)

| Method   | Path                                              | Description                    |
|----------|---------------------------------------------------|--------------------------------|
| `GET`    | `/sovd/v1/rxswin`                                 | List all RXSWIN entries        |
| `GET`    | `/sovd/v1/rxswin/report`                          | Vehicle-level RXSWIN report    |
| `GET`    | `/sovd/v1/rxswin/{component_id}`                  | Per-component RXSWIN           |
| `GET`    | `/sovd/v1/update-provenance`                      | Update provenance log          |

### TARA — ISO/SAE 21434 (F16)

| Method   | Path                                              | Description                    |
|----------|---------------------------------------------------|--------------------------------|
| `GET`    | `/sovd/v1/tara/assets`                            | TARA asset inventory           |
| `GET`    | `/sovd/v1/tara/threats`                           | TARA threat entries            |
| `GET`    | `/sovd/v1/tara/export`                            | Full TARA export document      |

### UDS Security Access — ISO 14229 (F17)

| Method   | Path                                              | Description                    |
|----------|---------------------------------------------------|--------------------------------|
| `GET`    | `/sovd/v1/x-uds/components/{id}/security-levels`  | UDS security levels            |
| `POST`   | `/sovd/v1/x-uds/components/{id}/security-access`  | Seed/key protocol (0x27)       |

### UCM Campaigns — AUTOSAR R24-11 (F18)

| Method   | Path                                              | Description                    |
|----------|---------------------------------------------------|--------------------------------|
| `GET`    | `/sovd/v1/ucm/campaigns`                          | List UCM campaigns             |
| `POST`   | `/sovd/v1/ucm/campaigns`                          | Create UCM campaign            |
| `GET`    | `/sovd/v1/ucm/campaigns/{id}`                     | Get campaign detail            |
| `POST`   | `/sovd/v1/ucm/campaigns/{id}/execute`             | Execute campaign               |
| `POST`   | `/sovd/v1/ucm/campaigns/{id}/rollback`            | Rollback campaign              |

### Vendor Extensions (x-uds)

| Method   | Path                                              | Description                    |
|----------|---------------------------------------------------|--------------------------------|
| `POST`   | `/sovd/v1/x-uds/components/{id}/connect`          | Connect to ECU via DoIP        |
| `POST`   | `/sovd/v1/x-uds/components/{id}/disconnect`       | Disconnect from ECU            |
| `POST`   | `/sovd/v1/x-uds/components/{id}/io/{dataId}`      | UDS IO Control (0x2F)          |
| `POST`   | `/sovd/v1/x-uds/components/{id}/comm-control`     | Communication Control (0x28)   |
| `POST`   | `/sovd/v1/x-uds/components/{id}/dtc-setting`      | Control DTC Setting (0x85)     |
| `GET`    | `/sovd/v1/x-uds/components/{id}/memory`           | Read Memory By Address (0x23)  |
| `PUT`    | `/sovd/v1/x-uds/components/{id}/memory`           | Write Memory By Address (0x3D) |
| `POST`   | `/sovd/v1/x-uds/components/{id}/flash`            | Request Download / Flash       |
| `GET`    | `/sovd/v1/x-uds/diag/keepalive`                   | TesterPresent keepalive status |

### Operational & Admin Endpoints

| Method    | Path                                              | Description                    |
|-----------|---------------------------------------------------|--------------------------------|
| `GET`     | `/sovd/v1/version-info`                           | SOVD version info (§4.1)      |
| `GET`     | `/sovd/v1/docs`                                   | Capability docs (§5.1)        |
| `GET`     | `/sovd/v1/$metadata`                              | OData metadata (§5.2)         |
| `GET`     | `/sovd/v1/components/{id}/snapshot`               | Diagnostic snapshot            |
| `GET`     | `/sovd/v1/export/faults`                          | NDJSON fault export (streaming)|
| `GET`     | `/sovd/v1/schema/data-catalog`                    | Schema introspection           |
| `GET`     | `/sovd/v1/components/{id}/data/subscribe`         | SSE data-change stream         |
| `GET`     | `/metrics`                                        | Prometheus scrape (config-gated)|
| `GET`     | `/openapi.json`                                   | OpenAPI 3.1 spec (CDF)        |
| `GET`     | `/healthz`                                        | K8s liveness probe             |
| `GET`     | `/readyz`                                         | K8s readiness probe            |
| `GET`     | `/x-admin/backup`                                 | Create state backup            |
| `POST`    | `/x-admin/restore`                                | Restore state backup           |
| `GET`     | `/x-admin/features`                               | List feature flags             |
| `GET/PUT` | `/x-admin/features/{flag}`                        | Get/set feature flag           |

> All collection endpoints support OData pagination via `$top` and `$skip` query parameters.

## Quick Start

### Prerequisites

- Rust 1.88+ (`rustup` recommended)
- For SOME/IP: `libvsomeip3` (optional, stub mode without it)

### Try it with the demo-ecu

The included `demo-ecu` example simulates a Battery Management System and a Cabin
Climate Controller. Use it to see the native server in action:

```bash
# Terminal 1 — start the mock ECU backend
cargo run -p demo-ecu
# → listening on http://localhost:3001/sovd/v1

# Terminal 2 — start the SOVD server (uncomment demo-ecu backend in config first)
cargo run -p opensovd-native-server
# → listening on http://localhost:8080/sovd/v1

# Terminal 3 — query via the SOVD gateway
curl http://localhost:8080/sovd/v1/components | jq
curl http://localhost:8080/sovd/v1/components/bms/data | jq
curl http://localhost:8080/sovd/v1/components/bms/faults | jq
curl -X POST http://localhost:8080/sovd/v1/components/bms/operations/self-test \
  -H "Content-Type: application/json" -d '{}' | jq

# Write data (read-write items only)
curl -X PUT http://localhost:8080/sovd/v1/components/climate/data/target-temp \
  -H "Content-Type: application/json" -d '{"value": 23.0}' | jq
```

### Configuration (`config/opensovd-native-server.toml`)

```toml
[server]
host = "0.0.0.0"
port = 8080

[auth]
enabled = false
# api_key = "my-secret-key"

[logging]
level = "info"

# Point the gateway at backends:
[[backends]]
name = "demo-ecu"
base_url = "http://localhost:3001"
api_prefix = "/sovd/v1"
component_ids = ["bms", "climate"]

# Add more backends (e.g. Classic Diagnostic Adapter):
# [[backends]]
# name = "CDA"
# base_url = "http://localhost:20002"
# api_prefix = "/sovd/v1"
# component_ids = ["brake-ecu", "eps-ecu"]
```

Environment variable overrides: `SOVD__SERVER__PORT=9090`, `SOVD__LOGGING__LEVEL=debug`

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
| [COVESA/vsomeip](https://github.com/COVESA/vsomeip) | SOME/IP via C FFI | Feature `vsomeip-ffi` only |

## Related Projects

- [eclipse-opensovd/opensovd](https://github.com/eclipse-opensovd/opensovd) — Main repository, design documents
- [eclipse-opensovd/opensovd-core](https://github.com/eclipse-opensovd/opensovd-core) — C++ Server, Client, Gateway
- [eclipse-opensovd/classic-diagnostic-adapter](https://github.com/eclipse-opensovd/classic-diagnostic-adapter) — CDA (Rust, SOVD→UDS/DoIP)
- [eclipse-opensovd/cpp-bindings](https://github.com/eclipse-opensovd/cpp-bindings) — C++ client libs for SOVD core components
- [COVESA/vsomeip](https://github.com/COVESA/vsomeip) — SOME/IP reference implementation
- [eclipse-score](https://github.com/eclipse-score) — S-CORE platform (Fault API, Persistency, Logging)

## Feature Roadmap

The server was built in four implementation waves. All are complete.

| Wave | Theme | Status |
|------|-------|--------|
| **Wave 1** | Security & entity model | ✅ Complete |
| **Wave 2** | HPC diagnostics & history | ✅ Complete |
| **Wave 3** | Enterprise & fleet | ✅ Complete |
| **Wave 4** | Data catalog & batch export | ✅ Complete |

### Wave 1 — Security & Entity Model ✅

Fine-grained AuthZ (per-resource, per-entity), SHA-256 hash-chained audit trail, full `apps`/`funcs` entities, software-package lifecycle (install/activate/rollback).

### Wave 2 — HPC Diagnostics & History ✅

KPI/system-info resources, historical diagnostic storage with time-range queries and background compaction, fault debouncing (FaultGovernor), per-client rate limiting, secrets abstraction, backup/restore, feature flags, TLS hot-reload, load tests (k6 + Criterion), fault injection tests.

### Wave 3 — Enterprise & Fleet ✅

Cloud bridge mode (BridgeTransport trait), multi-tenant isolation (JWT tenant_id + namespace), variant-aware discovery, zero-trust hardening, canary deployment routing, signed audit export, compliance evidence endpoint.

### Wave 4 — Data Catalog & Batch Export ✅

COVESA VSS semantic data catalog, NDJSON batch export (snapshot + faults), fault ontology enrichment (affectedSubsystem, correlatedSignals, classificationTags), schema introspection, SSE data-change streams, data contract versioning, reproducibility metadata.

> For the full research, gap analysis, and prioritization see
> [docs/integrated-roadmap.md](docs/integrated-roadmap.md).

### Production Enhancements (Future Work)

All waves are complete. Optional enhancements for production deployments:

| ID | Area | Status | Notes |
|----|------|--------|-------|
| F1 | **Persistent storage** | ✅ Done | `SledStorage` behind `persist` feature; 13 tests |
| F2 | **OTLP tracing** | ✅ Done | `otlp` feature; 11 instrumented handlers; Jaeger Compose stack |
| F3 | **WebSocket bridge** | ✅ Done | `WsBridgeTransport` behind `ws-bridge` feature; 8 tests |
| F4 | **Vault integration** | ✅ Done | `VaultSecretProvider` behind `vault` feature; 10 tests |
| F5 | **E2E test suite** | Planned | Testcontainers with CDA + demo-ecu for gateway round-trip |
| F6 | **SBOM / supply chain** | ✅ Done | `cargo-cyclonedx` CI job, CycloneDX JSON artifact |
| F7 | **Prometheus scrape** | ✅ Done | `MetricsConfig` gates `/metrics` endpoint via config |
| F8 | **SOME/IP real transport** | Planned | Validate `native-comm-someip` FFI against real COVESA/vsomeip |

> Full roadmap with architecture details: [docs/integrated-roadmap.md](docs/integrated-roadmap.md)

## License

[Apache-2.0](LICENSE)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). All contributors must sign the
[Eclipse Contributor Agreement (ECA)](https://www.eclipse.org/legal/ECA.php).

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
- **OEM Plugin Architecture** — `OemProfile` trait with AuthPolicy, AuthzPolicy, EntityIdPolicy, CdfPolicy sub-traits; compile-time auto-detection of vendor profiles
- **Feature Flags** — Lock-free atomic runtime toggles for audit, history, rate limiting, bridge; admin REST API at `/x-admin/features`
- **Historical Storage** — Time-range queries on faults and audit entries with pluggable `StorageBackend` trait and background compaction
- **Backup/Restore** — Full diagnostic state snapshot (faults + audit) via admin API with tamper-evident audit trail
- **TLS Hot-Reload** — Certificate file polling (30s) with graceful reload; supports TLS + mTLS
- **Multi-Tenant** — JWT `tenant_id` claim, namespace isolation, per-tenant policy
- **Cloud Bridge** — Feature-gated vehicle↔cloud brokered session management
- **Data Catalog** — COVESA VSS ontology, semantic metadata, NDJSON batch export, schema introspection
- **398 tests**, Clippy pedantic clean, `#![forbid(unsafe_code)]` (except vSomeIP FFI)

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

The SOVD standard and the broader ecosystem (Eclipse OpenSOVD, vendor implementations, ASAM direction)
point toward a clear evolution: from a standards-compliant REST gateway toward a **policy-driven,
enterprise-ready diagnostic platform** for software-defined vehicles and HPC architectures.
This roadmap captures the planned expansion in four waves.

| Wave | Theme | Status |
|------|-------|--------|
| **Wave 1** | Security & entity model | ✅ Complete |
| **Wave 2** | HPC diagnostics & history | ✅ Complete |
| **Wave 3** | Enterprise & fleet | ✅ Complete |
| **Wave 4** | AI-ready diagnostic data | ✅ Complete |

### Wave 1 — Security & Entity Model ✅

Fine-grained AuthZ (per-resource, per-entity), SHA-256 hash-chained audit trail, full `apps`/`funcs` entities, software-package lifecycle (install/activate/rollback).

### Wave 2 — HPC Diagnostics & History ✅

KPI/system-info resources, historical diagnostic storage with time-range queries and background compaction, fault debouncing (FaultGovernor), per-client rate limiting, secrets abstraction, backup/restore, feature flags, TLS hot-reload, load tests (k6 + Criterion), fault injection tests.

### Wave 3 — Enterprise & Fleet ✅

Cloud bridge mode (BridgeTransport trait), multi-tenant isolation (JWT tenant_id + namespace), variant-aware discovery, zero-trust hardening, canary deployment routing, signed audit export, compliance evidence endpoint.

### Wave 4 — AI-Ready Diagnostic Data ✅

COVESA VSS semantic data catalog, NDJSON batch export (snapshot + faults), fault ontology enrichment (affectedSubsystem, correlatedSignals, classificationTags), schema introspection, SSE data-change streams, data contract versioning, reproducibility metadata.

> For the full research, gap analysis, and prioritization see
> [docs/integrated-roadmap.md](docs/integrated-roadmap.md).

## License

[Apache-2.0](LICENSE)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). All contributors must sign the
[Eclipse Contributor Agreement (ECA)](https://www.eclipse.org/legal/ECA.php).

# Architecture Guide — OpenSOVD-native-server

> Modularer nativer SOVD-Server (Gateway) für HPC-Plattformen, architektonisch ausgerichtet am
> [Classic Diagnostic Adapter (CDA)](https://github.com/eclipse-opensovd/classic-diagnostic-adapter)
> des Eclipse OpenSOVD Projekts.

---

## 1. Überblick

OpenSOVD-native-server implementiert den [ISO 17978-3](https://www.iso.org/standard/85438.html) (SOVD) Standard
als REST/JSON-Gateway. Er nimmt SOVD-Anfragen entgegen und leitet sie an ein oder mehrere
Backends weiter — z.B. an CDA-Instanzen (SOVD→UDS/DoIP) oder Mock-ECUs. Optional kann er über
SOME/IP mit Adaptive-AUTOSAR-Diensten kommunizieren.

```
SOVD-Clients (HTTP/JSON)
        │
        ▼
┌──────────────────────────────────────────────────────┐
│               OpenSOVD-native-server                  │
│                                                       │
│  ┌────────────┐  ┌─────────────┐  ┌───────────────┐ │
│  │ native-sovd│  │ native-core │  │ native-health │ │
│  │ (REST API, │──│ (Router,    │──│ (System-      │ │
│  │  axum+tower│  │  FaultMgr,  │  │  Monitoring)  │ │
│  │  auth, OEM)│  │  LockMgr,   │  └───────────────┘ │
│  └────────────┘  │  AuditLog,  │                     │
│                  │  HttpBackend)│  ┌───────────────┐ │
│                  └──────┬──────┘  │native-comm-   │ │
│                         │         │   someip      │ │
│                         │         │ (vSomeIP FFI) │ │
│                         │         └───────────────┘ │
│  ┌──────────────────────┴────────────────────────┐  │
│  │            native-interfaces                   │  │
│  │  (Traits, SOVD types, OemProfile, Storage,     │  │
│  │   Secrets, EntityBackend, ComponentBackend)     │  │
│  └────────────────────────────────────────────────┘  │
└────────────────┬──────────────────┬──────────────────┘
                 │ SOVD HTTP         │ SOME/IP
                 ▼                   ▼
        CDA / demo-ecu       Adaptive AUTOSAR Apps
        (external backends)
```

---

## 2. Crate-Architektur

Das Workspace ist in 7 Crates aufgeteilt:

### 2.1 `native-interfaces` (≈ `cda-interfaces`)

**Zweck:** Gemeinsame Typen, Traits und Fehlerdefinitionen für alle Crates.

| Modul | Inhalt |
|-------|--------|
| `diag.rs` | UDS Service-IDs, `DiagTransport` (async trait), `DiagCommType`, `DiagCommAction`, `DiagComm`, `DiagnosticSession`, `EcuConnectionState`, `ServicePayload`, `UdsResponse` |
| `error.rs` | `DiagServiceError`, `DoipGatewaySetupError`, `ConnectionError`, `SomeIpError` — alle mit `thiserror` |
| `sovd.rs` | SOVD-API-Typen: `Collection<T>`, `SovdComponent`, `SovdFault`, `SovdData`, `SovdOperation`, `SovdErrorResponse`, `SovdMode`, `SovdModeDescriptor`, `SovdApp`, `SovdFunc`, `SovdSoftwarePackage` |
| `backend.rs` | `ComponentBackend` + `EntityBackend` Traits — Abstraktion für Gateway-Dispatch |
| `oem.rs` | `OemProfile` Supertrait (`AuthPolicy`, `EntityIdPolicy`, `DiscoveryPolicy`, `CdfPolicy`) + `DefaultProfile` |
| `storage.rs` | `StorageBackend` Trait + `InMemoryStorage` (BTreeMap) |
| `secrets.rs` | `SecretProvider` Trait + `EnvSecretProvider` + `StaticSecretProvider` |

**Design-Entscheidungen:**
- Alle SOVD-Typen nutzen `serde` mit `rename_all` für JSON-Konformität (camelCase, kebab-case, lowercase)
- `skip_serializing_if = "Option::is_none"` für optionale Felder
- `ComponentBackend` ist ein Trait Object — ermöglicht Gateway-Routing an beliebige Backends

### 2.2 `native-comm-someip` (≈ kein CDA-Pendant)

**Zweck:** SOME/IP-Kommunikation über vSomeIP FFI für Adaptive-AUTOSAR-Integration.

| Modul | Inhalt |
|-------|--------|
| `config.rs` | `SomeIpConfig`, `ServiceDefinition`, `MethodDefinition`, `EventGroupDefinition` |
| `service.rs` | `SomeIpRuntime` (init/start/stop/register), `SomeIpServiceProxy` (request/fire_and_forget/subscribe) |
| `ffi.rs` | Rust FFI-Deklarationen (`extern "C"`) matching `vsomeip_wrapper.h` |
| `runtime.rs` | `VsomeipApplication` — safe Rust wrapper über FFI mit Callback-Bridging |
| `ffi/vsomeip_wrapper.h` | C-Header mit opaken Handles und Callback-Typedefs |
| `ffi/vsomeip_wrapper.cpp` | C++-Implementierung, delegiert an `vsomeip::runtime` und `vsomeip::application` |
| `build.rs` | Kompiliert `vsomeip_wrapper.cpp` und linkt `libvsomeip3` (nur mit Feature) |

**Feature-Gate `vsomeip-ffi`:**
- **Ohne Feature** (Default): Stub-Modus — alle Operationen loggen Warnungen, Proxies geben leere Responses
- **Mit Feature**: Echte vSomeIP-Integration über C FFI

```
cargo build                              # Stub-Modus
cargo build --features vsomeip-ffi       # Echte vSomeIP-Integration
VSOMEIP_PREFIX=/opt/vsomeip cargo build --features vsomeip-ffi  # Custom Install-Pfad
```

**FFI-Architektur:**
```
Rust (safe)                    C wrapper                   C++ (vsomeip3)
─────────────                  ─────────                   ──────────────
VsomeipApplication             vsomeip_wrapper.cpp         vsomeip::application
  ├─ new()        ──────────▶  vsomeip_create()  ────────▶ runtime::get()->create_application()
  ├─ init()       ──────────▶  vsomeip_init()    ────────▶ app->init()
  ├─ start()      ──thread──▶  vsomeip_start()   ────────▶ app->start() [blocking]
  ├─ request()    ──────────▶  vsomeip_send_request() ──▶  app->send(msg)
  │   └─ oneshot::channel      message_trampoline() ◀────  register_message_handler λ
  ├─ stop()       ──────────▶  vsomeip_stop()    ────────▶ app->stop()
  └─ drop()       ──────────▶  vsomeip_destroy()           delete wrapper
```

**Callback-Bridging:**
- C++ `std::function` Callbacks → C Funktionspointer + `void* context`
- `message_trampoline()` dispatcht eingehende Nachrichten nach Typ:
  - `MT_RESPONSE` → matcht `oneshot::Sender` in `pending` HashMap
  - `MT_NOTIFICATION` → sendet auf `broadcast::Sender<SomeIpEvent>`
  - `MT_REQUEST` → sendet auf `mpsc::UnboundedSender<SomeIpMessage>`
- `availability_trampoline()` → sendet auf `broadcast::Sender<ServiceAvailability>`

### 2.3 `native-core` (≈ `cda-core`)

**Zweck:** Kern-Geschäftslogik — Gateway-Routing, Fault-Management, Audit, Locking.

| Modul | Inhalt |
|-------|--------|
| `router.rs` | `ComponentRouter` — dispatcht SOVD-Requests an registrierte `ComponentBackend`s |
| `http_backend.rs` | `SovdHttpBackend` — proxied SOVD REST an externe CDA/SOVD-Backends |
| `fault_manager.rs` | `FaultManager` — zentrale Fault-Aggregation (DashMap, optional sled-Persistenz) |
| `fault_bridge.rs` | `FaultBridge` — Adapter `FaultSink` → `FaultManager` (fault-lib-kompatibel) |
| `fault_governor.rs` | `FaultGovernor` — DFM-seitige Debounce-Schicht (W2.3) |
| `lock_manager.rs` | `LockManager` — exklusives Locking pro Component (auth-basierte Ownership) |
| `audit_log.rs` | `AuditLog` — Ring-Buffer + JSONL-Sink, SHA-256 Hash-Chain |
| `diag_log.rs` | `DiagLog` — diagnostisches Logging pro Component |

#### `ComponentRouter`

Gateway-Kern: registriert mehrere `ComponentBackend`-Implementierungen und routet SOVD-Anfragen:

```
SovdHttpBackend("http://cda:8080")  ──┐
SovdHttpBackend("http://demo:3000") ──┤
                                      ▼
                              ComponentRouter
                                      │
                          ┌───────────┼───────────┐
                          ▼           ▼           ▼
                     component-a  component-b  component-c
```

#### `FaultManager`

Thread-safe (DashMap) Fault-Aggregation aus mehreren Quellen:

```
report_fault(fault)           → Fault einfügen/überschreiben
clear_fault(id)               → einzelnen Fault entfernen
clear_faults_for_component()  → alle Faults einer Component löschen
get_faults_for_component()    → gefilterte Abfrage
```

#### `FaultGovernor` (W2.3)

DFM-seitige Debounce-Schicht — unterdrückt Rapid-Fire-Duplikate innerhalb eines konfigurierbaren
Zeitfensters. Implementiert die fault-lib Design-Anforderung für Multi-Fault-Aggregation im DFM.

### 2.4 `native-sovd` (≈ `cda-sovd`)

**Zweck:** SOVD REST API — axum-Router mit allen Endpunkten, Auth, OEM-Plugins.

| Modul | Inhalt |
|-------|--------|
| `routes.rs` | `build_router()` — axum-Router mit 60+ SOVD-Endpunkten + RED-Metrics-Middleware |
| `state.rs` | `AppState` — Shared State (`DiagState`, `SecurityState`, `RuntimeState`) |
| `auth.rs` | `AuthState`, `auth_middleware` — API-Key, JWT (HS256/RS256), OIDC (JWKS) |
| `dlt.rs` | `DltTextLayer` — DLT-Text-Format tracing Layer (lightweight, kein libdlt nötig) |
| `rate_limit.rs` | `RateLimiter` — Per-Client Token-Bucket mit Axum-Middleware |
| `openapi.rs` | OpenAPI 3.1 Spec-Generator mit OEM `CdfPolicy`-Extensions |
| `oem_sample.rs` | `SampleOemProfile` — Open-Source-Template für OEM-Anpassungen |

**Middleware-Stack (tower):**
1. `TraceLayer` — strukturiertes HTTP-Request-Logging
2. `TimeoutLayer` — 30s Request-Timeout
3. `CorsLayer` — konfigurierbare CORS
4. `auth_middleware` — JWT / API-Key / OIDC
5. `entity_id_validation_middleware` — OEM-spezifische Entity-ID-Validierung
6. `rate_limit_middleware` — Per-Client Rate Limiting
7. RED-Metrics-Middleware — `sovd_http_requests_total` + `sovd_http_request_duration_seconds`

**Request-Flow:**
```
HTTP Request
    │
    ▼
axum Router (/sovd/v1/...)
    │
    ├─ Middleware: auth → rate_limit → metrics
    ├─ Path + Query Extraction
    ├─ State<AppState> Injection
    │
    ▼
Handler-Funktion
    │
    ├─ state.backend.read_data()      → ComponentRouter → SovdHttpBackend → CDA
    ├─ state.diag.fault_manager.*()   → In-Memory Fault Store
    └─ state.runtime.health.*()       → sysinfo Metriken
    │
    ▼
Json<T> / StatusCode / SovdErrorResponse
```

### 2.5 `native-health`

**Zweck:** System-Health-Monitoring via `sysinfo`.

Liefert JSON mit:
- CPU-Count und -Auslastung
- Speicher (total, used, available, Prozent)
- System-Name, OS-Version, Hostname
- Uptime

### 2.6 `native-server` (≈ `cda-main`)

**Zweck:** Main-Binary — Konfiguration, Runtime-Initialisierung, Server-Start.

**Startup-Sequenz:**
```
1. Konfiguration laden (figment: TOML + Env)
2. Tracing initialisieren (EnvFilter + DltTextLayer + optional OTLP)
3. SovdHttpBackend(s) erstellen (pro Backend-URL)
4. ComponentRouter aufbauen (registriert alle Backends)
5. FaultManager + LockManager + AuditLog + HealthMonitor erstellen
6. SomeIpRuntime initialisieren (Stub)
7. AppState zusammenbauen (DiagState, SecurityState, RuntimeState)
8. axum::serve() mit Graceful Shutdown (10s Draining)
```

**Konfigurationsquellen (Priorität aufsteigend):**
1. `opensovd-native-server.toml` (Projekt-Root)
2. `config/opensovd-native-server.toml`
3. Environment-Variablen mit Prefix `SOVD_` und Separator `__`

### 2.7 `examples/demo-ecu`

**Zweck:** Mock-ECU-Backend (BMS + Climate Controller) für Gateway-Tests.

Eigenständiger axum-Server, der SOVD-Endpunkte bedient und als Backend für
`SovdHttpBackend` dient. Ermöglicht E2E-Tests ohne echte ECU-Hardware.

---

## 3. Dependency-Graph

```
native-server (Binary)
    ├── native-sovd
    │   ├── native-core
    │   │   └── native-interfaces
    │   ├── native-health
    │   └── native-interfaces
    ├── native-comm-someip
    │   └── native-interfaces
    └── native-interfaces
```

Alle Crates nutzen `workspace.dependencies` für einheitliche Versionen.

---

## 4. Datenfluss: SOVD Read-Data Request (Gateway)

```
Client                Gateway                   CDA / Backend
  │                     │                        │
  │  GET /sovd/v1/      │                        │
  │  components/hpc/    │                        │
  │  data/0xF190        │                        │
  │────────────────────▶│                        │
  │                     │                        │
  │              ComponentRouter                  │
  │              → resolve("hpc")                │
  │              → SovdHttpBackend               │
  │                     │                        │
  │              Forward GET request              │
  │                     │────────────────────────▶│
  │                     │  SOVD HTTP proxy        │
  │                     │                        │
  │                     │◀────────────────────────│
  │                     │  200 OK + JSON          │
  │                     │                        │
  │  200 OK             │                        │
  │  { "value": ... }   │                        │
  │◀────────────────────│                        │
```

---

## 5. Thread-Modell & Concurrency

| Ressource | Synchronisierung | Grund |
|-----------|------------------|-------|
| `FaultManager::faults` | `DashMap` | Lock-free concurrent R/W aus mehreren Handlern |
| `LockManager::locks` | `DashMap` | Concurrent lock acquire/release |
| `AuditLog::entries` | `std::Mutex<VecDeque>` | Ring-Buffer, kurze Locks |
| `FaultGovernor::state` | `DashMap` | Per-Fault debounce tracking |
| `RateLimiter::buckets` | `DashMap` | Per-Client token buckets |
| `HealthMonitor::system` | `std::Mutex` | Nur kurze Locks für `sysinfo` Refresh |
| `AppState` | `Arc<T>` | Shared ownership über alle axum-Handler (Clone via Arc) |

**Tokio-Runtime:** Multi-threaded (`#[tokio::main]`), alle I/O ist non-blocking.

---

## 6. Konfiguration

Die Konfiguration nutzt [figment](https://crates.io/crates/figment), wie auch CDA:

```toml
# config/opensovd-native-server.toml

[server]
host = "0.0.0.0"
port = 8080
# cert_path = "cert.pem"   # optional TLS
# key_path = "key.pem"

[logging]
level = "info"              # trace | debug | info | warn | error
format = "text"             # "text" oder "json" (SIEM-ready)
# otlp_endpoint = "http://localhost:4317"  # optional, requires --features otlp

[auth]
enabled = true
api_keys = ["my-secret-key"]
# jwt_secret = "..."
# oidc_issuer = "https://..."

[dlt]
enabled = false
ecu_id = "SOVD"
app_id = "NSVD"
ctx_id = "MAIN"
# daemon_socket = "/tmp/dlt"

[rate_limit]
enabled = true
max_requests = 100
window_secs = 60

[someip]
application_name = "opensovd-native-server"

# Backend-Konfiguration (Gateway → CDA / Mock-ECUs)
[[backends]]
name = "cda"
url = "http://localhost:8081"

[[backends]]
name = "demo-ecu"
url = "http://localhost:3000"
```

**Environment-Override-Beispiele:**
```bash
SOVD_SERVER__PORT=9090
SOVD_LOGGING__LEVEL=debug
SOVD_AUTH__ENABLED=false
```

---

## 7. Erweiterungspunkte

| Erweiterung | Wo | Status |
|-------------|----|--------|
| Neue SOVD-Endpunkte | `native-sovd/routes.rs` | ✅ 60+ Endpunkte implementiert |
| Neue Backends | `ComponentBackend` impl | ✅ `SovdHttpBackend` als Referenz |
| OEM-Anpassungen | `native-sovd/src/oem_*.rs` | ✅ Trait-basiert, auto-detected via `build.rs` |
| SOME/IP Services | Config `[[someip.*]]` | Stub-Modus ohne `vsomeip-ffi` Feature |
| Auth/Middleware | `native-sovd/src/auth.rs` | ✅ API-Key + JWT + OIDC |
| Persistierung | `native-core/fault_manager.rs` | ✅ sled Backend (Feature `persist`) |
| Apps/Funcs | `EntityBackend` trait | ✅ Default-Impl in `ComponentRouter` |
| Rate Limiting | `native-sovd/src/rate_limit.rs` | ✅ Token-Bucket per Client |
| Observability | DltTextLayer, OTLP, RED Metrics | ✅ JSON Logging, Prometheus, OTel |

### SOVD-Endpunkte (Auswahl)

| Endpunkt | SOVD § | Status |
|----------|--------|--------|
| Data Listing §7.5 | `GET /components/{id}/data` | ✅ |
| Operations §7.7 | `GET /components/{id}/operations` | ✅ |
| Faults §7.5 | `GET /components/{id}/faults/{faultId}` | ✅ |
| Locking §7.4 | `POST/GET/DELETE /components/{id}/lock` | ✅ |
| Capabilities §7.3 | `GET /components/{id}/capabilities` | ✅ |
| Bulk Data §7.5.3 | `POST /data/bulk-read` + `bulk-write` | ✅ |
| Groups §7.2 | `GET /groups`, `GET /groups/{id}` | ✅ |
| Proximity §7.9 | `POST .../proximityChallenge` | ✅ |
| Events/SSE §7.11 | `GET /components/{id}/faults/subscribe` | ✅ |
| Mode/Session §7.6 | `GET/POST /components/{id}/mode` | ✅ |
| Configuration §7.8 | `GET/PUT /components/{id}/config` | ✅ |
| Executions §7.7 | `GET .../executions`, `GET/DELETE .../executions/{id}` | ✅ |
| Apps §4.2.3 | `GET /apps`, `GET /apps/{id}` | ✅ |
| Funcs §4.2.3 | `GET /funcs`, `GET /funcs/{id}` | ✅ |
| SW Packages §5.5.10 | `POST .../activate`, `POST .../rollback` | ✅ |
| Audit Trail | `GET /sovd/v1/audit` | ✅ |
| System Info | `GET /sovd/v1/system-info` | ✅ |
| Pagination §5 | OData `$top`, `$skip`, `$filter`, `$orderby` | ✅ |

---

## 8. Alignment mit CDA

| CDA Crate | Native Crate | Gemeinsamkeiten |
|-----------|-------------|-----------------|
| `cda-interfaces` | `native-interfaces` | Error-Patterns, DiagComm-Typen, SOVD-Typen |
| `cda-core` | `native-core` | Fault-Management, Component-Routing |
| `cda-sovd` | `native-sovd` | axum + tower Stack, AppState-Pattern |
| `cda-main` | `native-server` | figment Config, Graceful Shutdown |

**Architekturelle Abgrenzung (seit v0.5.0):**

Der Native Server ist ein reiner **Gateway** — die UDS/DoIP-Kommunikation findet in der CDA statt.
Die ehemaligen Crates `native-comm-doip`, `native-comm-uds` und der `SovdTranslator` wurden
entfernt. Der Server kommuniziert über `SovdHttpBackend` mit CDA-Instanzen, die die eigentliche
SOVD→UDS-Übersetzung durchführen. Dies entspricht dem OpenSOVD High-Level-Design.

**Bewusste Unterschiede zum CDA:**
- `native-comm-someip` — CDA hat kein SOME/IP, da es nur klassische Diagnostik bedient
- `native-health` — HPC-spezifisch, CDA braucht kein System-Monitoring
- OEM-Plugin-System — trait-basiert mit auto-detection, CDA hat statische Konfiguration

---

## 9. API-Beispiele

### Discovery & Komponenten

```bash
# Server-Info (SOVD §5)
curl http://localhost:8080/sovd/v1 | jq

# Komponenten auflisten (paginiert)
curl "http://localhost:8080/sovd/v1/components?\$top=10&\$skip=0" | jq

# Einzelne Komponente
curl http://localhost:8080/sovd/v1/components/hpc-main | jq

# Verbinden / Trennen
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/connect
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/disconnect
```

### Daten & Faults

```bash
# DID-Katalog auflisten
curl http://localhost:8080/sovd/v1/components/hpc-main/data | jq

# DID lesen (VIN)
curl http://localhost:8080/sovd/v1/components/hpc-main/data/0xF190 | jq

# DID schreiben
curl -X PUT http://localhost:8080/sovd/v1/components/hpc-main/data/0xF190 \
  -H "Content-Type: application/json" \
  -d '{"value":"4d5254545354"}' | jq

# Bulk-Read (mehrere DIDs)
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/data/bulk-read \
  -H "Content-Type: application/json" \
  -d '{"data_ids":["F190","F191"]}' | jq

# Faults auflisten
curl http://localhost:8080/sovd/v1/components/hpc-main/faults | jq

# Einzelnen Fault löschen
curl -X DELETE http://localhost:8080/sovd/v1/components/hpc-main/faults/fault-123
```

### Locking, Mode, Capabilities

```bash
# Lock erwerben
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/lock \
  -H "Content-Type: application/json" \
  -d '{"lockedBy":"tester-1"}' | jq

# Lock freigeben
curl -X DELETE http://localhost:8080/sovd/v1/components/hpc-main/lock

# Diagnostic Mode abfragen / setzen
curl http://localhost:8080/sovd/v1/components/hpc-main/mode | jq
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/mode \
  -H "Content-Type: application/json" \
  -d '{"mode":"extended"}' | jq

# Capabilities
curl http://localhost:8080/sovd/v1/components/hpc-main/capabilities | jq
```

### Groups, Logs, Proximity, SSE

```bash
# Groups
curl http://localhost:8080/sovd/v1/groups | jq
curl http://localhost:8080/sovd/v1/groups/powertrain/components | jq

# Diagnostic Logs
curl http://localhost:8080/sovd/v1/components/hpc-main/logs | jq

# Proximity Challenge
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/proximityChallenge \
  -H "Content-Type: application/json" \
  -d '{}' | jq

# SSE Fault-Subscription (Dauerhafter Stream)
curl -N http://localhost:8080/sovd/v1/components/hpc-main/faults/subscribe

# Health Check
curl http://localhost:8080/sovd/v1/health | jq
```

### Authentifizierung

```bash
# Mit API-Key
curl -H "X-API-Key: my-secret-key" http://localhost:8080/sovd/v1/components | jq

# Mit JWT Bearer Token
curl -H "Authorization: Bearer eyJhbG..." http://localhost:8080/sovd/v1/components | jq
```

---

## 10. Build & Test

```bash
# Check (schnell, keine Codegen)
cargo check

# Unit Tests
cargo test

# Release Build (LTO + Strip)
cargo build --release

# Mit vSomeIP-Integration (requires libvsomeip3)
cargo build --release --features vsomeip-ffi

# Custom vSomeIP Install-Pfad
VSOMEIP_PREFIX=/opt/vsomeip cargo build --release --features vsomeip-ffi

# Mit persistentem FaultManager (sled statt DashMap)
cargo build --release --features persist

# Mit OpenTelemetry OTLP Export
cargo build --release --features otlp

# Cross-Compile (Beispiel: AArch64)
cargo build --release --target aarch64-unknown-linux-gnu
```

**Test-Coverage (398+ Tests, v0.12.0):**
- `native-interfaces` — 77 Tests (SOVD-Typen, StorageBackend, SecretProvider, OemProfile, Tenant, DataCatalog)
- `native-core` — 67 Tests (FaultManager, LockManager, DiagLog, Router, HttpBackend, FaultBridge, FaultGovernor, AuditLog)
- `native-health` — 6 Tests (JSON-Struktur, Speicher-Werte, Uptime)
- `native-sovd` — 160+ Tests (Pagination, Auth, Lock, HTTP-Handler, Bridge, Wave 4 endpoints, Feature flags)
- `native-server` — 1 Test

**Bekannte Test-Lücken:**

| Bereich | Tests | Grund |
|---------|-------|-------|
| `native-comm-someip` | 0 | Erfordert libvsomeip3 Runtime |
| OTLP Export | 0 | Erfordert laufenden OTel Collector |

### Open Source Readiness Check

| # | Prüfpunkt | Status | Details |
|---|-----------|--------|---------|
| 1 | **LICENSE** | ✅ | Apache-2.0 Volltext in `/LICENSE` |
| 2 | **NOTICE** | ✅ | Eclipse-konform mit Copyright, Third-Party-Content |
| 3 | **SPDX-Header** | ✅ | Alle `.rs`-Dateien: `// SPDX-License-Identifier: Apache-2.0` |
| 4 | **Copyright-Header** | ✅ | Alle `.rs`-Dateien: `// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project` |
| 5 | **Cargo.toml** | ✅ | `license = "Apache-2.0"`, `repository`, `homepage` gesetzt |
| 6 | **.gitignore** | ✅ | target/, .env, *.pem, *.key, .DS_Store, OEM-Dateien, MBDS-Audit |
| 7 | **CONTRIBUTING.md** | ✅ | ECA-Verweis, Code-Style, PR-Workflow |
| 8 | **Dependency-Lizenzen** | ✅ | MIT, Apache-2.0, BSD-3-Clause, ISC, Zlib — kein Copyleft |
| 9 | **Secrets im Code** | ✅ | Keine hartkodierten Keys in `.rs`-Dateien |
| 10 | **Proprietary Content** | ✅ | `oem_mbds.rs`, `MBDS_CONFORMANCE_AUDIT.md` gitignored |

### Weiterführende Dokumentation

- **[integrated-roadmap.md](integrated-roadmap.md)** — Feature-Roadmap (implementiert + geplant)
- **[future-work-implementation-plan.md](future-work-implementation-plan.md)** — Offene Punkte (F5, F8)
- **[iso-17978-3-compliance-audit.md](iso-17978-3-compliance-audit.md)** — ISO 17978-3 Compliance
- **[security-audit.md](security-audit.md)** — Security Audit
- **[adr/](adr/README.md)** — Architecture Decision Records (18 ADRs)

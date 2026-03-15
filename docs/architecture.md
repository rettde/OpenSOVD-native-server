# Architecture Guide — OpenSOVD-native-server

> Modularer nativer SOVD-Server für HPC-Plattformen, architektonisch ausgerichtet am
> [Classic Diagnostic Adapter (CDA)](https://github.com/eclipse-opensovd/classic-diagnostic-adapter)
> des Eclipse OpenSOVD Projekts.

---

## 1. Überblick

OpenSOVD-native-server implementiert den [ISO 17978-3](https://www.iso.org/standard/85438.html) (SOVD) Standard
als REST/JSON-Server. Er verbindet moderne SOVD-Clients (Diagnosewerkzeuge, OTA-Dienste) mit
klassischen Fahrzeug-ECUs über UDS/DoIP und optional mit Adaptive-AUTOSAR-Diensten über SOME/IP.

```
SOVD-Clients (HTTP/JSON)
        │
        ▼
┌───────────────────────────────────────────────────┐
│              OpenSOVD-native-server                       │
│                                                    │
│  ┌────────────┐  ┌────────────┐  ┌──────────────┐│
│  │ native-sovd│  │ native-core│  │native-health ││
│  │  (REST API)│──│(Translation│──│(System-      ││
│  │  axum+tower│  │ Faults,OTA)│  │ Monitoring)  ││
│  └─────┬──────┘  └─────┬──────┘  └──────────────┘│
│        │               │                          │
│  ┌─────▼──────┐  ┌─────▼──────┐  ┌──────────────┐│
│  │native-comm-│  │native-comm-│  │native-comm-  ││
│  │    uds     │  │    doip    │  │   someip     ││
│  │(UDS-Mgr,  │──│(DoipCodec, │  │(vSomeIP FFI) ││
│  │ TesterPres)│  │ TCP/UDP)   │  │              ││
│  └────────────┘  └─────┬──────┘  └──────────────┘│
│                        │                          │
│  ┌─────────────────────┴──────────────────────────┤
│  │          native-interfaces                      │
│  │   (Shared types, errors, SOVD/UDS definitions)  │
│  └─────────────────────────────────────────────────┤
└────────────────────────────────────────────────────┘
        │ TCP/UDP (DoIP)        │ SOME/IP
        ▼                       ▼
   Vehicle ECUs          Adaptive AUTOSAR Apps
```

---

## 2. Crate-Architektur

Das Workspace ist in 8 Crates aufgeteilt, die den CDA-Modulen entsprechen:

### 2.1 `native-interfaces` (≈ `cda-interfaces`)

**Zweck:** Gemeinsame Typen, Traits und Fehlerdefinitionen für alle Crates.

| Modul | Inhalt |
|-------|--------|
| `diag.rs` | UDS Service-IDs, `DiagTransport` (async trait), `DiagCommType`, `DiagCommAction`, `DiagComm`, `DiagnosticSession`, `EcuConnectionState`, `ServicePayload`, `UdsResponse`, `TesterPresentType` |
| `error.rs` | `DiagServiceError`, `DoipGatewaySetupError`, `ConnectionError`, `SomeIpError` — alle mit `thiserror` |
| `sovd.rs` | SOVD-API-Typen: `Collection<T>`, `SovdComponent`, `SovdFault`, `SovdData`, `SovdOperation`, `SovdErrorResponse`, `NetworkStructure` |

**Design-Entscheidungen:**
- Alle SOVD-Typen nutzen `serde` mit `rename_all` für JSON-Konformität (camelCase, kebab-case, lowercase)
- `skip_serializing_if = "Option::is_none"` für optionale Felder
- `DiagCommType::try_from(u8)` mappt UDS-SIDs auf Kategorien (Data, Faults, Modes, etc.)

### 2.2 `native-comm-doip` (≈ `cda-comm-doip`)

**Zweck:** DoIP-Kommunikation über TCP und UDP, basierend auf denselben Crates wie CDA.

| Modul | Inhalt |
|-------|--------|
| `config.rs` | `DoipConfig` — Tester-Adresse, Subnet, Gateway-Port, TLS-Port, Timeout, Source-Address |
| `connection.rs` | `DoipConnection` — TCP-Verbindung via `Framed<TcpStream, DoipCodec>`, Routing-Aktivierung, Diagnostic-Message-Austausch |
| `discovery.rs` | UDP-Broadcast Vehicle Identification Request (VIR) / Vehicle Announcement Message (VAM) Parsing |

**Protokoll-Stack:**
```
DoipConnection
    │
    ├─ connect()              → TCP-Verbindung zum Gateway
    ├─ activate_routing()     → RoutingActivationRequest/Response
    ├─ send_diagnostic(data)  → DiagnosticMessage + ACK/NACK + Response Loop
    │                           (inkl. NRC 0x78 ResponsePending Handling)
    └─ disconnect()           → Verbindung schließen
```

**Externe Crates:**
- `doip-codec` v2.0 — Tokio-Codec für DoIP-Nachrichten (Encoder/Decoder)
- `doip-definitions` v3.0 — `DoipMessage`, `DoipPayload`, `DoipHeader`, `ProtocolVersion`, `ActivationCode`
- `DoipMessageBuilder` — Builder-Pattern für korrekte Header-Berechnung

### 2.3 `native-comm-uds` (≈ `cda-comm-uds`)

**Zweck:** UDS-Protokoll-Management über eine DoIP-Verbindung.

| Modul | Inhalt |
|-------|--------|
| `manager.rs` | `UdsManager` — Session-Tracking, alle UDS-Services, `DtcInfo`-Parsing |
| `tester_present.rs` | `TesterPresentTask` — periodischer SID 0x3E Keepalive als Background-Task |

**UDS-Service-Abdeckung im `UdsManager`:**

| SID | Methode | Positive Response |
|-----|---------|------------------|
| `0x10` | `diagnostic_session_control()` | 0x50 |
| `0x11` | `ecu_reset()` | 0x51 |
| `0x14` | `clear_dtc()` | 0x54 |
| `0x19` | `read_dtc_by_status_mask()` | 0x59 — DTC-Records à 4 Bytes |
| `0x22` | `read_data_by_identifier()` | 0x62 — Daten ab Offset 3 |
| `0x27` | `security_access_request_seed()` / `_send_key()` | 0x67 |
| `0x2E` | `write_data_by_identifier()` | 0x6E |
| `0x31` | `routine_control_start()` | 0x71 |
| `0x34` | `request_download()` | 0x74 — MaxBlockSize-Extraktion |
| `0x36` | `transfer_data()` | 0x76 |
| `0x37` | `request_transfer_exit()` | 0x77 |
| `0x3E` | `tester_present()` | 0x7E (oder suppress) |

**NRC-Handling:**
- Jede Antwort mit Byte[0] == `0x7F` wird als Negative Response erkannt
- `NRC 0x78` (ResponsePending) wird in `DoipConnection::send_diagnostic()` transparent behandelt

### 2.4 `native-comm-someip` (≈ kein CDA-Pendant)

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

### 2.5 `native-core` (≈ `cda-core`)

**Zweck:** Kern-Geschäftslogik — Translation, Fault-Aggregation, OTA-Orchestrierung.

| Modul | Inhalt |
|-------|--------|
| `translation.rs` | `SovdTranslator` — zentrale SOVD↔UDS-Bridge |
| `fault_manager.rs` | `FaultManager` — zentrale Fault-Aggregation (DashMap-basiert) |
| `ota.rs` | `OtaFlashOrchestrator` — vollständiger UDS-Flash-Workflow |

#### `SovdTranslator`

Verwaltet pro SOVD-Component eine eigene DoIP-Verbindung + UDS-Manager + TesterPresent-Task:

```
ComponentMapping (Config)
    │
    ▼
connect_component(id)
    ├─ DoipConnection::new() + connect() + activate_routing()
    ├─ UdsManager::new(doip)
    └─ TesterPresentTask::spawn_for_ecu(id, uds, interval)
        │
        ▼
    DashMap<String, Arc<UdsManager>>     ← aktive Verbindungen
    DashMap<String, Arc<DoipConnection>>
    DashMap<String, TesterPresentTask>
```

**SOVD→UDS Mapping:**

| SOVD-API | UDS-Service |
|----------|-------------|
| `read_data(component, did)` | SID 0x22 ReadDataByIdentifier |
| `write_data(component, did, value)` | SID 0x2E WriteDataByIdentifier |
| `read_faults(component)` | SID 0x19 ReadDTCInformation (sub=0x02, mask=0xFF) |
| `clear_faults(component)` | SID 0x14 ClearDiagnosticInformation (group=0xFFFFFF) |
| `execute_routine(component, routine_id)` | SID 0x31 RoutineControl (sub=Start) |
| `switch_session(component, session)` | SID 0x10 DiagnosticSessionControl |

#### `FaultManager`

Thread-safe (DashMap) Fault-Aggregation aus mehreren Quellen:

```
report_fault(fault)           → Fault einfügen/überschreiben
clear_fault(id)               → einzelnen Fault entfernen
clear_faults_for_component()  → alle Faults einer Component löschen
get_faults_for_component()    → gefilterte Abfrage
update_from_uds_scan()        → alte Faults ersetzen durch neue UDS-Scan-Ergebnisse
```

#### `OtaFlashOrchestrator`

Vollständiger UDS-Flash-Workflow:

```
1. DiagnosticSessionControl → Programming (0x10, sub=0x02)
2. SecurityAccess           → Seed/Key (0x27)
3. RequestDownload          → Block-Size verhandeln (0x34)
4. TransferData             → Firmware in Chunks übertragen (0x36)
5. RequestTransferExit      → Transfer beenden (0x37)
6. ECUReset                 → Hard-Reset für Firmware-Aktivierung (0x11)
```

### 2.6 `native-sovd` (≈ `cda-sovd`)

**Zweck:** SOVD REST API — axum-Router mit allen Endpunkten.

| Modul | Inhalt |
|-------|--------|
| `routes.rs` | `build_router()` — axum-Router mit allen SOVD-v1-Endpunkten |
| `state.rs` | `AppState` — Shared State (`Arc<SovdTranslator>`, `Arc<FaultManager>`, `Arc<HealthMonitor>`) |

**Middleware-Stack (tower):**
1. `TraceLayer` — strukturiertes HTTP-Request-Logging
2. `TimeoutLayer` — 30s Request-Timeout (408 bei Überschreitung)
3. `CorsLayer` — permissive CORS für Entwicklung

**Request-Flow:**
```
HTTP Request
    │
    ▼
axum Router (/sovd/v1/...)
    │
    ├─ Path + Query Extraction
    ├─ State<AppState> Injection
    │
    ▼
Handler-Funktion
    │
    ├─ state.translator.read_data()   → UDS → DoIP → ECU
    ├─ state.fault_manager.get_*()    → In-Memory Fault Store
    └─ state.health.system_info()     → sysinfo Metriken
    │
    ▼
Json<T> / StatusCode / SovdErrorResponse
```

**DID-Parsing:** `data_id` wird als Hex interpretiert (z.B. `0xF190` → DID 0xF190).
**Routine-ID-Parsing:** `op_id` wird als Hex interpretiert (z.B. `0xFF00` → Routine 0xFF00).
**Daten-Encoding:** Request/Response-Payloads werden als Hex-Strings übertragen.

### 2.7 `native-health`

**Zweck:** System-Health-Monitoring via `sysinfo`.

Liefert JSON mit:
- CPU-Count und -Auslastung
- Speicher (total, used, available, Prozent)
- System-Name, OS-Version, Hostname
- Uptime

### 2.8 `native-server` (≈ `cda-main`)

**Zweck:** Main-Binary — Konfiguration, Runtime-Initialisierung, Server-Start.

**Startup-Sequenz:**
```
1. Konfiguration laden (figment: TOML + Env)
2. Tracing initialisieren (EnvFilter)
3. SovdTranslator erstellen (mit Component-Mappings)
4. FaultManager + HealthMonitor erstellen
5. SomeIpRuntime initialisieren (Stub)
6. AppState zusammenbauen
7. axum::serve() mit Graceful Shutdown
```

**Konfigurationsquellen (Priorität aufsteigend):**
1. `opensovd-native-server.toml` (Projekt-Root)
2. `config/opensovd-native-server.toml`
3. Environment-Variablen mit Prefix `SOVD_` und Separator `__`

---

## 3. Dependency-Graph

```
native-server (Binary)
    ├── native-sovd
    │   ├── native-core
    │   │   ├── native-comm-uds
    │   │   │   ├── native-comm-doip
    │   │   │   │   └── native-interfaces
    │   │   │   └── native-interfaces
    │   │   └── native-interfaces
    │   └── native-health
    ├── native-comm-someip
    │   └── native-interfaces
    └── native-interfaces
```

Alle Crates nutzen `workspace.dependencies` für einheitliche Versionen.

---

## 4. Datenfluss: SOVD Read-Data Request

```
Client                Server                    ECU
  │                     │                        │
  │  GET /sovd/v1/      │                        │
  │  components/hpc/    │                        │
  │  data/0xF190        │                        │
  │────────────────────▶│                        │
  │                     │                        │
  │              parse_did("0xF190")              │
  │              → DID = 0xF190                  │
  │                     │                        │
  │              translator.read_data("hpc", DID)│
  │              → uds.read_data_by_identifier() │
  │                     │                        │
  │              UDS Request: [0x22, 0xF1, 0x90] │
  │                     │────────────────────────▶│
  │                     │  DoipMessage            │
  │                     │  (DiagnosticMessage)    │
  │                     │                        │
  │                     │◀────────────────────────│
  │                     │  DoipMessage (ACK)      │
  │                     │                        │
  │                     │◀────────────────────────│
  │                     │  UDS: [0x62,F1,90,...] │
  │                     │                        │
  │              Response: data[3..]             │
  │              → hex::encode                   │
  │                     │                        │
  │  200 OK             │                        │
  │  { "did": "0xF190", │                        │
  │    "value": "5741..." }                      │
  │◀────────────────────│                        │
```

---

## 5. Datenfluss: OTA Firmware Flash

```
Client                Server                    ECU
  │                     │                        │
  │  POST .../flash     │                        │
  │  { firmware_data }  │                        │
  │────────────────────▶│                        │
  │                     │                        │
  │              OtaFlashOrchestrator::flash()    │
  │                     │                        │
  │              ① 0x10 Programming Session      │
  │                     │───────────────────────▶│
  │                     │◀──────────────────────│
  │              ② 0x27 Security Access          │
  │                     │───────────────────────▶│ Seed
  │                     │◀──────────────────────│
  │                     │───────────────────────▶│ Key
  │                     │◀──────────────────────│
  │              ③ 0x34 Request Download         │
  │                     │───────────────────────▶│
  │                     │◀──────────────────────│ MaxBlock
  │              ④ 0x36 Transfer Data (N×)       │
  │                     │───────────────────────▶│ Chunk 1
  │                     │◀──────────────────────│
  │                     │───────────────────────▶│ Chunk N
  │                     │◀──────────────────────│
  │              ⑤ 0x37 Transfer Exit            │
  │                     │───────────────────────▶│
  │                     │◀──────────────────────│
  │              ⑥ 0x11 ECU Reset                │
  │                     │───────────────────────▶│
  │                     │                        │ ⟳ Reboot
  │  200 OK             │                        │
  │  { bytes, blocks }  │                        │
  │◀────────────────────│                        │
```

---

## 6. Thread-Modell & Concurrency

| Ressource | Synchronisierung | Grund |
|-----------|------------------|-------|
| `FaultManager::faults` | `DashMap` | Lock-free concurrent R/W aus mehreren Handlern |
| `DoipConnection::framed` | `tokio::Mutex<Option<Framed>>` | Async-sicher, da Send/Receive sequentiell pro Connection |
| `DoipConnection::state` | `tokio::Mutex<EcuConnectionState>` | State-Updates aus async-Kontext |
| `UdsManager::current_session` | `tokio::RwLock` | Viele Leser (Status-Abfrage), seltene Schreiber (Session-Wechsel) |
| `SovdTranslator::uds_managers` | `DashMap` | Mehrere Components können parallel verbunden werden |
| `HealthMonitor::system` | `std::Mutex` | Nur kurze Locks für `sysinfo` Refresh |
| `AppState` | `Arc<T>` | Shared ownership über alle axum-Handler (Clone via Arc) |

**Tokio-Runtime:** Multi-threaded (`#[tokio::main]`), alle I/O ist non-blocking.

---

## 7. Konfiguration

Die Konfiguration nutzt [figment](https://crates.io/crates/figment), wie auch CDA:

```toml
# config/opensovd-native-server.toml

[server]
host = "0.0.0.0"
port = 8080

[doip]
tester_address = "192.168.1.100"
tester_subnet = "255.255.0.0"
gateway_port = 13400
tls_port = 0                    # 0 = kein TLS
send_timeout_ms = 5000
source_address = 3584           # 0x0E00

[someip]
application_name = "opensovd-native-server"
# vsomeip_config_path = "/etc/vsomeip/config.json"

[logging]
level = "info"                  # trace | debug | info | warn | error

# Component-Mappings (SOVD-Component → DoIP-ECU)
[[components]]
sovd_component_id = "hpc-main"
sovd_name = "HPC Main Controller"
doip_target_address = 1         # 0x0001
doip_source_address = 3584      # 0x0E00

[[components]]
sovd_component_id = "brake-ecu"
sovd_name = "Brake ECU"
doip_target_address = 16        # 0x0010
doip_source_address = 3584
```

**Environment-Override-Beispiele:**
```bash
SOVD_SERVER__PORT=9090
SOVD_DOIP__GATEWAY_PORT=13400
SOVD_LOGGING__LEVEL=debug
```

---

## 8. Erweiterungspunkte

| Erweiterung | Wo | Status |
|-------------|----|--------|
| Neue UDS-Services | `native-comm-uds/manager.rs` | ✅ 0x2F, 0x28, 0x85, 0x23, 0x3D implementiert |
| Neue SOVD-Endpunkte | `native-sovd/routes.rs` | ✅ IO-Control, Comm-Control, DTC-Setting, Memory R/W, OTA Flash |
| Neue ECU-Typen | `config/opensovd-native-server.toml` | ✅ 5 Beispiel-ECUs mit DIDs, Operations, Groups |
| SOME/IP Services | `config/opensovd-native-server.toml` | `[[someip.offered_services]]` / `[[someip.consumed_services]]` ergänzen |
| Auth/Middleware | `native-sovd/src/auth.rs` | ✅ API-Key + JWT Bearer tower-Layer (`[auth]` in Config) |
| TLS (DoIP) | `native-comm-doip/connection.rs` | ✅ `tokio-openssl` via `DoipStream` enum, `connect_tls()` / `auto_connect()` |
| Persistierung | `native-core/fault_manager.rs` | ✅ sled Backend (Feature `persist`), DashMap als Default |
| Data Listing §7.5 | `native-sovd/routes.rs` | ✅ `GET /components/{id}/data` — DID-Katalog aus Config |
| Operations §7.7 | `native-sovd/routes.rs` | ✅ `GET /components/{id}/operations` — Routinen-Katalog |
| Fault by ID §7.5 | `native-sovd/routes.rs` | ✅ `GET /components/{id}/faults/{faultId}` |
| Locking §7.4 | `native-core/lock_manager.rs` | ✅ `POST/GET/DELETE /components/{id}/lock` |
| Capabilities §7.3 | `native-sovd/routes.rs` | ✅ `GET /components/{id}/capabilities` |
| Bulk Data §7.5.3 | `native-sovd/routes.rs` | ✅ `POST /data/bulk-read` + `bulk-write` |
| Groups §7.2 | `native-sovd/routes.rs` | ✅ `GET /groups`, `GET /groups/{id}`, `GET /groups/{id}/components` |
| Proximity §7.9 | `native-sovd/routes.rs` | ✅ `POST .../proximityChallenge`, `GET .../proximityChallenge/{id}` |
| Logs §7.10 | `native-core/diag_log.rs` | ✅ `GET /components/{id}/logs` — Ring-Buffer |
| Events/SSE §7.11 | `native-sovd/routes.rs` | ✅ `GET /components/{id}/faults/subscribe` — SSE-Stream |
| Mode/Session §7.6 | `native-sovd/routes.rs` | ✅ `GET/POST /components/{id}/mode` — UDS 0x10 |
| Configuration §7.8 | `native-sovd/routes.rs` | ✅ `GET/PUT /components/{id}/config` — Config-DIDs |
| Fault Clear §7.6 | `native-sovd/routes.rs` | ✅ `DELETE /components/{id}/faults/{faultId}` — Einzel-DTC |
| Executions §7.7 | `native-sovd/routes.rs` | ✅ `GET .../executions`, `GET/DELETE .../executions/{id}` |
| Pagination §5 | `native-sovd/routes.rs` | ✅ OData `$top`, `$skip`, `$filter`, `$orderby` auf allen Listen |

---

## 9. Alignment mit CDA

| CDA Crate | Native Crate | Gemeinsamkeiten |
|-----------|-------------|-----------------|
| `cda-interfaces` | `native-interfaces` | Gleiche Error-Patterns, DiagComm-Typen |
| `cda-comm-doip` | `native-comm-doip` | Gleiche `doip-codec` + `doip-definitions` Crates |
| `cda-comm-uds` | `native-comm-uds` | Gleiche UdsManager-Patterns, TesterPresent-Tasks |
| `cda-core` | `native-core` | Translation-Layer, Component-Mapping |
| `cda-sovd` | `native-sovd` | axum + tower Stack, AppState-Pattern |
| `cda-main` | `native-server` | figment Config, Graceful Shutdown |

**Bewusste Abweichungen:**
- `native-comm-someip` — CDA hat kein SOME/IP, da es nur klassische Diagnostik bedient
- `native-health` — HPC-spezifisch, CDA braucht kein System-Monitoring
- OTA-Flash — CDA delegiert an externe Tools, Native führt den Flash-Workflow selbst aus

---

## 10. OTA Firmware Flash

Der native HPC-Server implementiert den vollständigen UDS-Flash-Workflow über DoIP — im Gegensatz zum CDA, der an externe Flash-Tools delegiert.

### Architektur

```
REST Client
    │  POST /sovd/v1/components/{id}/flash
    │  { "firmware_data": "<base64>", "memory_address": 0x20000000 }
    ▼
native-sovd/routes.rs → start_flash()
    │  Base64-Decode, Validierung
    ▼
native-core/translation.rs → flash()
    │  get_uds(component_id)
    ▼
native-core/ota.rs → OtaFlashOrchestrator::flash()
    │  Vollständige UDS-Sequenz
    ▼
native-comm-uds/manager.rs → UdsManager
    │  UDS-Frames über DoIP
    ▼
ECU (Firmware-Update)
```

### UDS-Sequenz

| Schritt | UDS SID | Beschreibung |
|---------|---------|-------------|
| 1 | `0x10` (sub=0x02) | DiagnosticSessionControl → Programming Session |
| 2 | `0x27` (sub=0x01/0x02) | SecurityAccess → Seed/Key-Austausch |
| 3 | `0x34` | RequestDownload → Block-Size verhandeln |
| 4 | `0x36` | TransferData → Firmware in Chunks senden |
| 5 | `0x37` | RequestTransferExit → Transfer abschließen |
| 6 | `0x11` (sub=0x01) | ECUReset → Hard Reset, neue Firmware aktivieren |

### Sicherheitsmaßnahmen

- **Request Body Limit**: 2 MiB max (via `RequestBodyLimitLayer`)
- **Authentifizierung**: Flash-Endpunkt erfordert gültige API-Key oder JWT
- **Leer-Prüfung**: Leere `firmware_data` wird mit 400 Bad Request abgelehnt
- **Base64-Validierung**: Ungültige Kodierung → 400 Bad Request
- **ECU-Verbindung**: Component muss verbunden sein, sonst 404 Not Found
- **Security Access**: UDS 0x27 Seed/Key vor Download obligatorisch

### Beispiel

```bash
# Firmware als Base64 kodieren
FIRMWARE_B64=$(base64 < firmware.bin)

# Flash starten (Component muss vorher verbunden sein)
curl -X POST http://localhost:8080/sovd/v1/components/hpc-main/flash \
  -H "Content-Type: application/json" \
  -H "X-API-Key: my-secret-key" \
  -d "{
    \"firmware_data\": \"${FIRMWARE_B64}\",
    \"memory_address\": 536870912
  }" | jq

# Response:
# {
#   "componentId": "hpc-main",
#   "bytesTransferred": 262144,
#   "blocksTransferred": 64,
#   "memoryAddress": "0x20000000",
#   "status": "completed"
# }
```

---

## 11. API-Beispiele

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

## 12. Build & Test

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

# Cross-Compile (Beispiel: AArch64)
cargo build --release --target aarch64-unknown-linux-gnu
```

**Test-Coverage (227 Tests gesamt):**
- `native-interfaces` — 33 Tests (Serialisierung aller SOVD-Typen, SID-Mapping, Display-Traits, camelCase-Prüfungen, displayCode-Regression)
- `native-core` — 75 Tests (FaultManager 8, LockManager 5, DiagLog 5, Translation 20, Router 10, HttpBackend 6, LocalBackend 10, FaultBridge 8, +3)
- `native-comm-uds` — 40 Tests (Mock-basiert via `DiagTransport`-Trait: Frame-Konstruktion, NRC-Handling, DTC-Parsing, Session Control, Security Access, alle UDS-Services)
- `native-health` — 6 Tests (JSON-Struktur, Speicher-Werte, Uptime)
- `native-sovd` — 73 Tests (8 Pagination inkl. $filter/$orderby→501, 5 Auth-Middleware E2E, 7 Lock-Enforcement/Ownership, 3 Flash-Handler, 50 HTTP-Handler-Integration)

**Bekannte Test-Lücken:**

| Bereich | Tests | Grund |
|---------|-------|-------|
| `native-comm-doip` | 0 | Netzwerk-abhängig (TCP/TLS), erfordert DoIP-Simulator oder Mock |
| `native-comm-someip` | 0 | Erfordert libvsomeip3 Runtime |
| OTA Flash Orchestrator | 0 | Erfordert aktive UDS-Verbindung (mock-fähig via `DiagTransport`) |

> Verbleibende Lücken sind infrastrukturbedingt (Hardware/Netzwerk). UDS-Tests nutzen jetzt den `DiagTransport`-Trait mit Mock-Transport, sodass keine echte ECU nötig ist.

### Code-Review Findings (implementiert)

| # | Kategorie | Finding | Maßnahme |
|---|-----------|---------|----------|
| 1 | **Statische Analyse** | 5 Clippy-Warnings (type_complexity, manual_div_ceil, manual_contains, field_reassign_with_default, needless_borrow) | Alle behoben — 0 Warnings |
| 2 | **Security** | API-Key-Vergleich nicht timing-safe | `subtle::ConstantTimeEq` für API-Key-Vergleich (Schutz gegen Timing-Side-Channel) |
| 3 | **Security** | Kein Request-Body-Size-Limit | `RequestBodyLimitLayer` (2 MiB) gegen Payload-DoS |
| 4 | **Security** | `execution_store` / `proximity_store` unbounded | Bounded auf 10.000 / 1.000 Einträge mit FIFO-Eviction |
| 5 | **Robustheit** | `unwrap_or(0)` bei DID-Hex-Parsing sendet stillschweigend DID 0x0000 an ECU | `map_err` → `DiagServiceError::InvalidRequest` mit klarer Fehlermeldung |
| 6 | **Robustheit** | `hex::decode().unwrap_or_default()` in `bulk_write` verschluckt Fehler | Explizite Fehlerbehandlung mit Error-Feld im Bulk-Response |
| 7 | **Robustheit** | `Mutex::lock().unwrap()` in `DiagLog` — Panic bei Mutex-Poisoning | `unwrap_or_else(\|e\| e.into_inner())` — graceful Recovery |
| 8 | **Safety** | Keine compile-time Warning-Durchsetzung | `#![deny(warnings)]` in `native-interfaces`, `native-core`, `native-sovd` |
| 9 | **Coding Guidelines** | Komplexer Typ `Arc<RwLock<HashMap<...>>>` | Type-Alias `ProxyMap` extrahiert |
| 10 | **Coding Guidelines** | Manuelle `div_ceil`-Berechnung | Stdlib `.div_ceil()` |
| 11 | **Traceability** | SOVD-Standard-§-Referenzen | Konsistent in Routes (§5, §7.2–§7.11), Translation, LockManager, DiagLog |
| 12 | **HTTP-Semantik** | Alle `DiagServiceError` → 500 Internal Server Error | `diag_error()` mappt 14 Varianten auf korrekte HTTP-Codes (404, 400, 501, 403, 502, 504) |
| 13 | **Performance** | `get_component`/`get_group` iteriert alle Einträge (O(n)) | Direkte Lookup-Methoden `get_component()`, `get_group()` im Translator |
| 14 | **HTTP-Semantik** | `connect`/`disconnect` → 200 OK ohne Body | 204 NO_CONTENT (REST-konform) |
| 15 | **HTTP-Semantik** | Flash-Stub → 404 NOT_FOUND | 501 NOT_IMPLEMENTED (semantisch korrekt) |
| 16 | **Robustheit** | SSE `unwrap_or_default()` verschluckt Fehler | `tracing::warn!` bei Serialisierungsfehler |
| 17 | **Feature** | OTA-Flash nur als Stub (501) | Vollständig verdrahtet: Route → Translator → OtaFlashOrchestrator → UDS |

### Open Source Readiness Check

| # | Prüfpunkt | Status | Details |
|---|-----------|--------|---------|
| 1 | **LICENSE** | ✅ | Apache-2.0 Volltext in `/LICENSE` |
| 2 | **NOTICE** | ✅ | Eclipse-konform mit Copyright, Third-Party-Content |
| 3 | **SPDX-Header** | ✅ | Alle 30 `.rs`-Dateien: `// SPDX-License-Identifier: Apache-2.0` |
| 4 | **Copyright-Header** | ✅ | Alle 30 `.rs`-Dateien: `// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project` |
| 5 | **Cargo.toml** | ✅ | `license = "Apache-2.0"`, `repository`, `homepage` gesetzt |
| 6 | **.gitignore** | ✅ | target/, .env, *.pem, *.key, .DS_Store, IDE-Dateien |
| 7 | **CONTRIBUTING.md** | ✅ | ECA-Verweis, Code-Style, PR-Workflow |
| 8 | **Dependency-Lizenzen** | ✅ | 218 Crates: MIT, Apache-2.0, BSD-3-Clause, ISC, Zlib — kein Copyleft |
| 9 | **Secrets im Code** | ✅ | Keine hartkodierten Keys in `.rs`-Dateien |
| 10 | **Secrets in Config** | ✅ | `default.toml` enthält keine Platzhalter-Secrets mehr |
| 11 | **Private Keys/Certs** | ✅ | Keine `.pem`/`.key`-Dateien im Repository |
| 12 | **Copyleft-Risiko** | ✅ | `r-efi` ist tri-lizenziert (MIT OR Apache-2.0 OR LGPL) — Apache-2.0 wird verwendet |

### CDA vs. SOVD-Standard Analyse

Detaillierte Analyse des Eclipse OpenSOVD Classic Diagnostic Adapter (CDA) — Implementierungsstand,
Beyond-Standard-Features und Vergleich mit Native: siehe **[cda-sovd-analysis.md](cda-sovd-analysis.md)**.

Abgeleitete Requirements für Native (gefiltert auf standard-konforme Erweiterungen):
siehe **[requirements-cda-adaptions.md](requirements-cda-adaptions.md)**.

### Architektur-Review

Abgleich der aktuellen Native-Implementierung mit dem OpenSOVD High Level Design —
Rollenverteilung Server vs. CDA vs. Gateway, identifizierte Abweichungen und Refactoring-Empfehlungen:
siehe **[architecture-review.md](architecture-review.md)**.

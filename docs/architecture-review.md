# Architektur-Review: OpenSOVD-native-server

**Datum:** März 2026  
**Referenz:** [OpenSOVD High Level Design](https://github.com/eclipse-opensovd/opensovd/blob/main/docs/design/design.md)

---

## 1. Befund: Der Native Server ist ein CDA-Klon, kein SOVD Server

### Ist-Zustand (aktuell implementiert)

```
                   SOVD Client
                       │
                       │ HTTP (REST)
                       ▼
               ┌───────────────┐
               │  native-sovd  │  ← axum REST API
               └───────┬───────┘
                       │
               ┌───────▼───────┐
               │  native-core  │  ← SovdTranslator: SOVD → UDS Übersetzung
               │               │     OtaFlashOrchestrator: UDS Flash
               │               │     FaultManager, LockManager, DiagLog
               └───────┬───────┘
                       │
            ┌──────────┼──────────┐
            ▼                     ▼
   ┌────────────────┐   ┌────────────────┐
   │ native-comm-uds│   │native-comm-doip│  ← UDS-Protokoll + DoIP-Transport
   └────────┬───────┘   └────────┬───────┘
            │                     │
            └──────────┬──────────┘
                       ▼
                  Legacy ECU (UDS/DoIP)
```

### Soll-Zustand (laut OpenSOVD Design)

```
                   SOVD Client
                       │
                       │ HTTP (REST)
                       ▼
               ┌───────────────┐
               │  SOVD Server  │  ← REST API, Fault Manager, Service Apps,
               │               │     Diagnostic DB, Auth, Locking
               └───────┬───────┘
                       │
                       │ SOVD (HTTP intern oder IPC)
                       ▼
               ┌───────────────┐
               │ SOVD Gateway  │  ← Routing zu Adaptern, Proxies, anderen Servern
               └──┬─────────┬──┘
                  │         │
                  ▼         ▼
          ┌───────────┐  ┌──────────────┐
          │    CDA    │  │ Rest of      │
          │ (SOVD→UDS)│  │ Vehicle SOVD │  ← Native SOVD ECUs
          └─────┬─────┘  └──────────────┘
                │
                │ UDS/DoIP
                ▼
           Legacy ECU
```

---

## 2. Detaillierte Abweichungen

### 2.1 Kritisch: UDS/DoIP-Stack gehört nicht in den SOVD Server

| Crate | Aktuelle Rolle | Laut Design gehört zu | Problem |
|-------|---------------|----------------------|---------|
| `native-comm-doip` | DoIP-Verbindung zu ECUs | **CDA** | Server darf nicht direkt mit ECUs sprechen |
| `native-comm-uds` | UDS-Protokoll (0x10, 0x22, 0x2E, 0x31, ...) | **CDA** | Kompletter UDS-Stack dupliziert CDA-Logik |
| `native-core/translation.rs` | `SovdTranslator`: SOVD→UDS Übersetzung | **CDA** | Das IST ein CDA — nicht ein Server |
| `native-core/ota.rs` | `OtaFlashOrchestrator`: UDS Flash (0x34/36/37) | **CDA** oder **Flash Service App** | UDS-Flash ist CDA-Verantwortung |

**Konsequenz:** Der `SovdTranslator` mit seinen 15+ UDS-Operationen (`read_data`, `write_data`, `read_faults`, `clear_faults`, `execute_routine`, `switch_session`, `io_control`, `communication_control`, `control_dtc_setting`, `read_memory`, `write_memory`, `flash`, `bulk_read`, `bulk_write`, `read_config`, `write_config`) ist eine vollständige CDA-Reimplementierung.

### 2.2 Korrekt platziert im Server

| Crate/Modul | Rolle | Bewertung |
|-------------|-------|-----------|
| `native-sovd` (REST API, auth, routes) | SOVD Server HTTP-Interface | ✅ korrekt |
| `native-core/fault_manager.rs` | Diagnostic Fault Manager | ✅ korrekt (Design: "zentrale Fehleraggregation") |
| `native-core/lock_manager.rs` | Locking (§7.4) | ✅ korrekt |
| `native-core/diag_log.rs` | Diagnostic Log Buffer (§7.10) | ✅ korrekt |
| `native-health` | Health Monitoring | ✅ korrekt |
| `native-comm-someip` | SOME/IP für AUTOSAR Adaptive | ✅ korrekt (HPC-spezifisch) |
| `native-interfaces` | Shared Types | ✅ korrekt |

### 2.3 Fehlend

| Komponente | Beschreibung | Status |
|------------|-------------|--------|
| **SOVD Gateway** | Routing zu CDA, Proxy, anderen SOVD-Servern | ❌ fehlt komplett |
| **SOVD Client Library** | HTTP-Client um CDA/andere Server anzusprechen | ❌ fehlt — stattdessen wird UDS direkt gesprochen |
| **Service App Framework** | Erweiterbare Service-Apps (Flash Master, DTC Clear) | ❌ fehlt — OTA ist direkt im Core |
| **IPC-Schicht** | Kommunikation Server ↔ Gateway ↔ Adapter | ❌ fehlt |

---

## 3. Ursachenanalyse

Der Native Server wurde als **monolithischer Standalone-Server** implementiert, der alle Rollen (Server + CDA + Gateway) in einem Prozess vereint. Dies war vermutlich der pragmatische Ansatz für einen Prototyp, hat aber folgende Probleme:

1. **Doppelte Implementierung** — Die gleiche SOVD→UDS Logik existiert sowohl im CDA als auch hier
2. **Keine Wiederverwendung** — Der CDA kann nicht als Backend genutzt werden
3. **Kein Gateway** — Kein Routing zu verschiedenen Backends (CDA für Legacy-ECUs, direkte SOVD-Verbindung für native ECUs)
4. **Keine Skalierung** — Server, Gateway und CDA können nicht unabhängig deployed werden
5. **Keine native SOVD-ECUs** — Das Design sieht vor, dass der Gateway auch mit ECUs kommuniziert, die bereits SOVD sprechen. Das ist mit der aktuellen Architektur nicht möglich.

---

## 4. Durchgeführtes Refactoring (abgeschlossen)

### Phase 1: Separation of Concerns — ✅ IMPLEMENTIERT

**a) `ComponentBackend` Trait** (`native-interfaces/src/backend.rs`)

```rust
#[async_trait]
pub trait ComponentBackend: Send + Sync {
    fn name(&self) -> &str;
    fn list_components(&self) -> Vec<SovdComponent>;
    fn get_component(&self, component_id: &str) -> Option<SovdComponent>;
    fn handles_component(&self, component_id: &str) -> bool;
    async fn connect(&self, component_id: &str) -> Result<(), DiagServiceError>;
    async fn read_data(&self, component_id: &str, data_id: &str) -> Result<serde_json::Value, DiagServiceError>;
    async fn read_faults(&self, component_id: &str) -> Result<Vec<SovdFault>, DiagServiceError>;
    async fn execute_operation(&self, component_id: &str, op_id: &str, params: Option<&[u8]>) -> Result<serde_json::Value, DiagServiceError>;
    // ... vollständige SOVD-API-Oberfläche (25+ Methoden)
}
```

**b) `LocalUdsBackend`** (`native-core/src/local_backend.rs`) — wraps `SovdTranslator` hinter Feature-Gate `local-uds`:

```rust
pub struct LocalUdsBackend { translator: Arc<SovdTranslator> }
impl ComponentBackend for LocalUdsBackend { ... }
```

**c) `SovdHttpBackend`** (`native-core/src/http_backend.rs`) — HTTP-Client zum externen CDA:

```rust
pub struct SovdHttpBackend { config: SovdHttpBackendConfig, client: reqwest::Client, ... }
impl ComponentBackend for SovdHttpBackend { ... }
```

**d) `ComponentRouter`** (`native-core/src/router.rs`) — Gateway-Pattern, aggregiert Backends:

```rust
pub struct ComponentRouter { backends: Vec<Arc<dyn ComponentBackend>> }
impl ComponentBackend for ComponentRouter { ... }  // Selbst ein Backend!
```

**e) `FaultBridge`** (`native-core/src/fault_bridge.rs`) — fault-lib kompatibles Pattern:

```rust
pub struct FaultBridge { fault_manager: Arc<FaultManager> }
impl FaultSink for FaultBridge { ... }  // Liefert FaultRecords an DFM
```

### Neue Architektur (implementiert)

```
                   SOVD Client
                       │
                       │ HTTP (REST)
                       ▼
               ┌───────────────┐
               │  native-sovd  │  ← axum REST API (routes.rs: state.backend.*)
               └───────┬───────┘
                       │
               ┌───────▼───────┐
               │ ComponentRouter│  ← Gateway: dispatches per component
               │   (Gateway)    │
               └──┬──────────┬──┘
                  │          │
                  ▼          ▼
         ┌─────────────┐  ┌──────────────┐
         │LocalUdsBackend│  │SovdHttpBackend│
         │(feature-gated)│  │(standard mode)│
         └───────┬───────┘  └──────┬───────┘
                 │                  │ HTTP (SOVD REST)
                 │ UDS/DoIP         ▼
                 ▼            ┌──────────┐
           Legacy ECU         │ Ext. CDA │
                              └──────────┘
```

### Phase 2: Vollständige Trennung (zukünftig)

- Server ohne `local-uds` Feature kompilieren: `cargo build --no-default-features`
- Nur `[[backends]]` in Config → rein standard-konform, kein UDS/DoIP im Server
- CDA als eigenständiger Prozess (bestehender eclipse-opensovd/classic-diagnostic-adapter)

---

## 5. Geänderte Dateien

| Datei | Änderung |
|-------|----------|
| `native-interfaces/src/backend.rs` | **NEU** — `ComponentBackend` Trait (25+ Methoden) |
| `native-interfaces/src/lib.rs` | Export `backend` Modul |
| `native-interfaces/src/sovd.rs` | `PartialEq`/`Eq` auf `SovdFaultSeverity`, `SovdFaultStatus` |
| `native-core/src/router.rs` | **NEU** — `ComponentRouter` (Gateway-Pattern) mit Tests |
| `native-core/src/http_backend.rs` | **NEU** — `SovdHttpBackend` (reqwest HTTP-Client) |
| `native-core/src/local_backend.rs` | **NEU** — `LocalUdsBackend` (wraps `SovdTranslator`) |
| `native-core/src/fault_bridge.rs` | **NEU** — `FaultBridge` (fault-lib kompatibel) mit Tests |
| `native-core/src/lib.rs` | Module + Feature-Gates (`local-uds`) |
| `native-core/Cargo.toml` | `reqwest`, `base64`, optionale UDS/DoIP deps |
| `native-sovd/src/state.rs` | `AppState.backend: Arc<dyn ComponentBackend>` statt `translator` |
| `native-sovd/src/routes.rs` | Alle Handler auf `state.backend.*`, keine `native_comm_uds` Typen |
| `native-sovd/Cargo.toml` | `native-comm-uds` Dependency entfernt |
| `native-server/src/main.rs` | Backend-Konfiguration, `ComponentRouter` Init, `FaultBridge` |
| `native-server/Cargo.toml` | Feature-Gates (`local-uds`), optionale UDS/DoIP deps |
| `Cargo.toml` (workspace) | `reqwest` Dependency |
| `config/opensovd-native-server.toml` | `[[backends]]` Sektion, Dokumentation |

---

## 6. Konfiguration

```toml
# Standard-konform: externe CDA-Instanzen via HTTP
[[backends]]
name = "CDA Legacy ECUs"
base_url = "http://cda:20002"
api_prefix = "/sovd/v1"
component_ids = ["brake-ecu", "eps-ecu", "bms-ecu"]

# Standalone: lokale UDS/DoIP-Kommunikation (feature "local-uds")
[[components]]
sovd_component_id = "hpc-main"
doip_target_address = 1
```

---

## 7. Zusammenfassung

| Aspekt | Status | Ergebnis |
|--------|--------|----------|
| REST API (native-sovd) | ✅ korrekt | Unverändert, nutzt jetzt `state.backend` |
| FaultManager, LockManager, DiagLog | ✅ korrekt | FaultBridge verbindet fault-lib Pattern |
| Health, SOME/IP | ✅ korrekt | Unverändert |
| Auth Middleware | ✅ korrekt | Unverändert |
| **ComponentBackend Trait** | ✅ **implementiert** | `native-interfaces/src/backend.rs` |
| **ComponentRouter (Gateway)** | ✅ **implementiert** | `native-core/src/router.rs` |
| **SovdHttpBackend** | ✅ **implementiert** | `native-core/src/http_backend.rs` |
| **LocalUdsBackend** | ✅ **implementiert** | `native-core/src/local_backend.rs` (feature-gated) |
| **FaultBridge** | ✅ **implementiert** | `native-core/src/fault_bridge.rs` |
| **native-comm-uds/doip** | ✅ **feature-gated** | Nur mit `--features local-uds` |
| **Tests** | ✅ **75 Tests bestanden** | 47 native-core + 28 native-sovd |

**Kernaussage:** Der Native Server ist jetzt standard-konform aufgebaut. Die UDS/DoIP-Logik (`SovdTranslator`, `native-comm-uds`, `native-comm-doip`) ist hinter dem `local-uds` Feature-Gate isoliert. Im Standard-Modus kommuniziert der Server über `SovdHttpBackend` mit externen CDA-Instanzen — genau wie es die OpenSOVD-Architektur vorsieht.

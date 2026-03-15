# CDA — Implementierungsstand vs. SOVD-Standard

**Quelle:** [eclipse-opensovd/classic-diagnostic-adapter](https://github.com/eclipse-opensovd/classic-diagnostic-adapter) (main, Stand März 2026)

---

## 1. Workspace-Architektur (12 Crates)

| Crate | Funktion |
|-------|----------|
| `cda-database` | MDD/ODX-Datenbank: FlatBuffers + Protobuf für Diagnostic Descriptions |
| `cda-interfaces` | Shared Types: `UdsEcu` Trait, `DiagService`, `ComParam`, `EcuGateway`, `FileManager`, `SchemaProvider` |
| `cda-core` | Diagnostic Kernel (`diag_kernel/`) |
| `cda-comm-doip` | DoIP-Kommunikation (doip-codec 2.0.8 + doip-definitions 3.0.13) |
| `cda-comm-uds` | UDS-Kommunikation |
| `cda-sovd` | SOVD REST API (axum + aide für OpenAPI) |
| `cda-sovd-interfaces` | SOVD API-Typen: Components, Functions, Locking, Apps, Error |
| `cda-main` | Binary: figment Config, Runtime-Init |
| `cda-tracing` | Observability: OpenTelemetry OTLP + AUTOSAR DLT + tokio-console |
| `cda-plugin-security` | Security-Plugin-System: JWT Claims, RBAC, per-Service-Autorisierung |
| `cda-health` | Health Monitoring |
| `cda-build` | Build-Utilities |

**API-Basis-Pfad:** `/vehicle/v15/` (SOVD v1.5 Alignment)

---

## 2. SOVD-Standard Compliance

### 2.1 Implementierte Standard-Features

| SOVD-§ | Feature | CDA-Implementierung | Details |
|--------|---------|---------------------|---------|
| §5 | Discovery | ✅ | `/vehicle/v15`, `/vehicle/v15/components` |
| §7.2 | Components | ✅ | `GET/POST/PUT /components/{ecu}` mit Variant-Info, ECU-State (Online/Offline/NotTested/Duplicate/Disconnected), SDGs |
| §7.3 | Capabilities | ✅ Teilweise | Via `include-schema` Query-Param — JSON Schema neben Daten zurückgegeben |
| §7.4 | Locking | ✅ | Component-Level **und** Vehicle-Level Locks mit Expiration (`lock_expiration` in Sekunden) |
| §7.5 | Data | ✅ | `GET/PUT /data/{diag_service}` — DID-basiert aus MDD |
| §7.6 | Faults | ✅ | `GET/DELETE /faults`, `GET/DELETE /faults/{id}` |
| §7.7 | Operations | ✅ | `POST/GET /operations/{service}/executions` mit Lifecycle (GET/DELETE/PUT pro Execution) |
| §7.8 | Modes | ✅ | 4 Sub-Ressourcen: `/modes/session`, `/modes/security`, `/modes/commctrl`, `/modes/dtcsetting` |
| §7.9 | Configurations | ✅ | `GET /configurations`, `GET/PUT /configurations/{diag_service}` |
| §7.10 | Logs | ❌ | Nicht implementiert |
| §7.11 | Events/SSE | ❌ | Kein SSE/Event-Streaming |
| §7.12 | Proximity | ❌ | Kein Proximity Challenge |
| §7.23 | Authorization | ✅ | `POST /vehicle/v15/authorize` + Security Plugin Middleware |
| §7.x | Functions/Groups | ✅ | `/vehicle/v15/functions/{group}/...` — Functional Groups aus Config |
| §7.x | Apps | ✅ | `/vehicle/v15/apps/sovd2uds/...` — SOVD-App-Konzept |

### 2.2 Nicht implementierte Standard-Features

| SOVD-§ | Feature | Bemerkung |
|--------|---------|-----------|
| §7.10 | Diagnostic Logs | Kein Log-Buffer oder Log-Endpunkt |
| §7.11 | Events/SSE | Kein Server-Sent-Events für Fault-Subscription |
| §7.12 | Proximity Challenge | Kein physischer Proximity-Nachweis |
| §5 | OData Pagination | Kein `$top`/`$skip`/`$filter`/`$orderby` — CDA nutzt eigene Query-Params |
| §7.5.3 | Bulk Data (Standard) | Standard-Bulk nicht via SOVD-Spec, sondern via `x-sovd2uds-bulk-data` Extension |

---

## 3. Beyond-Standard: CDA-spezifische Erweiterungen

### 3.1 x-Extensions (Vendor-Extensions im SOVD-Sinne)

| Extension | Endpunkte | Beschreibung |
|-----------|-----------|-------------|
| **x-single-ecu-jobs** | `GET /x-single-ecu-jobs`, `GET /x-single-ecu-jobs/{job_name}` | Direkte Ausführung von Single-ECU Diagnostic Jobs aus MDD |
| **x-sovd2uds-download** | `PUT /requestdownload`, `POST/GET /flashtransfer`, `GET/DELETE /flashtransfer/{id}`, `PUT /transferexit` | Granularer UDS-Download/Flash-Workflow — RequestDownload (0x34), TransferData (0x36), TransferExit (0x37) als separate REST-Endpunkte mit Status-Tracking |
| **x-sovd2uds-bulk-data** | `GET /mdd-embedded-files`, `GET /mdd-embedded-files/{id}` | Zugriff auf in MDD eingebettete Dateien (Firmware-Images, Kalibrierdaten) |
| **x-include-sdgs** | Query-Param auf `/components` | Special Data Groups (SDGs) aus ODX — zusätzliche Metadaten pro ECU |

### 3.2 Architektur-Features über den Standard hinaus

| Feature | Beschreibung |
|---------|-------------|
| **MDD/ODX-Datenbank** | Komplette Diagnostic-Description-Verarbeitung: ODX → MDD (via odx-converter), FlatBuffers + Protobuf für schnellen Zugriff. Keine hartkodierten DIDs — alles aus der Datenbank |
| **Dynamisches Routing** | Routen werden dynamisch zur Laufzeit basierend auf den in MDD-Dateien konfigurierten ECUs generiert, nicht statisch definiert |
| **OpenAPI Auto-Generation** | `aide` Crate generiert automatisch OpenAPI/Swagger-Dokumentation für alle Endpunkte mit JSON Schema |
| **Security Plugin Architecture** | Vollständiges Plugin-System: `SecurityPlugin` Trait mit `AuthApi` (JWT Claims) + `SecurityApi` (per-DiagService Autorisierung). Unterstützt RBAC, ABAC, Custom-Plugins |
| **Vehicle-Level Locking** | Locks nicht nur auf Component-Ebene, sondern auch auf Vehicle-Ebene (`/vehicle/v15/locks`) |
| **ComParam Operations** | `/operations/comparam/executions` — Kommunikationsparameter als eigene Operations-Klasse |
| **Generic Service Passthrough** | `PUT /genericservice` — Direkter UDS-Service-Durchgriff ohne SOVD-Abstraktion |
| **Network Structure** | `/apps/sovd2uds/data/networkstructure` — Fahrzeug-Netzwerktopologie |
| **Flash File Management** | `/apps/sovd2uds/bulk-data/flashfiles` — Flash-Dateien-Verwaltung auf Server-Seite |
| **OpenTelemetry + DLT** | Production-Grade Observability: OTLP Export, AUTOSAR DLT Tracing, tokio-console Integration |
| **ECU Variant Detection** | Erkennung von ECU-Varianten mit Status-Management (Online, Offline, NotTested, Duplicate, Disconnected, NoVariantDetected) |
| **JSON Schema Inline** | `include-schema` Query-Parameter gibt JSON Schema direkt neben den Daten zurück |
| **Content-Type Negotiation** | `application/json` und `application/octet-stream` für Payload-Handling |
| **cargo-deny** | Lizenz- und Advisory-Prüfung via `deny.toml` |
| **REUSE Compliance** | `REUSE.toml` für FSFE REUSE Standard |
| **mockall** | Dependency für umfassende Unit-Tests mit Mocks |

---

## 4. Technologie-Vergleich CDA vs. Native

| Aspekt | CDA | Native (unser Projekt) |
|--------|-----|----------------------|
| **Rust Edition** | 2024 | 2021 |
| **SOVD API Version** | v1.5 (`/vehicle/v15/`) | v1.1 (`/sovd/v1/`) |
| **Diagnostic Description** | MDD/ODX-Datenbank (FlatBuffers + Protobuf) | TOML-Config (hartcodierte DIDs) |
| **Routing** | Dynamisch (zur Laufzeit aus MDD generiert) | Statisch (compile-time) |
| **OpenAPI** | ✅ aide Crate (automatisch) | ❌ Keine OpenAPI-Generierung |
| **Auth** | Plugin-System (JWT + RBAC + per-Service) | API-Key + JWT Bearer (statisch) |
| **Locking** | Component + Vehicle Level, mit Expiration | Component Level |
| **Modes** | 4 Sub-Ressourcen (Session, Security, CommCtrl, DTCSetting) | Einzelner Mode-Endpunkt |
| **Flash/OTA** | x-sovd2uds-download (granular, 4 Endpunkte) | Monolithisch (1 Endpunkt, OtaFlashOrchestrator) |
| **Logs** | ❌ | ✅ Ring-Buffer |
| **SSE** | ❌ | ✅ Fault-Subscription |
| **Proximity** | ❌ | ✅ Proximity Challenge |
| **Bulk Data** | x-sovd2uds-bulk-data (MDD-Dateien) | Standard POST bulk-read/bulk-write |
| **Tracing** | OpenTelemetry + DLT + tokio-console | tracing (basic) |
| **Pagination** | Eigene Query-Params | OData $top/$skip |
| **SOME/IP** | ❌ | ✅ vSomeIP FFI |
| **Health** | ✅ cda-health | ✅ native-health (CPU, Memory, Uptime) |
| **Fault Persistence** | In-Memory (DashMap vermutet) | sled oder DashMap (Feature-Gate) |
| **Test Framework** | mockall + Integration Tests Crate | axum::test + Unit Tests |

---

## 5. Zusammenfassung

### CDA-Stärken (gegenüber SOVD-Standard und Native)

1. **MDD/ODX-Integration** — Der größte Differentiator: CDA liest Diagnostic Descriptions aus standardisierten ODX-Dateien (ISO 22901-1), statt DIDs hartzukodieren. Das macht ihn für **reale Fahrzeugdiagnose** produktionsreif.

2. **Security Plugin Architecture** — Erweiterbar und sicher: JWT mit Claims-basierter Autorisierung auf DiagService-Ebene (RBAC/ABAC).

3. **x-sovd2uds-download** — Granularer Flash-Workflow als separate REST-Endpunkte mit Status-Tracking, anstatt monolithischem Flash-Aufruf.

4. **OpenAPI Auto-Generation** — Jeder Endpunkt hat automatisch generierte Swagger-Docs.

5. **Vehicle-Level Locking** — Fahrzeugweite Sperren zusätzlich zu Component-Locks.

6. **Production-Grade Observability** — OpenTelemetry OTLP + AUTOSAR DLT.

### CDA-Lücken (gegenüber SOVD-Standard)

1. **§7.10 Logs** — Kein Diagnostic Log Buffer.
2. **§7.11 Events/SSE** — Kein Server-Sent Events.
3. **§7.12 Proximity** — Kein Proximity Challenge.
4. **OData Pagination** — Keine `$top`/`$skip`-Unterstützung.

### Native-Stärken (gegenüber CDA)

1. **Logs/SSE/Proximity** — Alle drei im SOVD-Standard gefordert, im CDA nicht implementiert.
2. **SOME/IP** — vSomeIP FFI-Integration für Adaptive AUTOSAR.
3. **OData Pagination** — Standard-konforme Query-Parameter.
4. **Fault Persistence** — sled Backend für Crash-sichere DTC-Speicherung.
5. **Einfacherer Einstieg** — TOML-Config statt MDD/ODX-Toolchain.

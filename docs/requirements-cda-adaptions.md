# Requirements: CDA-Adaptionen für OpenSOVD-native-server

**Herkunft:** Analyse des [Eclipse OpenSOVD CDA](https://github.com/eclipse-opensovd/classic-diagnostic-adapter) — gefiltert auf Features, die für den OpenSOVD-native-server sinnvoll sind und den SOVD-Standard nicht konterkarieren.

**Ausschluss-Kriterien:** MDD/ODX-abhängige Features (x-single-ecu-jobs, x-sovd2uds-bulk-data, x-include-sdgs, ComParam Operations, Dynamic Routing) wurden bewusst ausgeschlossen, da Native auf TOML-Config basiert.

---

## REQ-CDA-01: Modes als Sub-Ressourcen (SOVD §7.8 Alignment)

**Priorität:** Hoch  
**Quelle:** CDA `cda-sovd/src/sovd/components/ecu/modes.rs`  
**SOVD-Referenz:** §7.8 — Modes sind als individuelle Sub-Ressourcen definiert

### Ist-Zustand (Native)

Einzelner Endpunkt `/components/{id}/mode` mit GET/POST — alle Modes (Session, Security, CommControl, DTCSetting) über einen generischen Endpunkt.

### Soll-Zustand

```
GET    /components/{id}/modes                → Liste aller verfügbaren Modes
GET    /components/{id}/modes/session        → Aktueller Session-Mode (UDS 0x10)
PUT    /components/{id}/modes/session        → Session-Mode setzen
GET    /components/{id}/modes/security       → Security-Access-Status (UDS 0x27)
PUT    /components/{id}/modes/security       → Security Access ausführen
GET    /components/{id}/modes/comm-control   → Communication Control (UDS 0x28)
PUT    /components/{id}/modes/comm-control   → Communication Control setzen
GET    /components/{id}/modes/dtc-setting    → DTC Setting Status (UDS 0x85)
PUT    /components/{id}/modes/dtc-setting    → DTC Setting setzen
```

### Begründung

Der SOVD-Standard definiert Modes als eigenständige Sub-Ressourcen. Der CDA implementiert dies korrekt. Der aktuelle Native-Ansatz mit einem einzelnen `/mode`-Endpunkt widerspricht dem Standard-Ressourcenmodell. Die Aufteilung erhöht außerdem die Sicherheit: verschiedene Modes können unterschiedlich autorisiert werden.

### Abwärtskompatibilität

Der bestehende `/mode` Endpunkt kann als Deprecated beibehalten und intern auf `/modes/session` umleiten.

---

## REQ-CDA-02: Vehicle-Level Locking (SOVD §7.4)

**Priorität:** Hoch  
**Quelle:** CDA `cda-sovd/src/sovd/locks.rs` — `locks::vehicle::{post, get}`, `locks::vehicle::lock::{get, put, delete}`  
**SOVD-Referenz:** §7.4 — Locking auf Vehicle-Ebene

### Ist-Zustand (Native)

Locks nur auf Component-Ebene (`/components/{id}/lock`).

### Soll-Zustand

```
POST   /locks                → Vehicle-Level Lock erstellen (mit Expiration)
GET    /locks                → Alle aktiven Vehicle-Level Locks auflisten
GET    /locks/{lock_id}      → Einzelnen Lock abfragen
PUT    /locks/{lock_id}      → Lock verlängern
DELETE /locks/{lock_id}      → Lock freigeben
```

### Datenmodell

```rust
pub struct SovdVehicleLock {
    pub id: String,
    pub locked_by: String,
    pub lock_expiration: chrono::DateTime<chrono::Utc>,
    pub owned: bool,  // true wenn aktueller Client der Besitzer ist
}
```

### Begründung

Der SOVD-Standard definiert Locking auf Vehicle- und Component-Ebene. Vehicle-Level Locks sperren alle Komponenten gleichzeitig — kritisch für OTA-Szenarien, in denen kein anderer Client während eines Flash-Vorgangs auf ECUs zugreifen darf. Der CDA implementiert dies; Native sollte nachziehen.

### Constraint

Ein Vehicle-Level Lock muss alle Component-Level Locks blockieren. Bestehende Component-Locks müssen bei Vehicle-Lock-Erstellung geprüft und ggf. abgelehnt werden.

---

## REQ-CDA-03: Authorize-Endpunkt (SOVD §7.23)

**Priorität:** Hoch  
**Quelle:** CDA `cda-sovd/src/sovd/mod.rs` — `POST /vehicle/v15/authorize`  
**SOVD-Referenz:** §7.23 — Authorization

### Ist-Zustand (Native)

Auth via Middleware (API-Key / JWT Bearer). Kein dedizierter `/authorize`-Endpunkt.

### Soll-Zustand

```
POST   /authorize            → Credentials übergeben, Token erhalten
```

### Request

```json
{
  "grant_type": "client_credentials",
  "client_id": "diagnostic-tool-1",
  "client_secret": "..."
}
```

### Response

```json
{
  "access_token": "eyJ...",
  "token_type": "Bearer",
  "expires_in": 3600
}
```

### Begründung

Der SOVD-Standard definiert einen expliziten Authorize-Endpunkt. Dies ermöglicht Clients, sich programmatisch zu authentifizieren, ohne externe OAuth-Server vorauszusetzen. Der bestehende API-Key/JWT-Mechanismus in der Middleware bleibt bestehen — der Authorize-Endpunkt ergänzt ihn als Standard-konformer Token-Ausgabepunkt.

---

## REQ-CDA-04: OpenAPI-Generierung

**Priorität:** Mittel  
**Quelle:** CDA nutzt `aide` Crate für automatische OpenAPI-Docs

### Ist-Zustand (Native)

Keine OpenAPI/Swagger-Dokumentation.

### Soll-Zustand

- Automatische OpenAPI 3.1 Spec-Generierung für alle SOVD-Endpunkte
- Endpunkt `GET /openapi.json` liefert aktuelle Spec
- Optional: Swagger UI unter `/docs`

### Technologie-Empfehlung

`utoipa` Crate (bevorzugt gegenüber `aide` wegen besserer axum-Integration und Wartungsstand) mit `utoipa-swagger-ui` für Swagger UI.

### Begründung

OpenAPI-Docs erleichtern Client-Entwicklung, Testing und Integration erheblich. Für ein Open-Source-Projekt ist eine maschinenlesbare API-Beschreibung essenziell. Der SOVD-Standard selbst basiert auf einer OpenAPI-Spezifikation.

---

## REQ-CDA-05: JSON Schema Inline (`include-schema`)

**Priorität:** Mittel  
**Quelle:** CDA `cda-sovd-interfaces/src/lib.rs` — `IncludeSchemaQuery`  
**SOVD-Referenz:** §7.3 — Capabilities / Self-Description

### Ist-Zustand (Native)

Kein Mechanismus zur Laufzeit-Schema-Auslieferung.

### Soll-Zustand

Query-Parameter `?include-schema=true` auf Daten- und Konfigurations-Endpunkten. Wenn gesetzt, enthält die Response ein zusätzliches `schema`-Feld mit dem JSON Schema der Datenstruktur.

```json
{
  "items": [...],
  "schema": {
    "type": "object",
    "properties": { ... }
  }
}
```

### Begründung

Ermöglicht generischen SOVD-Clients, UI-Formulare dynamisch zu generieren, ohne die Datenstruktur im Voraus zu kennen. Standard-konform und ergänzt `GET /capabilities`.

---

## REQ-CDA-06: Granularer Flash-Workflow (x-Extension)

**Priorität:** Mittel  
**Quelle:** CDA `x-sovd2uds-download` — 4 Endpunkte mit Status-Tracking

### Ist-Zustand (Native)

Monolithischer `POST /components/{id}/flash` mit `OtaFlashOrchestrator` — gesamter UDS-Flash-Workflow in einem Request.

### Soll-Zustand (als x-Extension, zusätzlich zum bestehenden Endpunkt)

```
GET    /components/{id}/x-flash                         → Flash-Fähigkeiten und Status
PUT    /components/{id}/x-flash/request-download        → UDS RequestDownload (0x34)
POST   /components/{id}/x-flash/transfer                → UDS TransferData (0x36), gibt transfer_id zurück
GET    /components/{id}/x-flash/transfer/{id}            → Transfer-Status abfragen
DELETE /components/{id}/x-flash/transfer/{id}            → Transfer abbrechen
PUT    /components/{id}/x-flash/transfer-exit            → UDS RequestTransferExit (0x37)
```

### Begründung

Der monolithische Flash-Endpunkt funktioniert für einfache Szenarien, bietet aber kein Status-Feedback während des Transfers und keine Möglichkeit, den Vorgang zu unterbrechen. Der granulare Ansatz des CDA erlaubt:
- **Progress-Tracking** pro Transfer-Block
- **Abbruch** eines laufenden Transfers
- **Retry** einzelner Schritte bei Fehler
- **Client-gesteuerte Blockgröße**

Der bestehende `POST /flash` Endpunkt bleibt als Convenience-Wrapper bestehen.

### Constraint

Alle x-Extensions müssen mit dem `x-`-Prefix versehen sein, um Standard-Namespace-Konflikte zu vermeiden (SOVD erlaubt Vendor-Extensions mit `x-`-Prefix explizit).

---

## REQ-CDA-07: Security Plugin Architecture

**Priorität:** Mittel  
**Quelle:** CDA `cda-plugin-security/src/lib.rs` — `SecurityPlugin`, `SecurityApi`, `AuthApi` Traits

### Ist-Zustand (Native)

Statische Auth-Middleware mit API-Key und JWT. Keine per-Endpunkt- oder per-Service-Autorisierung.

### Soll-Zustand

Trait-basiertes Security-Plugin-System:

```rust
pub trait SecurityPlugin: Send + Sync {
    /// Authentifizierung: Claims aus Request extrahieren
    fn authenticate(&self, headers: &HeaderMap) -> Result<Box<dyn Claims>, AuthError>;

    /// Autorisierung: Zugriff auf spezifischen Endpunkt/Service prüfen
    fn authorize(&self, claims: &dyn Claims, resource: &str, action: &str) -> Result<(), AuthError>;
}

pub trait Claims: Send + Sync {
    fn subject(&self) -> &str;
    fn roles(&self) -> &[String];
}
```

### Begründung

Die aktuelle Auth-Middleware ist binär (erlaubt/verweigert). Es gibt keine Möglichkeit, bestimmte Endpunkte für bestimmte Rollen zu sperren (z.B. Flash nur für `admin`, Data-Read für `viewer`). Ein Plugin-System ermöglicht:
- **RBAC**: Rollenbasierte Zugriffskontrolle
- **Per-Service-Autorisierung**: Flash vs. Read-Only unterschiedlich behandeln
- **Custom-Plugins**: OEM-spezifische Auth-Logik ohne Core-Änderung

### Migration

Die bestehende `AuthConfig` wird als Default-Plugin (`StaticAuthPlugin`) gewrapped. Kein Breaking Change.

---

## REQ-CDA-08: OpenTelemetry Observability

**Priorität:** Niedrig  
**Quelle:** CDA `cda-tracing/src/otel.rs` — OTLP Export + DLT

### Ist-Zustand (Native)

Basis-Tracing via `tracing` Crate (Konsolen-Output).

### Soll-Zustand

- OpenTelemetry OTLP Export (Traces + Metrics) — optional via Config aktivierbar
- Strukturierte Trace-IDs in allen Diagnose-Operationen
- Feature-Gate `otel` um Dependency optional zu halten

### Konfiguration

```toml
[telemetry]
enabled = false
otlp_endpoint = "http://localhost:4317"
service_name = "opensovd-native-server"
```

### Begründung

Für Production-Deployments auf HPC-ECUs ist verteiltes Tracing essenziell zur Fehleranalyse. OpenTelemetry ist der Industrie-Standard und integriert mit Jaeger, Grafana Tempo, Datadog etc. Als Feature-Gate hat es keinen Impact auf Binär-Größe wenn deaktiviert.

---

## REQ-CDA-09: Content-Type Negotiation (JSON + Octet-Stream)

**Priorität:** Niedrig  
**Quelle:** CDA `cda-sovd/src/sovd/mod.rs` — `get_payload_data`, `get_octet_stream_payload`

### Ist-Zustand (Native)

Nur `application/json` für alle Endpunkte. Flash-Firmware wird als Base64 in JSON kodiert.

### Soll-Zustand

- `application/json` für strukturierte Daten (Default)
- `application/octet-stream` für binäre Payloads (Flash-Firmware, Memory-Dumps)
- Content-Type via `Accept`/`Content-Type` Header gesteuert

### Begründung

Base64-Kodierung von Firmware-Dateien erhöht die Payload-Größe um ~33%. Für OTA-Flash-Szenarien mit mehreren MB Firmware ist `application/octet-stream` deutlich effizienter. Außerdem ist dies Standard-HTTP-Praxis für binäre Daten.

---

## REQ-CDA-10: ECU-State-Erweiterung

**Priorität:** Niedrig  
**Quelle:** CDA `cda-sovd-interfaces/src/components/ecu/mod.rs` — `State` Enum

### Ist-Zustand (Native)

```rust
pub enum SovdConnectionState {
    Available,
    Busy,
    Unavailable,
}
```

### Soll-Zustand

```rust
pub enum SovdConnectionState {
    Available,          // Erreichbar, bereit
    Busy,               // Verbunden, in Benutzung
    Unavailable,        // Nicht erreichbar
    NotTested,          // Noch nicht geprüft (nach Startup)
    Disconnected,       // War verbunden, Verbindung verloren
}
```

### Begründung

`NotTested` und `Disconnected` bilden reale Zustände ab, die im Fahrzeug-Lifecycle auftreten (z.B. nach Server-Neustart sind ECUs noch nicht geprüft; nach Netzwerkunterbrechung ist eine ECU disconnected aber potentiell wieder erreichbar). Dies verbessert die Diagnose-UX erheblich. Die neuen Werte sind rückwärtskompatibel, da bestehende Clients unbekannte Werte ignorieren können.

---

## REQ-CDA-11: Network Structure Endpunkt (x-Extension)

**Priorität:** Niedrig  
**Quelle:** CDA `apps::sovd2uds::data::networkstructure`

### Ist-Zustand (Native)

`NetworkStructure` Typ existiert bereits in `native-interfaces/src/sovd.rs` (Gateway, NetworkEcu), aber kein dedizierter Endpunkt.

### Soll-Zustand

```
GET    /x-network-structure    → Fahrzeug-Netzwerktopologie
```

### Response

```json
{
  "gateways": [
    {
      "name": "Central Gateway",
      "network_address": "192.168.1.1",
      "logical_address": "0x0001",
      "ecus": [
        { "qualifier": "HPC-Main", "logical_address": "0x0E00" }
      ]
    }
  ]
}
```

### Begründung

Die Datenstruktur ist bereits implementiert. Ein Endpunkt macht sie für Clients zugänglich und ermöglicht Topologie-Visualisierung in Diagnose-Tools. Als `x-`-Extension ist kein Standard-Konflikt möglich.

---

## REQ-CDA-12: cargo-deny für CI/CD

**Priorität:** Niedrig  
**Quelle:** CDA `deny.toml`

### Ist-Zustand (Native)

Manuelle Lizenz-Prüfung (in OSR dokumentiert).

### Soll-Zustand

- `deny.toml` mit Lizenz-Allowlist und Advisory-DB-Check
- CI-Pipeline-Schritt: `cargo deny check`

### Begründung

Automatisierte Prüfung statt manueller Review bei jedem Dependency-Update. Fängt inkompatible Lizenzen und bekannte Sicherheitslücken automatisch ab.

---

## Zusammenfassung

| REQ | Feature | Prio | Standard-Konformität |
|-----|---------|------|---------------------|
| CDA-01 | Modes als Sub-Ressourcen | Hoch | §7.8 verlangt Sub-Ressourcen |
| CDA-02 | Vehicle-Level Locking | Hoch | §7.4 definiert Vehicle-Locks |
| CDA-03 | Authorize-Endpunkt | Hoch | §7.23 definiert `/authorize` |
| CDA-04 | OpenAPI-Generierung | Mittel | Standard basiert auf OpenAPI |
| CDA-05 | JSON Schema Inline | Mittel | §7.3 Self-Description |
| CDA-06 | Granularer Flash (x-Ext) | Mittel | x-Extension, kein Konflikt |
| CDA-07 | Security Plugin Arch | Mittel | Ergänzt §7.23 |
| CDA-08 | OpenTelemetry | Niedrig | Orthogonal zum Standard |
| CDA-09 | Content-Type Negotiation | Niedrig | Standard-HTTP-Praxis |
| CDA-10 | ECU-State-Erweiterung | Niedrig | Rückwärtskompatibel |
| CDA-11 | Network Structure (x-Ext) | Niedrig | x-Extension, kein Konflikt |
| CDA-12 | cargo-deny CI/CD | Niedrig | Tooling, kein Code-Impact |

### Ausgeschlossene CDA-Features

| Feature | Ausschluss-Grund |
|---------|-----------------|
| MDD/ODX-Datenbank | Erfordert komplette Toolchain-Änderung; Native nutzt TOML-Config |
| Dynamic Routing | Architektonisch an MDD gekoppelt |
| x-single-ecu-jobs | MDD-spezifisches Konzept |
| x-sovd2uds-bulk-data (MDD Files) | MDD-spezifisch; Native hat Standard-Bulk-Read/Write |
| x-include-sdgs | ODX Special Data Groups — ohne ODX nicht anwendbar |
| ComParam Operations | Kommunikationsparameter aus MDD — nicht anwendbar |
| REUSE.toml | SPDX-Header bereits vorhanden; REUSE ist optionale Zusatzschicht |

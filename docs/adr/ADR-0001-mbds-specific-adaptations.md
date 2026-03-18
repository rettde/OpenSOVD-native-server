# ADR-0001: Mercedes-Benz–spezifische Anpassungen (MBDS S-SOVD) — Refactoring-Plan

**Status:** Accepted  
**Datum:** 2026-03-18  
**Autor:** Cascade / AI-Pair (reviewed by Maintainer)  
**Kontext:** MBDS S-SOVD Compliance Audit Rev. 1–3

---

## 1. Entscheidungskontext

Das OpenSOVD-native-server–Projekt implementiert die ISO 17978-3 / ASAM SOVD V1.1.0–Spezifikation.
Während des MBDS-Konformitätsaudits wurden **15 Code-Änderungen** durchgeführt, die über den
reinen ISO/ASAM-Standard hinausgehen und Mercedes-spezifische Anforderungen aus dem
Dokument **MBDS_S-SOVD_2024-07** umsetzen.

Dieses ADR katalogisiert **jede einzelne Mercedes-spezifische Anpassung**, klassifiziert sie
und schlägt eine Refactoring-Strategie vor, um MBDS-Logik sauber vom generischen
SOVD-Standard-Code zu trennen.

---

## 2. Katalog der MBDS-spezifischen Anpassungen

### Kategorie A — Standard-Abweichungen (MBDS überschreibt ISO/ASAM)

Hier weicht MBDS **explizit** vom Standard ab. Ohne diese Änderungen wäre der Server
standardkonform, aber nicht MBDS-konform.

| # | Änderung | MBDS-Referenz | Datei(en) | Auswirkung |
|---|----------|---------------|-----------|------------|
| A1 | **`/areas` Endpoint verboten und entfernt** | §2.2 | `routes.rs`, `openapi.rs` | ISO 17978-3 definiert Area als gültigen Entity-Typ; MBDS verbietet ihn explizit. Route, Handler, CDF-Pfad und Test gelöscht. |
| A2 | **Ungültiges JWT-Token → HTTP 403 statt 401** | §6.3 | `auth.rs:301-308, 462, 476, 518` | RFC 9110 / OAuth2 sieht 401 für ungültige Tokens vor. MBDS verlangt 403 (`SOVD-ERR-403`). Betrifft 7 Stellen in `validate_jwt()` und `validate_oidc_jwt()`. |
| A3 | **Scope-Ceiling: Max `After_Sales_ENHANCED`** | §6.2 | `auth.rs:71-76, 145-160` | Standard-SOVD kennt kein Scope-Ceiling. MBDS definiert eine fixe Scope-Hierarchie (`After_Sales_BASIC`, `After_Sales_ENHANCED`). Hardcoded als `default_allowed_scopes()`. |

### Kategorie B — MBDS-spezifische Zusatzanforderungen (Standard-Erweiterung)

MBDS fordert Features, die der Standard nicht verlangt, ihm aber nicht widersprechen.
Ein generischer SOVD-Server bräuchte sie nicht.

| # | Änderung | MBDS-Referenz | Datei(en) | Auswirkung |
|---|----------|---------------|-----------|------------|
| B1 | **VIN-gebundenes Token** | §6.2 | `auth.rs:58-60, 121-123, 130-144` | JWT muss `vin`-Claim enthalten, geprüft gegen `AuthConfig.required_vin`. `enforce_claims()` → 403 bei Mismatch. Standard-SOVD kennt keinen VIN-Claim. |
| B2 | **DDAG Entity-ID Validierung** | §2.3 | `routes.rs:28-47, 1847-1870` | Entity-IDs müssen 1-64 Zeichen, `[a-zA-Z0-9_-]`, kein führender/abschließender Hyphen sein. `entity_id_validation_middleware()` + `validate_entity_id()`. Standard-SOVD definiert keine Entity-ID-Syntax. |
| B3 | **mTLS (Client-Zertifikat-Pflicht)** | §6.3 | `main.rs:68-72, 219-222, 251-290` | `client_ca_path` in `ServerConfig`, `build_mtls_config()` mit `WebPkiClientVerifier`. Standard-SOVD empfiehlt TLS, erzwingt aber kein mTLS. |
| B4 | **DLT-Anbindung (AUTOSAR DLT Daemon)** | §8 | `dlt.rs` (NEU, 197 Zeilen), `main.rs:122-131` | `DltLayer` tracing-subscriber mit Unix-Socket-Forwarding an DLTDaemon. AUTOSAR-spezifisches Logging-Format. Standard-SOVD definiert kein Log-Format. |
| B5 | **Trace-ID Propagierung (W3C Trace Context)** | §8 | `routes.rs:49-67` | `trace_id_middleware()` liest/generiert `traceparent`-Header. MBDS fordert durchgängige Trace-IDs über Proxy-Kette. Standard-SOVD erwähnt es nicht. |
| B6 | **mDNS/DNS-SD Discovery** | §4.2 | `mdns.rs` (NEU, 126 Zeilen), `main.rs:212-213` | `MdnsHandle::register()` via `mdns-sd` Crate, `_sovd._tcp.local.`. MBDS fordert lokale Discovery; ISO 17978-3 §4.2 erwähnt es als Option. |
| B7 | **Bulk-Data Kategorien** | §7.5.3 | `sovd.rs:377-394`, `backend.rs:140-145` | `SovdBulkDataCategory` Enum (`currentData`, `logs`, `trigger`), `category`-Feld in `SovdBulkReadRequest` + Backend-Trait. Standard definiert nur Bulk-Data ohne Kategorie-Filter. |
| B8 | **`scope`-Feld in `SovdFault`** | §7.1 | `sovd.rs:74-76`, `fault_bridge.rs:137` | Neues optionales Feld `scope: Option<String>` in `SovdFault`. MBDS fordert Fault-Scope (component/system/network). Standard-SOVD hat kein `scope`-Feld. |

### Kategorie C — MBDS-getriebene CDF-Extensions (OpenAPI-Annotationen)

MBDS verlangt bestimmte `x-sovd-*`-Extensions in der Capability Description File.
Diese sind teilweise auch im ASAM-Standard erwähnt, werden aber von MBDS **erzwungen**.

| # | Änderung | MBDS-Referenz | Datei | Auswirkung |
|---|----------|---------------|-------|------------|
| C1 | **`x-sovd-applicability` (offline)** | §5.6 | `openapi.rs:48-51` | `{"online": true, "offline": true}` im `info`-Block. Statisch hardcoded. |
| C2 | **`x-sovd-unit` auf Data-Pfaden** | §5.4 | `openapi.rs:400` | `"x-sovd-unit": "raw"` — statisch, nicht aus Data Catalog abgeleitet. |
| C3 | **`x-sovd-proximity-proof-required`** | §5.4 | `openapi.rs:486` | `false` auf Operations-Pfad. Statisch, nicht konfigurierbar. |
| C4 | **`/version-info` Endpoint** | §4.1 | `routes.rs:555-568` | `GET /version-info` mit `sovdVersion`, `apiVersions`. MBDS erzwingt diesen Endpunkt, der im CDF per §5.6 verboten ist (daher nur in routes, nicht in OpenAPI-Spec). |

---

## 3. Abhängigkeitsmatrix — Betroffene Crates und Dateien

```
                     ┌─────────────────────────────────────────┐
                     │         MBDS-spezifischer Code           │
                     └───────────┬─────────────┬───────────────┘
                                 │             │
              ┌──────────────────┤             ├──────────────────┐
              ▼                  ▼             ▼                  ▼
     native-sovd/         native-server/   native-interfaces/  native-core/
     ├─ auth.rs            ├─ main.rs       ├─ sovd.rs          ├─ fault_bridge.rs
     │  A2 A3 B1           │  B3 B4 B6      │  B7 B8            │  B8
     ├─ routes.rs          │                ├─ backend.rs       └─ http_backend.rs
     │  A1 B2 B5 C4        │                │  B7                  B7
     ├─ openapi.rs         │                │
     │  A1 C1 C2 C3        │                │
     ├─ dlt.rs (NEU)       │                │
     │  B4                  │                │
     └─ mdns.rs (NEU)      │                │
        B6                  │                │
```

### Neue Abhängigkeiten (nur MBDS-bedingt)

| Crate | Dependency | Grund |
|-------|-----------|-------|
| `native-sovd` | `mdns-sd` | B6 — mDNS/DNS-SD |
| `native-sovd` | `tracing-subscriber` (registry) | B4 — DLT Layer |
| `native-server` | `rustls`, `rustls-pemfile` | B3 — mTLS |

---

## 4. Refactoring-Entscheidung

### 4.1 Ziel

Mercedes-spezifische Logik so isolieren, dass:
1. Der **generische SOVD-Server** ohne MBDS-Code kompilier- und nutzbar bleibt.
2. MBDS-Anpassungen als **Feature-Flag** oder **separates Modul** zuschaltbar sind.
3. Andere OEMs (BMW, VW, Stellantis) eigene Profile ohne Fork erstellen können.

### 4.2 Implementierter Ansatz: **OemProfile Trait-Hierarchie** (CDA-inspiriert)

Architektur-Vorbild: CDA's `SecurityPlugin: Any + SecurityApi + AuthApi` Pattern
aus `cda-plugin-security/src/lib.rs` — trait-basierte Plugin-Hierarchie mit
Middleware-Integration und Lifecycle-Management.

#### Trait-Hierarchie (`native-interfaces/src/oem.rs`)

```rust
/// Vier spezialisierte Sub-Traits für verschiedene Belange:
pub trait AuthPolicy: Send + Sync {
    fn invalid_token_status(&self) -> HttpStatusCode { 401 }
    fn invalid_token_error_code(&self) -> &'static str { "SOVD-ERR-401" }
    fn validate_claims(&self, claims: &HashMap<String, Value>, path: &str)
        -> Result<(), (HttpStatusCode, String, String)> { Ok(()) }
    fn allowed_scopes(&self) -> &[&str] { &[] }
}

pub trait EntityIdPolicy: Send + Sync {
    fn validate_entity_id(&self, id: &str) -> Result<(), String> { /* permissive */ }
}

pub trait DiscoveryPolicy: Send + Sync {
    fn areas_enabled(&self) -> bool { true }  // MBDS: false
}

pub trait CdfPolicy: Send + Sync {
    fn applicability(&self) -> CdfApplicability { /* online only */ }
    fn default_data_unit(&self) -> &'static str { "unspecified" }
    fn default_proximity_proof_required(&self) -> bool { false }
}

/// Haupt-Trait: kombiniert alle Sub-Traits (wie CDA's SecurityPlugin)
pub trait OemProfile: AuthPolicy + EntityIdPolicy + DiscoveryPolicy + CdfPolicy
    + Send + Sync
{
    fn name(&self) -> &'static str;
    fn id(&self) -> &'static str;
    fn as_auth_policy(&self) -> &dyn AuthPolicy;
    fn as_entity_id_policy(&self) -> &dyn EntityIdPolicy;
    fn as_discovery_policy(&self) -> &dyn DiscoveryPolicy;
    fn as_cdf_policy(&self) -> &dyn CdfPolicy;
}
```

#### Implementierte Profile

| Profil | Struct | Datei | Beschreibung |
|--------|--------|-------|-------------|
| **Standard** | `DefaultProfile` | `native-interfaces/src/oem.rs` | Permissiv, keine OEM-Einschränkungen |
| **MBDS** | `MbdsProfile` | `native-sovd/src/oem_mbds.rs` | Alle 15 MBDS-Anpassungen (A1–C4) |

#### Injection-Architektur

```
                  Arc<dyn OemProfile>
                         │
          ┌──────────────┼───────────────────┐
          ▼              ▼                   ▼
     AppState      AuthState          openapi.rs
     (routes)    (auth middleware)   (CDF builder)
          │              │                   │
    EntityIdPolicy  AuthPolicy          CdfPolicy
    (middleware)    (validate_claims)  (x-sovd-*)
```

- **`AppState.oem_profile`** — `Arc<dyn OemProfile>` im Shared State (Singleton, nicht per-Request)
- **`AuthState`** — bündelt `AuthConfig` + `Arc<dyn OemProfile>` für die Auth-Middleware
  (analog zu CDA's `SecurityPluginMiddleware`)
- **`entity_id_validation_middleware`** — erhält `Arc<dyn OemProfile>` via Closure-Capture
- **`build_openapi_json_with_policy`** — erhält `&dyn CdfPolicy` für `x-sovd-*` Werte

#### Entfernte MBDS-Hardcodes aus generischem Code

| Vorher (hardcoded) | Nachher (via OemProfile) |
|---------------------|--------------------------|
| `required_vin` in `AuthConfig` | `MbdsProfile::validate_claims()` |
| `allowed_scopes` in `AuthConfig` | `MbdsProfile::validate_claims()` |
| `default_allowed_scopes()` | `MbdsProfile::allowed_scopes()` |
| `StatusCode::FORBIDDEN` (7× in auth.rs) | `auth_policy.invalid_token_status()` |
| `validate_entity_id()` (freistehende Funktion) | `EntityIdPolicy::validate_entity_id()` via Middleware |
| `"x-sovd-unit": "raw"` in openapi.rs | `CdfPolicy::default_data_unit()` |
| `"x-sovd-applicability": {offline: true}` | `CdfPolicy::applicability()` |
| `"x-sovd-proximity-proof-required": false` | `CdfPolicy::default_proximity_proof_required()` |

**Beibehalten im generischen Code** (standardkompatibel, für alle OEMs nützlich):
- `SovdFault.scope` (B8), `SovdBulkDataCategory` (B7) — optionale Felder
- Trace-ID Middleware (B5), `/version-info` (C4) — generisch nützlich
- `dlt.rs` (B4), `mdns.rs` (B6), `build_mtls_config` (B3) — vorerst nicht feature-gated,
  da die Module über Config deaktivierbar sind (`DltConfig.enabled`, `MdnsConfig`, `client_ca_path`)

---

## 5. Übersichtsmatrix — Umsetzungsstatus

| # | Anpassung | Implementierung | Status |
|---|-----------|-----------------|--------|
| A1 | `/areas` entfernt | `DiscoveryPolicy::areas_enabled()` → `false` in MbdsProfile | ✅ |
| A2 | 401→403 | `AuthPolicy::invalid_token_status()` → `403` in MbdsProfile | ✅ |
| A3 | Scope-Ceiling | `AuthPolicy::validate_claims()` prüft `allowed_scopes` | ✅ |
| B1 | VIN-Claim | `AuthPolicy::validate_claims()` prüft `vin` Claim | ✅ |
| B2 | DDAG Entity-ID | `EntityIdPolicy::validate_entity_id()` in MbdsProfile | ✅ |
| B3 | mTLS | Config-gesteuert via `client_ca_path` | ✅ (Config) |
| B4 | DLT Layer | Config-gesteuert via `DltConfig.enabled` | ✅ (Config) |
| B5 | Trace-ID | Generisch im Standardcode | ✅ (generisch) |
| B6 | mDNS | Config-gesteuert via `MdnsConfig` | ✅ (Config) |
| B7 | Bulk-Data Category | Optionales Feld, generisch im Standardcode | ✅ (generisch) |
| B8 | Fault Scope | Optionales Feld, generisch im Standardcode | ✅ (generisch) |
| C1 | `x-sovd-applicability` | `CdfPolicy::applicability()` | ✅ |
| C2 | `x-sovd-unit` | `CdfPolicy::default_data_unit()` | ✅ |
| C3 | `x-sovd-proximity-proof-required` | `CdfPolicy::default_proximity_proof_required()` | ✅ |
| C4 | `/version-info` | Generisch im Standardcode | ✅ (generisch) |

---

## 6. Verifizierung

```
$ cargo clippy --workspace     → 0 errors, 0 warnings
$ cargo test --workspace       → 165 tests passed, 0 failed
```

Neue Tests in `oem_mbds.rs`:
- `mbds_profile_rejects_invalid_entity_ids` — DDAG-Regeln
- `mbds_profile_forbids_areas` — Area-Verbot
- `mbds_profile_returns_403_for_invalid_tokens` — Status-Code-Policy
- `mbds_profile_enforces_vin` — VIN-Binding
- `mbds_profile_enforces_scope_ceiling` — Scope-Enforcement
- `mbds_cdf_policy` — CDF-Extensionswerte
- `default_profile_is_permissive` — Baseline-Verifikation

---

## 7. Konsequenzen

### Positiv
- **OEM-Entkopplung:** Generischer SOVD-Server ohne Mercedes-Lock-in
- **Testbarkeit:** `DefaultProfile` in Tests, `MbdsProfile` in Produktion
- **Erweiterbarkeit:** Neue OEM-Profile (BMW, VW) als eigene `OemProfile`-Impls
- **Keine Feature-Flags nötig:** Profile werden zur Laufzeit via `AppState` injiziert
- **CDA-Konsistenz:** Gleiche Architektur wie CDA SecurityPlugin

### Negativ
- **Indirektion:** Trait-basierte Policies statt direkter if-Checks
- **HashMap-Konvertierung:** Claims werden für die Policy zu `HashMap<String, Value>` konvertiert

### Verbleibende offene Punkte (Phase 2)

| # | Punkt | Aufwand |
|---|-------|---------|
| 1 | `dlt.rs`, `mdns.rs`, `build_mtls_config` hinter `#[cfg(feature)]` für Dependency-Reduktion | S |
| 2 | Config-basierte Profilauswahl (`oem_profile: "mbds"` in TOML) statt hardcoded in `main.rs` | S |
| 3 | `DiscoveryPolicy::areas_enabled()` in Router verdrahten (aktuell: `/areas` bereits gelöscht) | S |

---

## 8. Entscheidung

**Implemented** — OemProfile Trait-Hierarchie (CDA-inspiriert) ist vollständig umgesetzt.

### 8.1 OEM-Profil-Isolation (Proprietary / nicht Open-Source)

Proprietäre OEM-Profile werden **automatisch anhand des Dateinamens erkannt** —
keine Cargo-Feature-Flags nötig:

| Aspekt | Umsetzung |
|--------|-----------|
| **Auto-Detection** | `build.rs` scannt `src/oem_*.rs` (außer `oem_sample.rs`) und setzt `cfg(has_oem_<name>)` |
| **Kein Feature-Flag** | Kein `--features mbds` o.ä. — Datei vorhanden = Profil aktiv |
| **`.gitignore`** | `native-sovd/src/oem_mbds.rs` ist in `.gitignore` gelistet |
| **Default-Profil** | Ohne OEM-Datei: `SampleOemProfile` (standard SOVD, open-source) |
| **OEM-Profil** | Mit OEM-Datei: automatisch kompiliert und als aktives Profil geladen |

```
# Open-Source Build (GitHub — oem_mbds.rs nicht vorhanden):
cargo build                    → SampleOemProfile (standard SOVD)
cargo test --workspace         → 161 Tests passed

# Proprietärer Build (lokal — oem_mbds.rs vorhanden):
cargo build                    → MbdsProfile (auto-detected via build.rs)
cargo test --workspace         → 168 Tests passed (+7 MBDS-Tests)
```

**Mechanismus:**
```
native-sovd/build.rs
  ├── scannt src/oem_*.rs
  ├── oem_mbds.rs gefunden → println!("cargo:rustc-cfg=has_oem_mbds")
  └── oem_sample.rs → ignoriert (immer kompiliert)

lib.rs:
  #[cfg(has_oem_mbds)]
  pub mod oem_mbds;

main.rs:
  #[cfg(has_oem_mbds)]    → Arc::new(MbdsProfile::default())
  #[cfg(not(has_oem_mbds))] → Arc::new(SampleOemProfile)
```

### 8.2 OEM-Sample als Template

`native-sovd/src/oem_sample.rs` dient als **dokumentierte Vorlage** für OEM-Profile:
- Alle 4 Sub-Traits (`AuthPolicy`, `EntityIdPolicy`, `DiscoveryPolicy`, `CdfPolicy`)
  mit ausführlichen Inline-Kommentaren und Beispielcode
- Zeigt alle möglichen Customization Points (VIN-Binding, Scope-Ceiling,
  Region-Restriction, Workshop-ID, Entity-ID-Format, CDF-Extensions, etc.)
- Schritt-für-Schritt-Anleitung im Header: Copy → Rename → Implement → Register

### 8.3 Betroffene Dateien

Open-Source (in GitHub):
- `native-interfaces/src/oem.rs` — OemProfile Trait + DefaultProfile
- `native-sovd/src/oem_sample.rs` — Generisches OEM-Template mit Dokumentation (NEU)
- `native-sovd/build.rs` — Auto-Detection von `oem_*.rs` Dateien (NEU)
- `native-server/build.rs` — Propagiert OEM-cfg-Flags zum Server-Binary (NEU)
- `native-sovd/src/state.rs` — `oem_profile: Arc<dyn OemProfile>` in AppState
- `native-sovd/src/auth.rs` — `AuthState` mit OemProfile, `enforce_claims` delegiert an AuthPolicy
- `native-sovd/src/routes.rs` — `entity_id_validation_middleware` nutzt EntityIdPolicy
- `native-sovd/src/openapi.rs` — `build_openapi_json_with_policy` nutzt CdfPolicy
- `native-server/src/main.rs` — Profil via `#[cfg(has_oem_*)]` automatisch gewählt

Proprietär (`.gitignore`d):
- `native-sovd/src/oem_mbds.rs` — MbdsProfile mit allen 15 MBDS-Anpassungen

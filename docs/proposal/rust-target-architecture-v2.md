# Rust-Zielarchitektur v2 — gateway-orientierter SOVD-Server

| Feld | Wert |
|------|------|
| **Status** | Architekturentwurf |
| **Datum** | 2026-03-24 |
| **Autor** | Rettstatt, mit AI-Unterstützung bei Analyse und Strukturierung |
| **Bezug** | Baut auf `architecture-intent-native-server.md` auf |

---

## 1. Zweck

Dieses Dokument übersetzt die Intentionsbeschreibung in eine **konkrete technische Zielarchitektur** für einen nativen OpenSOVD-Server in Rust.

Es beantwortet nicht die Frage „Wie portieren wir `opensovd-core` nach Rust?“, sondern die wichtigere Frage:

**Welche Architekturideen aus OpenSOVD und `opensovd-core` sind stark genug, um sie bewusst in eine Rust-native Serverarchitektur zu überführen?**

Das Dokument ist damit:

- **kein** Portierungsplan für `opensovd-core`,
- **kein** Vorschlag für eine enge technische Abhängigkeit,
- sondern ein Entwurf für eine **eigenständige Rust-Zielarchitektur**, die
  - den gateway-orientierten Zuschnitt des `native-server` erhält,
  - die stärkeren Konzepte aus `opensovd-core` gezielt übernimmt,
  - und Kern, Extensions und Plattformdienste sauber trennt.

---

## 2. Leitthesen

## 2.1 Der Server bleibt gateway-orientiert

Der `native-server` bleibt nach außen ein **SOVD-Server** und darf intern eine **Gateway-Schicht** enthalten. Das Gateway ist jedoch eine interne Routing-Verantwortung, nicht die Identität des öffentlichen SOVD-Vertrags.

## 2.2 Konzepte übernehmen, nicht Implementierungsbasis erzwingen

`opensovd-core` soll nicht das Runtime-Fundament des Rust-Servers werden. Übernommen werden sollen nur die Konzepte, die architektonisch klarer und nachhaltiger sind.

## 2.3 Kleine capability-orientierte Schnittstellen sind besser als ein großes Backend-Trait

Das heutige `ComponentBackend` ist für die Realität eines breiten Prototyps verständlich, langfristig aber zu breit. Die Zielarchitektur soll fachliche Fähigkeiten explizit und getrennt modellieren.

## 2.4 Die Topologie ist ein Kernobjekt

Discovery, Relationen und Sichtbarkeit von Entitäten sollen nicht implizit aus Backend-Listen abgeleitet werden. Es braucht eine explizite `Topology` als zentrales Modell.

## 2.5 Kern und Extensions müssen strukturell getrennt werden

Standard-SOVD, UDS-nahe Erweiterungen, Cloud-/Fleet-Funktionen und Betriebsfunktionen dürfen nicht mehr dieselbe architektonische Gewichtung haben.

## 2.6 Kein Big-Bang-Umbau

Die Überführung muss inkrementell möglich sein. Bestehende `ComponentBackend`-Implementierungen sollen zunächst über Adapter weiterverwendbar bleiben.

---

## 3. Zielbild auf hoher Ebene

```text
SOVD Client
   │
   ▼
HTTP/API Layer (native-sovd)
   - Core routes
   - Extension routes
   - OpenAPI/CDF composition
   - Error mapping
   │
   ▼
Security Layer
   - Authenticator
   - Authorizer
   - OEM policy hooks
   │
   ▼
Core Runtime (native-core)
   - Topology
   - CapabilityRegistry
   - GatewayResolver
   - DiscoveryRuntime
   - Local platform services
   │
   ├── Native SOVD backends
   ├── CDA adapters
   ├── Local diagnostic services
   └── Extension providers
```

Wichtig ist die Trennung:

- **HTTP/API Layer** beschreibt den öffentlichen Vertrag
- **Core Runtime** löst Entitäten, Fähigkeiten und Routing auf
- **Backends/Adapter** stellen konkrete Diagnosefähigkeiten bereit
- **Extensions** bleiben sichtbar getrennt

---

## 4. Zielzustand der Workspace-Rollen

## 4.1 `native-interfaces`

`native-interfaces` wird zur Heimat der **stabilen Domänen- und Vertragsgrenzen**.

### Inhalt

- Entitätsreferenzen und Entitätssammlungen
- capability-orientierte Traits
- Discovery-Deltas und Discovery-Schnittstellen
- generische AuthN-/AuthZ-Verträge
- Extension-Deskriptoren
- SOVD-Modelle und gemeinsame Typen

### Geplanter Zuschnitt

```text
native-interfaces/
  src/
    entity.rs
    capability.rs
    discovery.rs
    auth.rs
    extension.rs
    sovd.rs
    oem.rs
```

### Beispiele für Inhalte

- `EntityKind`, `EntityRef`, `EntityCollection`
- `DiscoveryDelta`
- `DataAccess`, `FaultAccess`, `OperationAccess`, `ModeAccess`, `ConfigAccess`
- `Authenticator`, `Authorizer`, `IdentityContext`
- `ExtensionDescriptor`

---

## 4.2 `native-core`

`native-core` wird zur Heimat der **laufzeitrelevanten Orchestrierung**.

### Inhalt

- `Topology`
- `CapabilityRegistry`
- `GatewayResolver`
- `DiscoveryRuntime`
- serverseitige Dienste wie Locking, Fault-Aggregation, Audit, History
- Kompositionsschicht / Builder für den Serverkern
- Adapter für Legacy-Backends

### Geplanter Zuschnitt

```text
native-core/
  src/
    topology.rs
    capability_registry.rs
    gateway_resolver.rs
    discovery_runtime.rs
    assembly.rs
    adapters/
      legacy_component_backend.rs
      cda_http_backend.rs
      native_http_backend.rs
    fault_manager.rs
    lock_manager.rs
    audit_log.rs
    history.rs
```

### Leitidee

`native-core` soll wissen,

- welche Entitäten es gibt,
- welche Fähigkeiten pro Entität verfügbar sind,
- und wie Requests intern aufgelöst werden.

Es soll **nicht** selbst der HTTP-Server und **nicht** der gesamte Satz an Extensions sein.

---

## 4.3 `native-sovd`

`native-sovd` wird zur Heimat des **HTTP-/SOVD-Vertrags**.

### Inhalt

- Core-Routen
- Extension-Router
- Request-Extraktoren
- Fehlerübersetzung
- OpenAPI-/CDF-Zusammensetzung
- HTTP-nahe Middleware

### Geplanter Zuschnitt

```text
native-sovd/
  src/
    routes/
      core/
        discovery.rs
        data.rs
        faults.rs
        operations.rs
        modes.rs
        locks.rs
        config.rs
        logs.rs
      extensions/
        uds.rs
        bridge.rs
        admin.rs
        fleet.rs
    auth/
      extractors.rs
      middleware.rs
    error.rs
    openapi/
      core.rs
      extensions.rs
      compose.rs
```

### Leitidee

`native-sovd` soll nicht alle fachlichen Entscheidungen enthalten, sondern sie über stabile Core-Schnittstellen konsumieren.

---

## 4.4 `native-server`

`native-server` wird zum **schlanken Binary-Entry-Point**.

### Inhalt

- Konfiguration laden
- Builder/Assembly anstoßen
- Backends und Extensions registrieren
- Server starten

### Nicht mehr gewünscht

- große Mengen fachlicher Initialisierung direkt in `main.rs`
- implizite Vermischung von Betriebslogik, Discovery, Backends und API-Aufbau

---

## 5. Zentrale Kernobjekte

## 5.1 `Topology`

### Aufgabe

`Topology` beschreibt die sichtbare diagnostische Landschaft:

- Komponenten
- Apps
- Areas
- Relationen zwischen ihnen
- Änderungen über die Zeit

### Warum sie zentral ist

Heute ist ein großer Teil der Sicht auf das System faktisch in Routing- und Backend-Listen versteckt. Für eine saubere Architektur muss diese Sicht explizit modelliert sein.

### Gewünschte Eigenschaften

- konsistente Read-/Write-Guards
- Änderungsereignisse (`TopologyEvent`)
- Relationen wie:
  - `apps_of_component`
  - `area_of_component`
  - `area_of_app`
- unabhängig von einem konkreten Transport

---

## 5.2 `CapabilityRegistry`

### Aufgabe

Die `CapabilityRegistry` verbindet Entitäten mit ihren verfügbaren Fähigkeiten.

`Topology` beantwortet: **Welche Entitäten gibt es?**

`CapabilityRegistry` beantwortet: **Welche Entität kann was und über welchen Handler?**

### Warum das wichtig ist

In der aktuellen Architektur sind Entität, Routing und Fähigkeit stark vermischt. Eine Registry dazwischen macht das Modell expliziter und reduziert die Abhängigkeit von einem einzigen großen Backend-Trait.

### Mögliche Form

```rust
pub struct EntityCapabilities {
    pub data: Option<Arc<dyn DataAccess>>,
    pub faults: Option<Arc<dyn FaultAccess>>,
    pub operations: Option<Arc<dyn OperationAccess>>,
    pub modes: Option<Arc<dyn ModeAccess>>,
    pub config: Option<Arc<dyn ConfigAccess>>,
    pub logs: Option<Arc<dyn LogAccess>>,
    pub software_packages: Option<Arc<dyn SoftwarePackageAccess>>,
    pub extended_diag: Option<Arc<dyn ExtendedDiagAccess>>,
}
```

Das ist nur eine Skizze; wichtig ist das Prinzip, nicht genau diese Struktur.

---

## 5.3 `GatewayResolver`

### Aufgabe

Der `GatewayResolver` löst einen SOVD-Request intern auf:

- Welche Entität ist betroffen?
- Welche Fähigkeit wird angesprochen?
- Welcher Handler oder welches Backend ist dafür zuständig?

### Vorteil gegenüber heute

Das Routing wird expliziter und weniger an einen großen Backing-Typ gekoppelt. Die Gateway-Rolle bleibt erhalten, aber sie wird als eigene Auflösungslogik modelliert.

---

## 5.4 `DiscoveryRuntime`

### Aufgabe

`DiscoveryRuntime` nimmt Discovery-Quellen auf und spielt deren Änderungen in `Topology` und `CapabilityRegistry` ein.

### Zielmodell

Statt nur statischer Konfiguration soll Discovery künftig auch dynamische Quellen unterstützen:

- statische Konfiguration
- mDNS-SD / DNS-SD
- Backend-Heartbeat / Presence
- Cloud-/Fleet-Discovery

### Gewünschtes Datenmodell

```rust
pub struct DiscoveryDelta {
    pub remove: Vec<EntityRef>,
    pub add: EntityCollection,
}
```

Das folgt bewusst dem stärkeren Grundgedanken aus `opensovd-core`, ohne dessen Implementierung direkt zu übernehmen.

---

## 5.5 `NativeServerBuilder`

### Aufgabe

Der Builder bildet den Kompositionspunkt für den nativen Server:

- Listener / Transport
- Security
- Topology
- Discovery
- Core services
- Extensions
- observability

### Warum er wichtig ist

Der aktuelle `main.rs` ist für den Reifegrad des Projekts zu breit. Ein Builder macht:

- Assembly testbar
- Defaults explizit
- Varianten besser kontrollierbar
- Serveraufbau verständlicher

---

## 6. Trait-Modell v2

## 6.1 Grundprinzip

Statt eines breiten `ComponentBackend` werden die Fähigkeiten in kleine Traits zerlegt.

### Kernfähigkeiten

```rust
pub trait EntityDiscovery {
    fn entities(&self) -> EntityCollection;
}

pub trait DataAccess: Send + Sync {
    async fn list_data(&self, entity: &EntityRef) -> Result<Vec<DataDescriptor>, ServiceError>;
    async fn read_data(&self, entity: &EntityRef, data_id: &str) -> Result<serde_json::Value, ServiceError>;
    async fn write_data(&self, entity: &EntityRef, data_id: &str, value: serde_json::Value) -> Result<(), ServiceError>;
}

pub trait FaultAccess: Send + Sync {
    async fn list_faults(&self, entity: &EntityRef, filter: FaultFilter) -> Result<Vec<SovdFault>, ServiceError>;
    async fn get_fault(&self, entity: &EntityRef, fault_id: &str) -> Result<SovdFault, ServiceError>;
    async fn clear_fault(&self, entity: &EntityRef, fault_id: &str) -> Result<(), ServiceError>;
}
```

### Weitere Fähigkeiten

- `OperationAccess`
- `ModeAccess`
- `LockAccess` oder serverseitig zentraler `LockService`
- `ConfigAccess`
- `LogAccess`
- `SoftwarePackageAccess`
- `CapabilityAccess`

### Extensions separat

UDS-nahe Funktionen bleiben in getrennten Traits, z. B.:

- `ExtendedDiagAccess`
- `SecurityAccess`
- `MemoryAccess`
- `FlashAccess`

Damit bleibt sichtbar, was Standard-SOVD ist und was nicht.

---

## 6.2 Was serverseitig zentral bleibt

Nicht alles muss backend-spezifisch sein. Einige Dinge sind systemweit sinnvoller als zentrale Serverdienste:

- Authentifizierung und Autorisierung
- Locking
- Audit
- Rate Limiting
- Feature Flags
- History / Export / Observability

Die Zielarchitektur soll daher **nicht** alles in capability-Traits der Backends verschieben.

---

## 6.3 Kompatibilitätsadapter für den Übergang

Damit die Migration inkrementell bleibt, soll ein Adapter die heutige Welt weiter nutzbar machen.

### Idee

Ein `LegacyComponentBackendAdapter` kapselt ein bestehendes `ComponentBackend` und exponiert daraus die kleineren Fähigkeiten.

### Nutzen

- kein Big-Bang-Umbau
- bestehende Tests und Backends bleiben zunächst nutzbar
- neue Architektur kann schrittweise unter dem bestehenden Verhalten wachsen

---

## 7. Sicherheitsgrenze v2

## 7.1 Trennung von AuthN und AuthZ

Die Zielarchitektur übernimmt explizit die saubere Trennung zwischen:

- **Authenticator** — Wer ist der Aufrufer?
- **Authorizer** — Darf dieser Aufrufer diesen Request ausführen?

### Rolle des OEM-Profils

Das `OemProfile` bleibt wertvoll, sollte aber stärker als Policy-Quelle verstanden werden:

- Claim-Regeln
- Scope-Regeln
- Entity-ID-Regeln
- Discovery-Regeln
- CDF-Regeln

Nicht jede dieser Regeln muss direkt denselben Runtime-Typ dominieren.

---

## 7.2 Vorgeschlagene Grenze

```rust
pub trait Authenticator {
    type Identity;
    async fn authenticate(&self, request: &RequestParts) -> Result<Self::Identity, AuthError>;
}

pub trait Authorizer<I> {
    async fn authorize(&self, identity: &I, request: &RequestContext) -> Result<(), AuthError>;
}
```

Das OEM-Profil kann darunter spezialisierte Policy-Implementierungen liefern oder konfigurieren.

---

## 8. Route- und OpenAPI-Struktur v2

## 8.1 Core-Routen

Der erste Kern soll eigene Router-Module haben für:

- Discovery
- Components / entity capabilities
- Data
- Faults
- Operations
- Modes
- Locks
- Configurations
- Logs
- Version / metadata / CDF

## 8.2 Extension-Router

Getrennt davon:

- `x-uds`
- `x-admin`
- `x-bridge`
- Fleet-/Cloud-spezifische Erweiterungen
- OEM-spezifische Zusatzpfade

## 8.3 OpenAPI-Komposition

Die CDF-/OpenAPI-Erzeugung soll künftig ebenfalls getrennt sein:

- Core-Spezifikation
- registrierte Extension-Fragmente
- OEM-Policy-Anpassungen

Dadurch wird sichtbarer, was Teil des Kerns ist und was optionale Erweiterung ist.

---

## 9. Was aus `opensovd-core` bewusst übernommen werden sollte

- `Topology` als eigenes Kernobjekt
- Discovery als Delta-/Stream-Modell
- kleine Provider-/Capability-Traits
- generische AuthN-/AuthZ-Grenze
- Builder-/Assembly-Denke
- modulare Route-Struktur
- disziplinierte Fehlergrenzen

## 10. Was bewusst **nicht** übernommen werden sollte

- keine direkte technische Abhängigkeit als Fundament
- kein 1:1-Port der aktuellen Struktur
- keine Aufgabe des Gateway-first-Zuschnitts
- keine Vermischung von Standard-SOVD und UDS-naher Funktionalität im Kern

---

## 11. Inkrementelle Migrationsreihenfolge

## Phase 1 — Architekturgrenze explizit machen

- Kern vs. Extensions in Docs und Routing markieren
- `ComponentBackend` fachlich zerlegen, zunächst nur als Design und Wrapper
- `x-uds` klar als Extension positionieren

## Phase 2 — Topology einführen

- `EntityRef`, `EntityKind`, `EntityCollection` konsolidieren
- `Topology` in `native-core` einführen
- bestehende Discovery-/Backend-Konfiguration in `Topology` spiegeln

## Phase 3 — CapabilityRegistry + Adapter

- `CapabilityRegistry` einführen
- `LegacyComponentBackendAdapter` bauen
- bestehende Routen über Registry statt direkt über den großen Backend-Typ anbinden

## Phase 4 — AuthN/AuthZ entkoppeln

- Laufzeitgrenze zwischen Authentifizierung und Autorisierung schärfen
- OEM-Policies in klarere Zuständigkeiten überführen

## Phase 5 — Assembly vereinfachen

- `NativeServerBuilder` bzw. `ServerAssembly` einführen
- `main.rs` auf Konfiguration und Start reduzieren

## Phase 6 — Dynamische Discovery und spätere Konvergenz

- Discovery-Deltas einführen
- Topology-Events nutzen
- gezielt prüfen, welche Bausteine später gemeinschaftlich oder upstream-fähig sind

---

## 12. Entscheidungsregel für künftige Umbauten

Ein Umbau gehört in diese Zielarchitektur, wenn er mindestens eine der folgenden Fragen mit **ja** beantwortet:

- Macht er die Grenze zwischen Server, Gateway und CDA klarer?
- Macht er den SOVD-Kern kleiner und expliziter?
- Ersetzt er ein breites Interface durch kleinere, fachliche Fähigkeiten?
- Trennt er Kern und Extension deutlicher?
- Erhöht er die Fähigkeit, Discovery und Topologie explizit zu modellieren?
- Verbessert er die Kompositions- und Testbarkeit des Servers?

Wenn nicht, ist er wahrscheinlich eher Feature-Ausbau als Architekturarbeit.

---

## 13. Kurzfassung für die interne Diskussion

Die Zielrichtung sollte nicht sein, `opensovd-core` in Rust nachzubauen. Die Zielrichtung sollte sein, aus den stärkeren Konzepten von `opensovd-core` und den praktischen Stärken des `native-server` eine **eigene Rust-Zielarchitektur** zu formen:

- **Topology-first als Modell**
- **Gateway-first im Zugriff**
- **kleine capability-orientierte Traits**
- **klare AuthN/AuthZ-Grenze**
- **strikte Kern-/Extension-Trennung**
- **inkrementelle Migration ohne Big Bang**

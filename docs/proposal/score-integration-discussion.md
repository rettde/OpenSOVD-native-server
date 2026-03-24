# Diskussionspunkte: OpenSOVD-native-server als Bestandteil von Eclipse S-CORE

| Feld       | Wert                                                    |
|------------|---------------------------------------------------------|
| **Datum**  | 2026-03-22                                              |
| **Autor**  | Rettstatt (unterstützt durch Cascade AI)                |
| **Status** | Entwurf — Diskussionsgrundlage für OpenSOVD-Committer   |
| **Scope**  | Aufnahme von `OpenSOVD-native-server` in S-CORE / OpenSOVD |

---

## 0. Zusammenfassung

Der `OpenSOVD-native-server` ist eine vollständige Rust-Implementierung des
SOVD-Servers gemäß ISO 17978-3, mit 489+ Tests, 100% Spec-Abdeckung und
produktionsnaher Härtung. Dieses Dokument listet die aus meiner Sicht
notwendigen Diskussionspunkte auf, um eine Aufnahme in das Eclipse-Ökosystem
(OpenSOVD oder S-CORE) zu ermöglichen.

---

## 1. Einordnung im Eclipse-Ökosystem

### 1.1 Abgrenzung zu bestehenden Komponenten

| Bestehende Komponente | Sprache | Rolle | Überlappung mit native-server |
|----------------------|---------|-------|-------------------------------|
| `opensovd-core` | C++ | SOVD Server / Client / Gateway | **Hoch** — beide implementieren die SOVD-Server-Rolle |
| `classic-diagnostic-adapter` (CDA) | Rust | SOVD → UDS/DoIP Brücke | **Komplementär** — native-server nutzt CDA als Backend |
| `cpp-bindings` | C++ | Client-Bibliotheken | **Keine** — native-server ist serverseitig |
| S-CORE Fault API | C++ | Fault-Modell für HPC | **Partiell** — native-server hat eigenes FaultManager-Modell |
| S-CORE Persistency | C++ | Persistenz-Abstraction | **Partiell** — native-server hat `StorageBackend` trait |

### 1.2 Diskussionsfrage: Wo angesiedelt?

```
Option A: eclipse-opensovd/OpenSOVD-native-server
  → Neben opensovd-core als alternative Rust-Implementierung

Option B: eclipse-score/.../sovd-server-rust
  → Teil der S-CORE Plattform, SOVD als Plattform-Service

Option C: Eigenes Top-Level-Projekt unter eclipse-opensovd
  → Eigenständiges Repository mit eigener Governance
```

**Empfehlung:** Option A — der Server ist funktional eine SOVD-Server-Implementierung
und gehört logisch zu OpenSOVD. S-CORE-Integration erfolgt über definierte
Schnittstellen (Fault API, Persistency), nicht durch Einbettung.

---

## 2. Eclipse Foundation Governance

### 2.1 IP Clearance (CQ-Prozess)

Jede Drittanbieter-Dependency muss durch den Eclipse IP-Review (Contribution
Questionnaire). Der native-server hat ~40 direkte Dependencies:

| Lizenz-Kategorie | Anzahl | Status |
|-----------------|--------|--------|
| Apache-2.0 | ~25 | ✅ Eclipse-kompatibel |
| MIT | ~10 | ✅ Eclipse-kompatibel |
| BSD-3-Clause | ~3 | ✅ Eclipse-kompatibel |
| MPL-2.0 (vsomeip) | 1 | ⚠️ Nur über FFI, optional, feature-gated |
| Copyleft (GPL/LGPL) | 0 | ✅ Keine Copyleft-Dependencies |

**Aktion:** Vollständige CQ-Einreichung aller Dependencies vorbereiten.
`cargo-cyclonedx` SBOM ist bereits im CI generiert.

### 2.2 Eclipse Contributor Agreement (ECA)

Alle bisherigen Commits stammen von einem Autor. ECA muss vor dem ersten
Eclipse-Commit unterschrieben sein.

**Aktion:** ECA für alle Committer sicherstellen.

### 2.3 Committer-Modell

Eclipse-Projekte benötigen mindestens 2 Committer aus verschiedenen Organisationen.

**Diskussion:**
- Wer sind die initialen Committer?
- Gibt es Interesse anderer OEMs oder Zulieferer, Committer zu stellen?
- Mentor aus bestehendem OpenSOVD-Projekt?

### 2.4 SPDX-Header

Alle Dateien tragen bereits den korrekten Header:
```rust
// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0
```

**Status:** ✅ Bereits konform.

---

## 3. Technische Integrationspunkte

### 3.1 Koexistenz mit opensovd-core (C++)

Beide Server implementieren dieselbe ISO 17978-3 API. Die Koexistenz muss
geklärt werden:

| Aspekt | opensovd-core (C++) | native-server (Rust) |
|--------|--------------------|-----------------------|
| Architektur | Provider-per-entity | ComponentBackend god-trait + Router |
| CDF-Validierung | Internes Schema-System | Eigene CDF-Generierung + CDF Validator custom rules |
| Zielplattform | Embedded / HPC | HPC / Cloud / Container |
| Backend-Anbindung | Direkte Provider | HTTP-Gateway → CDA |
| Test-Abdeckung | ~160 Tests | 489+ Tests |
| OEM-Erweiterbarkeit | Plugin-Traits (in Entwicklung) | `OemProfile` trait mit build.rs Auto-Detection |

**Diskussionsfrage:** Ist die Eclipse-Community offen für zwei parallele
SOVD-Server-Implementierungen (C++ und Rust)? Oder wird Konsolidierung erwartet?

**Argument für Koexistenz:**
- Verschiedene Deployment-Ziele (Embedded vs. Cloud)
- CDA ist bereits Rust — native-server schließt die Lücke zum reinen Rust-Stack
- CDF Validator mit custom rules beweist Conformance beider Implementierungen
  unabhängig voneinander

### Ergänzende Bewertung: Ist `opensovd-core` eine belastbare Grundlage?

 **Kurzurteil:** als konzeptioneller Referenzpunkt **ja**, als unmittelbare
 technische Grundlage für `native-server` derzeit **nur eingeschränkt**.

#### Was gegen `opensovd-core` als direktes Fundament spricht

 - **[kein konsolidierter Mainline-Kern]**
   - Im uns vorliegenden Clone liegt der fachliche Gehalt nicht in einer sichtbar
     gereiften Mainline, sondern verteilt sich auf inkrementelle Branches wie
     `inc/zf`, `inc/liebherr`, `feat/mtls` und `inc/native-server`.
   - Das spricht eher für einen frühen Architektur- und Integrationsstand als
     für ein bereits belastbares Fundament, auf dem man eine zweite
     Implementierung direkt aufbauen sollte.

 - **[grundlegender Architekturmismatch]**
   - `opensovd-core` ist um `Topology`, `EntityRef` und entity-lokale Provider
     (`DataProvider`, `FaultProvider`) herum aufgebaut.
   - `native-server` ist dagegen gateway-orientiert: `ComponentRouter`
     aggregiert `ComponentBackend`-Implementierungen und routet Requests zu
     CDA- oder nativen SOVD-Backends.
   - Dieser Unterschied ist kein Refactoring-Detail, sondern ein anderer
     Zuschnitt der Kernverantwortung. Ein direktes Aufsetzen auf
     `opensovd-core` würde daher entweder dessen Modell verbiegen oder den
     `native-server` von seinem Gateway-Zielbild wegziehen.

 - **[ungeklärte Architekturfragen würden implizit festgeschrieben]**
   - Wenn `native-server` `opensovd-core` zum technischen Fundament macht,
     würden damit offene Leitentscheidungen faktisch vorweggenommen:
     entity-zentriertes Provider-Modell statt Gateway-Zuschnitt,
     Topology-first statt Backend-Routing-first, implizite Priorisierung einer
     bestimmten Server-Sicht, bevor die Community die Rollengrenzen zwischen
     Server, Gateway und CDA geklärt hat.

 - **[Scope passt nicht zum aktuellen nativen Zielbild]**
   - `opensovd-core` wirkt derzeit bewusst schmaler und näher an einem kleinen,
     standardnahen Referenzkern.
   - `native-server` adressiert dagegen bereits Gateway-Betrieb,
     OEM-Erweiterbarkeit, CDF-Generierung, Betriebsfunktionen und eine breitere
     Integrationsrealität.
   - Gerade deshalb ist `opensovd-core` derzeit keine ausreichende Grundlage
     für den gesamten nativen Zuschnitt.

#### Welche Konzepte in der Implementierung trotzdem überlegen wirken

 Auch wenn `opensovd-core` aus meiner Sicht **nicht** das direkte Fundament für
 `native-server` sein sollte, gibt es dort mehrere Konzepte, die fachlich und
 implementatorisch stärker wirken als unser heutiger Zuschnitt und die sich zu
 übernehmen lohnen.

 - **[`Topology` als First-Class-Modell]**
   - `opensovd-core` modelliert Komponenten, Apps und Areas als expliziten
     Entitätsgraphen mit konsistenten Lese-/Schreib-Guards, Relationen wie
     `apps_of_component`, `area_of_component` und einem
     `broadcast`-basierten Event-Mechanismus (`TopologyEvent`).
   - Das ist dem heutigen rein komponentenzentrierten Routing im
     `native-server` konzeptionell überlegen, wenn wir Discovery, Relationen und
     spätere dynamische Topologieänderungen sauber abbilden wollen.
   - **Empfehlung:** nicht das gesamte `opensovd-core` übernehmen, aber eine
     vergleichbare interne `Topology`-Schicht für den nativen Kern prüfen.

 - **[saubere Interface-Segregation über kleine Provider-Traits]**
   - `opensovd-core` hängt Fähigkeiten wie Daten- und Fault-Zugriff an kleine,
     explizite Traits (`DataProvider`, `FaultProvider`) statt an einen großen
     Sammel-Backend-Typ.
   - Im Vergleich dazu ist `native-server` mit `ComponentBackend` weiterhin
     relativ breit aufgestellt, auch wenn mit `ExtendedDiagBackend` bereits ein
     erster Schritt in Richtung Trait-Diät erfolgt ist.
   - **Empfehlung:** das Gateway-Modell beibehalten, aber `ComponentBackend`
     weiter in capability-orientierte Facetten zerlegen.

 - **[klare Trennung von Authentifizierung und Autorisierung]**
   - `opensovd-server` trennt generisch zwischen `Authenticator` und
     `Authorizer` und baut beide als austauschbare Middleware-Layer.
   - Das ist als Architekturgrenze sauberer als eine engere Kopplung von
     Auth-Konfiguration, Policy-Logik und OEM-Profilen.
   - **Empfehlung:** das bestehende `OemProfile`-Modell nicht aufgeben, aber die
     Laufzeitgrenze zwischen AuthN und AuthZ stärker nach diesem Muster
     entkoppeln.

 - **[`DiscoveryProvider` als Stream von Topologie-Diffs]**
   - Das Discovery-Modell in `opensovd-core` ist stark: Provider liefern einen
     langlebigen Stream aus `(remove, add)`-Deltas, die in die zentrale
     Topologie eingespielt werden.
   - Für einen gateway-orientierten nativen Server ist das interessanter als ein
     rein statisches Backend-Setup, weil sich damit mDNS-SD, Backend-Appearing
     und Backend-Disappearing konzeptionell sauber abbilden lassen.
   - **Empfehlung:** dieses Muster für die künftige Backend-/Entity-Discovery im
     nativen Server adaptieren.

 - **[komponierbarer Serveraufbau]**
   - `opensovd-server` besitzt mit `ServerBuilder` einen klaren,
     testbaren Kompositionspunkt für Listener, Base-URI, Discovery,
     Middleware-Layer und zusätzliche Services.
   - Demgegenüber ist der Startpfad in `native-server/src/main.rs` sehr breit und
     enthält viel Initialisierung in einem Monolithen.
   - **Empfehlung:** keinen direkten Port, aber mittelfristig eine ähnliche
     Builder-/Assembly-Schicht für den nativen Server einführen.

 - **[modulare Route-Struktur und disziplinierte Fehlergrenzen]**
   - `opensovd-server` trennt Routen sauber nach `entities`, `data`, `fault` und
     `version`; zusätzlich mappt `routes/error.rs` Domänenfehler typisiert auf
     HTTP-/SOVD-Responses und sanitisiert interne Fehlerdetails.
   - Das ist strukturell klarer als eine sehr große zentrale Route-Datei.
   - **Empfehlung:** die Route-Struktur im `native-server` entlang von
     Kern-/Extension-Grenzen weiter modularisieren und die Fehlerübersetzung an
     einer zentralen Stelle bündeln.

#### Welche Konzepte **nicht** ersetzt werden sollten

 Ein pauschaler Rückbau in Richtung `opensovd-core` wäre ebenfalls falsch.
 Mehrere Konzepte im `native-server` sind für unser Zielbild derzeit stärker
 oder passender:

 - **[Gateway-first-Zuschnitt]**
   - Für die Anbindung an CDA und andere SOVD-Backends ist unser
     `ComponentRouter`/`SovdHttpBackend`-Modell passend.

 - **[CDF-/OEM-Erweiterbarkeit]**
   - Das `OemProfile`-/`CdfPolicy`-Modell und die eigene CDF-Generierung sind
     aktuell stärker anpassbar als das, was `opensovd-core` heute sichtbar
     anbietet.

 - **[betriebliche Reife]**
   - TLS, OIDC/JWKS, OTLP, Feature Flags, Persistenzoptionen, Audit-Trail und
     weitere Betriebsfunktionen sind im `native-server` bereits deutlich weiter
     ausgearbeitet.

#### Empfehlung

 **Empfehlung:** `opensovd-core` sollte im Proposal **nicht** als tragendes
 technisches Fundament des `native-server` dargestellt werden. Stattdessen sollte
 es als:

 - **[Referenzpunkt]** für Modellierungs- und Architekturabgleich,
 - **[Interoperabilitätsziel]** für einen kleinen SOVD-Kern,
 - und **[Konzeptspender]** für gezielt übernehmbare Bausteine

 beschrieben werden.

 Praktisch bedeutet das:

 1. **Keine enge technische Abhängigkeit erzwingen**
    - keine Aussage „native-server basiert auf opensovd-core“
    - keine forcierten Umbauten nur um das Modell künstlich anzugleichen

 2. **Selektive Übernahme stärkerer Konzepte**
    - `Topology`-Schicht
    - capability-orientierte Provider-Facetten
    - generische AuthN/AuthZ-Grenze
    - Discovery-Diff-Modell
    - Builder-/Assembly-Pattern
    - modulare Route-Struktur

 3. **Konvergenz auf Interface-Ebene statt auf Implementierungsebene**
    - gemeinsamer minimaler SOVD-Kern
    - kompatible CDF-/Capability-Sicht
    - klare Rollenabgrenzung zwischen Server, Gateway und CDA

 4. **Übernahme erst nach Architekturklärung**
    - erst Leitbild und Kernprofil festziehen
    - dann gezielt entscheiden, welche Konzepte upstream-fähig oder gemeinsam
      nutzbar sind

 Damit bleibt die Aussage im Proposal belastbar: `opensovd-core` ist aktuell
 **nicht** die fertige Grundlage, auf die man den nativen Server einfach setzt;
 es enthält aber mehrere **qualitativ gute Architekturideen**, die wir bewusst
 übernehmen sollten.

### Empfohlener technischer Pfad: Rust-native Überführung statt direkter Abhängigkeit

 Aus der Code- und Architekturbetrachtung folgt aus meiner Sicht ein klarer
 technischer Pfad für das Proposal:

 **Nicht**:

 - `native-server` auf `opensovd-core` als Runtime-Fundament aufsetzen
 - das entity-first Modell ungeprüft übernehmen
 - die bestehende Gateway-Orientierung des nativen Servers zurückbauen

 **Sondern**:

 - die stärkeren Architekturideen aus `opensovd-core` bewusst in eine
   **Rust-native Zielarchitektur** überführen
 - dabei den `native-server` als gateway-orientierten SOVD-Server beibehalten
 - die Konvergenz auf **Konzept- und Interface-Ebene** suchen, nicht über eine
   erzwungene gemeinsame Implementierungsbasis

 Konkret heißt das:

 1. **`Topology` in Rust als First-Class-Kernmodell etablieren**
    - explizite Entitätsreferenzen
    - Relationen zwischen Komponenten, Apps und Areas
    - Änderungsereignisse statt rein statischer Sicht

 2. **Breites Backend-Interface in kleine Fähigkeiten zerlegen**
    - Discovery
    - Data
    - Faults
    - Operations
    - Modes
    - Locks
    - Config
    - Capabilities
    - UDS-nahe Erweiterungen weiterhin getrennt halten

 3. **AuthN und AuthZ architektonisch entkoppeln**
    - OEM-Policies bleiben wichtig
    - die Laufzeitgrenze zwischen Authentifizierung und Autorisierung sollte aber
      sauberer und generischer modelliert werden

 4. **Discovery als Stream von Topologie-Diffs modellieren**
    - nicht nur statische Backend-Listen
    - sondern ein Modell, das mDNS, Backend-Wechsel und dynamische SOVD-Landschaften
      sauber aufnehmen kann

 5. **Kern und Extensions strukturell trennen**
    - Standard-SOVD in den Kern
    - `x-uds`, Cloud/Fleet, Ops und OEM-spezifische Themen klar als Erweiterung

 6. **Server-Assembly vereinfachen**
    - weg vom sehr breiten Startup-Monolithen
    - hin zu einer klaren Builder-/Assembly-Schicht

 **Empfehlung für das Proposal:**

 Wir sollten deshalb nicht argumentieren, dass `native-server` auf
 `opensovd-core` basieren sollte. Die stärkere Aussage ist:

 > `opensovd-core` ist aktuell vor allem als Architektur- und Konzeptquelle
 > wertvoll. Der strategisch sinnvolle Weg ist, die besten Konzepte gezielt in
 > eine konsistente Rust-Architektur für den nativen Server zu überführen.

 Das ist aus meiner Sicht die sauberste Position, weil sie gleichzeitig

 - die bisherige Arbeit in `opensovd-core` ernst nimmt,
 - die Stärken des nativen Servers nicht opfert,
 - und einen realistischen Konvergenzpfad eröffnet.

### 3.2 CDF Validator Integration

 Der `dsagmbh/sovd-cdf-validator` ist das Referenz-Tool zur SOVD-Conformance-Prüfung.

 **Vorschlag:**
1. CI-Job der den generierten CDF (`/openapi.json`) gegen Standard-Regeln validiert
2. Custom Rules für native-server-spezifische Extensions (RXSWIN, TARA, UCM, x-uds)
3. CDF-Diff zwischen opensovd-core und native-server als Kompatibilitätsnachweis

**Diskussionsfrage:** Soll der CDF Validator als gemeinsamer Quality Gate für
beide Server-Implementierungen dienen?

### 3.3 S-CORE Fault API Alignment

native-server hat ein eigenes Fault-Modell:
- `FaultBridge` → `FaultGovernor` (Debounce) → `FaultManager`
- `SovdFault` mit `severity`, `status`, `scope`, `affectedSubsystem`, `correlatedSignals`
- SSE-basierte Fault-Subscription

S-CORE definiert ebenfalls eine Fault API für HPC-Plattformen.

**Diskussionsfrage:** Kann native-server's `FaultSink` trait an die S-CORE
Fault API angebunden werden? Das Design ist bereits darauf ausgelegt:
```rust
// native-interfaces/src/audit_sink.rs
pub trait AuditSink: Send + Sync {
    fn record(&self, entry: SovdAuditEntry);
}
```
Analog existiert `FaultSink` als Eingangs-Interface für externe Fault-Quellen.

### 3.4 S-CORE Persistency Alignment

native-server nutzt eine eigene `StorageBackend`-Abstraktion:
```rust
pub trait StorageBackend: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<u8>>;
    fn set(&self, key: &str, value: Vec<u8>);
    fn delete(&self, key: &str);
    fn snapshot(&self) -> Vec<(String, Vec<u8>)>;
    fn rollback(&self, snapshot: Vec<(String, Vec<u8>)>);
}
```

**Diskussionsfrage:** Kann die S-CORE Persistency-Schicht als Backend für
dieses Trait verwendet werden? Oder ist ein Adapter nötig?

### 3.5 SOME/IP (COVESA/vsomeip)

native-server bietet optional SOME/IP-Unterstützung über FFI-Bindings:
- `native-comm-someip` Crate mit `vsomeip-ffi` Feature
- Stub-Modus ohne echte vSomeIP-Bibliothek
- MPL-2.0-lizenzierter Code wird nur über C-FFI angebunden, nicht verlinkt

**Diskussionsfrage:** Wie passt das zu S-CORE's eigener Communication-Middleware?
Gibt es eine gemeinsame SOME/IP-Abstraction?

### 3.6 Logging / Tracing

native-server nutzt `tracing` + `tracing-subscriber` mit optionalem OTLP-Export.
S-CORE hat eigene Logging-Dienste.

**Diskussionsfrage:** Reicht die OTLP-Integration als Brücke, oder wird eine
direkte S-CORE Logging-Integration erwartet?

---

## 4. OEM-spezifischer Code

### 4.1 Architektur

native-server hat ein `OemProfile`-Plugin-System:
- `OemProfile` trait mit Sub-Traits: `AuthPolicy`, `AuthzPolicy`, `EntityIdPolicy`, `DiscoveryPolicy`, `CdfPolicy`
- `build.rs` erkennt `src/oem_*.rs` Dateien automatisch
- Open-Source-Build bekommt `SampleOemProfile` (permissiv)
- OEM-spezifische Profile (z.B. `oem_mbds.rs`) sind `.gitignore`d

### 4.2 Diskussionsfrage

Proprietärer OEM-Code darf nicht ins Eclipse-Repository. Das aktuelle Design
löst das bereits durch:
1. `.gitignore` für `oem_*.rs` (außer `oem_sample.rs`)
2. `build.rs` Auto-Detection — kein Compile-Fehler ohne OEM-Profile
3. `SampleOemProfile` als vollständig dokumentiertes Template

**Ist dieses Modell Eclipse-kompatibel?** Der CDA-Ansatz für OEM-Erweiterungen
sollte als Referenz dienen.

---

## 5. CI/CD Anforderungen

### 5.1 Aktuelle CI-Pipeline

```yaml
# .github/workflows/ci.yml
- cargo fmt --check
- cargo clippy --workspace -- -D warnings
- cargo test --workspace
- cargo-cyclonedx (SBOM)
- Docker Build (distroless)
```

### 5.2 Eclipse CI-Anforderungen

| Anforderung | Status | Aktion |
|-------------|--------|--------|
| Eclipse Foundation CI (Jenkins/GHA) | ⚠️ Aktuell GitHub Actions | Migration oder Dual-CI |
| Reproducible Builds | ✅ `Cargo.lock` committed | — |
| SBOM Generation | ✅ CycloneDX in CI | — |
| License Scanning | ⚠️ Nicht automatisiert | `cargo-deny` oder `cargo-license` hinzufügen |
| Dependency Audit | ⚠️ Nicht automatisiert | `cargo-audit` hinzufügen |
| Signed Releases | ❌ Nicht implementiert | Sigstore oder GPG-Signing einrichten |

---

## 6. Dokumentation

### 6.1 Vorhandene Dokumentation

| Dokument | Pfad | Status |
|----------|------|--------|
| README | `README.md` | ✅ Umfassend (423 Zeilen) |
| CONTRIBUTING | `CONTRIBUTING.md` | ✅ Mit ECA-Referenz |
| NOTICE | `NOTICE` | ✅ Eclipse-konform |
| LICENSE | `LICENSE` | ✅ Apache-2.0 |
| CHANGELOG | `CHANGELOG.md` | ✅ Per-Version |
| Compliance Audit | `docs/iso-17978-3-compliance-audit.md` | ✅ 51/51 Requirements |
| ADRs (19 Stück) | `docs/adr/` | ✅ A1.1–A5.1 |
| Roadmap | `docs/integrated-roadmap.md` | ✅ Waves 1–4 complete |
| Architecture | `README.md` + ADRs | ✅ |
| API Reference | `README.md` Endpoints-Tabelle | ✅ |
| OpenAPI Spec | `/openapi.json` (runtime) | ✅ CDF |
| AGENTS.md | `AGENTS.md` | ✅ AI-Disclosure |

### 6.2 Fehlende Dokumentation für Eclipse

| Dokument | Aktion |
|----------|--------|
| Eclipse Project Proposal (`.eclipseproject`) | Erstellen |
| Eclipse Foundation NOTICE (erweitert) | Prüfen gegen EF-Template |
| Third-Party License Summary | Aus SBOM generieren |
| Developer Setup Guide | Erweitern (aktuell nur Quick Start) |
| Release Process | Dokumentieren |

---

## 7. Qualitätskennzahlen

| Metrik | Wert |
|--------|------|
| **Testanzahl** | 489+ |
| **Line Coverage** | ~81% |
| **Clippy** | Pedantic clean, zero warnings |
| **unsafe_code** | `#![forbid(unsafe_code)]` (Ausnahme: vsomeip FFI) |
| **Copyleft Dependencies** | 0 |
| **SOVD Conformance** | 51/51 ISO 17978-3 Requirements |
| **OpenAPI CDF** | Generiert, 12 Contract Tests |
| **Security** | JWT/OIDC, mTLS, RBAC, Rate Limiting, Audit Trail |
| **ADRs** | 19 architektonische Entscheidungen dokumentiert |
| **MSRV** | Rust 1.88+ |

---

## 8. Vorgeschlagene Reihenfolge der Diskussionspunkte

| # | Thema | Priorität | Blocker? |
|---|-------|-----------|----------|
| 1 | **Koexistenz C++ / Rust** — Akzeptiert die Community zwei Server-Implementierungen? | Hoch | Ja |
| 2 | **Repository-Platzierung** — OpenSOVD oder S-CORE? | Hoch | Ja |
| 3 | **Committer-Modell** — Wer sind die initialen Committer (min. 2 Orgs)? | Hoch | Ja |
| 4 | **IP Clearance** — CQ-Prozess für alle Dependencies | Hoch | Ja |
| 5 | **CDF Validator** als gemeinsamer Quality Gate | Mittel | Nein |
| 6 | **S-CORE Fault API** Alignment (FaultSink Adapter) | Mittel | Nein |
| 7 | **S-CORE Persistency** Alignment (StorageBackend Adapter) | Mittel | Nein |
| 8 | **OEM-Plugin-Modell** — Eclipse-Kompatibilität bestätigen | Mittel | Nein |
| 9 | **CI/CD Migration** — Eclipse Foundation CI Setup | Niedrig | Nein |
| 10 | **Signed Releases** — Sigstore/GPG | Niedrig | Nein |

---

## 9. Nächste Schritte

1. **Intern:** Diskussionspapier mit OpenSOVD Project Lead teilen
2. **Feedback:** Klärung der Blocker-Fragen (#1–#4)
3. **CDF Validator:** Integration in CI als Conformance-Nachweis
4. **IP-Vorbereitung:** `cargo-deny` + SBOM für CQ-Einreichung aufsetzen
5. **Eclipse Proposal:** Formales Project Proposal erstellen (nach Klärung #1–#4)

---

## 10. Anhang: Was bringt native-server ins Ökosystem ein?

### Alleinstellungsmerkmale gegenüber opensovd-core (C++)

| Feature | native-server | opensovd-core (C++) |
|---------|:------------:|:-------------------:|
| Vollständiger Rust-Stack (CDA + Server) | ✅ | ❌ |
| OEM Plugin Architecture (`OemProfile`) | ✅ | In Entwicklung |
| CDF-Generierung mit OEM-Customization | ✅ | ❌ |
| Fault Pipeline (Bridge → Governor → Manager) | ✅ | Extern (S-CORE) |
| Multi-Tenant Isolation | ✅ | ❌ |
| Cloud Bridge Transport | ✅ | ❌ |
| COVESA VSS Data Catalog | ✅ | ❌ |
| RXSWIN / TARA / UCM Endpoints | ✅ | ❌ |
| UDS Vendor Extensions (x-uds) | ✅ | ❌ |
| SSE Fault Subscription | ✅ | ❌ |
| Prometheus RED Metrics | ✅ | ❌ |
| OTLP Tracing | ✅ | ❌ |
| Hash-Chained Audit Trail | ✅ | ❌ |
| Feature Flags (Runtime Toggles) | ✅ | ❌ |
| Container-Ready (Distroless Docker, Helm) | ✅ | ❌ |
| systemd Watchdog Integration | ✅ | ❌ |
| 489+ Tests, 81% Coverage | ✅ | ~160 Tests |

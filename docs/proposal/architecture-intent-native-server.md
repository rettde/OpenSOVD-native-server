# Architektur- und Intentionsbeschreibung — OpenSOVD-native-server

| Feld | Wert |
|------|------|
| **Status** | Diskussionsentwurf |
| **Datum** | 2026-03-24 |
| **Autor** | Rettstatt, mit AI-Unterstützung bei Recherche und Strukturierung |
| **Zweck** | Klärung von Rolle, Scope und Leitentscheidungen vor einer eventuellen Community-Diskussion |

---

## 1. Warum dieses Dokument existiert

Der aktuelle `opensovd-native-server` hat in kurzer Zeit viel Funktionalität gesammelt.
Das hat geholfen, offene Fragen sichtbar zu machen, aber auch zu Unschärfen geführt:
Was ist hier eigentlich der intendierte Kern — ein SOVD-Server, ein Gateway, ein CDA-naher Adapter oder ein Sammelbecken für alle diagnostischen Themen?

Dieses Dokument zieht deshalb bewusst eine Grenze ein. Es ist **keine Marketingbeschreibung** und **keine Verteidigung des aktuellen Umfangs**. Es ist ein formulierter Vorschlag dafür,

- welche Rolle ein `native-server` im OpenSOVD-Umfeld haben sollte,
- wie er sich von **SOVD-Server**, **SOVD-Gateway** und **Classic Diagnostic Adapter (CDA)** abgrenzt,
- welche Ambiguitäten der SOVD-Spezifikation wir **bewusst** auflösen,
- und auf welchen **kleinen, meinungsstarken Kern** wir die Diskussion zunächst reduzieren.

---

## 2. Ausgangspunkt aus OpenSOVD

Die offiziellen OpenSOVD-Designunterlagen unterscheiden konzeptionell mehrere Bausteine:

- **SOVD Server**
  - zentraler Einstiegspunkt für diagnostische Requests via SOVD
  - implementiert die SOVD-API
  - dispatcht zu Services, DB und Fault Manager

- **SOVD Gateway**
  - leitet SOVD-Requests an passende Backend-Ziele weiter
  - routet zwischen Clients und verteilten SOVD-Komponenten
  - unterstützt Multi-ECU-Kommunikation

- **Classic Diagnostic Adapter (CDA)**
  - übersetzt SOVD-Aufrufe in UDS
  - bindet legacy ECUs an
  - arbeitet mit ODX-beschriebenen ECU-spezifischen Erwartungen
  - übernimmt UDS-nahe Verantwortung

Dazu kommen weitere Plattformbausteine wie Fault Library, Diagnostic Fault Manager, Diagnostic DB, Service Apps und Persistenz.

Der wichtige Punkt ist: **Diese Rollen sind im OpenSOVD-Design getrennt.**
Auch wenn sie in einer konkreten Implementierung in einem Prozess koexistieren können, bleiben sie fachlich unterschiedliche Verantwortlichkeiten.

---

## 3. Leitentscheidung

### 3.1 Zielbild

Der `opensovd-native-server` soll **nicht** als universelles „alles diagnostische in einem Prozess“-Artefakt verstanden werden.

Er soll als **gateway-orientierter SOVD-Server** verstanden werden.

Das heißt:

- Er ist nach außen ein **SOVD-Server**.
- Er darf intern ein **Gateway** enthalten.
- Er ist **nicht** selbst der CDA.
- Er ist **nicht** die kanonische Heimat aller UDS-, Fault-, Persistenz- oder Betriebsfunktionen.

### 3.2 Kurzform

**Der native-server ist der SOVD-Einstiegspunkt eines Systems.**
Er stellt die SOVD-API bereit, erzwingt Sicherheits- und Modellierungsregeln und koordiniert den Zugriff auf diagnostische Backends und Plattformdienste.

Wenn ein System Legacy-ECUs über UDS erreichen muss, geschieht das **über einen CDA** oder eine vergleichbare Adapter-Komponente — nicht dadurch, dass der SOVD-Server selbst fachlich zum UDS-Stack wird.

---

## 4. Klare Rollenabgrenzung

## 4.1 SOVD-Server

### Verantwortung

Der SOVD-Server ist der **öffentliche diagnostische Vertrag** des Systems.
Er ist verantwortlich für:

- Bereitstellung der SOVD-HTTP-API
- Authentifizierung und Autorisierung am Systemrand
- Validierung von Requests und Identifikatoren
- Konsistente Modellierung von Ressourcen, Fehlern und Responses
- Discovery und Sicht auf die verfügbare Diagnosetopologie
- Aufruf von Plattformdiensten und Backends über stabile interne Schnittstellen
- Veröffentlichung des Capability Description File (CDF / OpenAPI)

### Nicht-Verantwortung

Der SOVD-Server ist **nicht** verantwortlich für:

- UDS-Session-Management
- Tester-Present-Semantik
- DoIP- oder andere UDS-Transporte
- ECU-spezifische ODX-Logik
- proprietäre Diagnoseabläufe auf Transport-/Serviceebene

### Konsequenz

Der Server darf UDS-nahe Funktionen **sichtbar machen**, aber nur als klar markierte **Extension**. Sie gehören nicht in die fachliche Definition des Kerns.

---

## 4.2 SOVD-Gateway

### Verantwortung

Das Gateway ist eine **interne Routing- und Dispatch-Schicht**.
Es ist verantwortlich für:

- Zuordnung von Komponenten oder Ressourcen zu Backends
- Weiterleitung von SOVD-Requests an passende Backend-Ziele
- Aggregation über mehrere SOVD-fähige Zielsysteme
- Routing zu CDA, nativen SOVD-Komponenten oder anderen SOVD-konformen Diensten

### Nicht-Verantwortung

Das Gateway ist **nicht** verantwortlich für:

- Definition des öffentlichen Diagnosemodells
- UDS-Übersetzung
- eigene Diagnosefachlichkeit jenseits von Routing und Aggregation

### Konsequenz

Ein Server **kann** ein Gateway enthalten. Das ändert nichts daran, dass Gateway und Server zwei unterschiedliche Rollen bleiben.

---

## 4.3 Classic Diagnostic Adapter (CDA)

### Verantwortung

Der CDA ist die **Kompatibilitätsschicht zu klassischen Diagnoseprotokollen**.
Er ist verantwortlich für:

- Übersetzung von SOVD-Aufrufen in UDS
- Umgang mit ECU-spezifischen Diagnosebeschreibungen, z. B. ODX
- UDS-Sessions, Security Access, Transportparameter und legacy ECU-Kommunikation
- technische Semantik wie Keepalive/Tester Present, soweit sie aus UDS resultiert

### Nicht-Verantwortung

Der CDA ist **nicht** verantwortlich für:

- globale System-Discovery
- plattformweite Authentifizierung und Autorisierung
- zentrale Fault-Aggregation des Gesamtsystems
- das öffentliche Plattformmodell eines SOVD-Servers

### Konsequenz

Wenn eine Funktion eigentlich ausdrückt „sprich UDS mit einer ECU“, dann ist sie **CDA-nah** und nicht Teil des SOVD-Kerns.

---

## 5. Bewusste Auflösung der Spec-Ambiguitäten

ISO 17978 / SOVD ist eine brauchbare Schnittstellenbeschreibung, aber keine vollständige Softwarearchitektur.
Deshalb treffen wir hier explizite Entscheidungen, wo die Spezifikation offen ist.

## 5.1 Ambiguität: Server vs. Gateway

### Entscheidung

Wir unterscheiden fachlich zwischen **SOVD-Server** und **SOVD-Gateway**, auch wenn beides in einer Implementierung zusammen deployt werden kann.

### Begründung

Ohne diese Trennung vermischt sich die Verantwortung für:

- das öffentliche API-Modell
- Routing und Aggregation
- und Backend-spezifische Speziallogik

Die Folge ist eine unscharfe Architektur, in der jede zusätzliche Funktion „irgendwo im Server“ landet.

---

## 5.2 Ambiguität: Gehören `connect` / `disconnect` in den SOVD-Kern?

### Entscheidung

Nein. `connect` und `disconnect` sind **nicht** Teil des SOVD-Kerns.
Wenn solche Funktionen benötigt werden, werden sie ausschließlich als **explizite UDS-/Adapter-Extension** geführt.

### Begründung

Ein allgemeiner SOVD-Server beschreibt Ressourcen und diagnostische Operationen auf SOVD-Ebene. Ein explizites „connecte mich jetzt an die ECU“ ist typischerweise Ausdruck einer darunterliegenden Transport- oder Session-Logik und gehört damit in CDA-/UDS-Nähe.

### Konsequenz für das Repo

Die bestehenden `/sovd/v1/x-uds/components/{component_id}/connect` und `.../disconnect` Endpunkte sind als **Extension** akzeptabel, aber sie dürfen **nicht** als Teil der Kernidentität des Servers dargestellt werden.

---

## 5.3 Ambiguität: Gehören Keepalive / Tester-Present in den Server?

### Entscheidung

Nein. Keepalive- und Tester-Present-Semantik gehören zur **UDS-Session-Verwaltung** und damit fachlich in CDA oder Proxy-nahe Schichten.

### Begründung

Die OpenSOVD-Architektur ordnet UDS-spezifische Verantwortung dem Adapter- bzw. Proxy-Bereich zu. Eine Abfrage wie `/x-uds/diag/keepalive` kann als betriebliche Extension existieren, ist aber **nicht Teil des SOVD-Kerns**.

---

## 5.4 Ambiguität: Wo gehört Fault-Logik hin?

### Entscheidung

Der SOVD-Server stellt Fault-Ressourcen bereit, aber die **Fault-Fachlichkeit** liegt konzeptionell in Fault Library, Diagnostic Fault Manager und Diagnostic DB.

### Begründung

Auch die OpenSOVD-Designunterlagen trennen diese Rollen: Der Server stellt die Sicht nach außen bereit; Fault Library und Fault Manager bilden die plattforminterne Domäne.

### Konsequenz

Ein lokaler `FaultManager` im gleichen Prozess ist für Demo-, Referenz- oder Integrationszwecke zulässig. Er darf aber nicht die Architekturbehauptung erzeugen, der SOVD-Server **sei** der Fault Manager.

---

## 5.5 Ambiguität: Wo gehören Flashing, UCM, Security Access und ähnliche Funktionen hin?

### Entscheidung

Diese Funktionen gehören **nicht in den minimalen SOVD-Kern**.
Sie sind entweder:

- Service-App-Funktionen,
- Plattform-Extensions,
- oder UDS-/OEM-spezifische Erweiterungen.

### Begründung

Solche Funktionen sind stark von OEM-, Fahrzeug- und Plattformkontext abhängig. Für eine erste, klare Referenzarchitektur sind sie zu speziell und erzeugen mehr Diskussion über Randbereiche als über den eigentlichen Kern.

---

## 5.6 Ambiguität: Welche Entity-Typen stehen im Fokus?

### Entscheidung

Der erste, meinungsstarke Kern ist **komponenten-zentriert**.

### Begründung

`components` sind der naheliegendste und am wenigsten interpretationsbedürftige Einstieg. `apps`, `funcs` und `areas` bleiben wichtig, sollen aber nicht den ersten Diskussionskern dominieren.

### Konsequenz

`apps`, `funcs` und `areas` können in Implementierung oder Profilen vorhanden sein, gehören aber **nicht** zur ersten Architekturgeschichte, mit der wir in eine Community-Diskussion gehen.

---

## 5.7 Ambiguität: Entity-ID-Syntax

### Entscheidung

Der Kern akzeptiert standardmäßig nur URL-sichere, konservative IDs; OEM-Profile dürfen einschränken, aber nicht beliebig aufweichen.

### Begründung

Die SOVD-Spezifikation schreibt keine belastbare ID-Syntax fest. Ohne eine klare Entscheidung entsteht unnötige Unsicherheit bei Routing, Logging, Security und Interoperabilität.

---

## 5.8 Ambiguität: Was beweist CDF-/Validator-Compliance?

### Entscheidung

CDF- und Validator-Compliance beweisen **Vertragskonformität**, aber **nicht** automatisch architektonische Stimmigkeit.

### Begründung

Der Validator ist wichtig, aber er beantwortet nicht die Frage, ob Zuständigkeiten sauber geschnitten sind. Deshalb bleibt Architekturklärung eine menschliche Aufgabe.

---

## 6. Der kleinere, meinungsstarke Kern

Für die nächste Diskussion schlagen wir einen **bewusst kleinen Kern** vor.
Nicht alles, was heute im Repo implementiert ist, gehört in diese Kernbeschreibung.

## 6.1 Was zum Kern gehört

### Komponenten-zentrierte SOVD-Grundfunktionen

- Server Info / Discovery
- `components` als primäre Entität
- Capabilities
- Data lesen und schreiben
- Faults listen, lesen, löschen, abonnieren
- Operations und Execution-Status
- Locks
- Modes
- Configurations
- Logs
- OpenAPI/CDF
- Authentifizierung und Autorisierung als Randfunktion des Servers

### Warum genau diese Menge?

Weil sie den eigentlichen öffentlichen Wert des Servers definiert:
Ein Client kann diagnostisch mit einem System über SOVD arbeiten, ohne dass wir schon alle OEM-, UDS- und Plattformsonderfälle in den Kern ziehen.

---

## 6.2 Was ausdrücklich nicht zum Kern gehört

### UDS- und Adapter-Extensions

- `x-uds/*`
- `connect` / `disconnect`
- keepalive / tester present
- raw memory, flash, communication control, DTC setting
- UDS Security Access

### Plattform- und Betriebsfunktionen

- `/x-admin/*`
- Backup/Restore
- Feature Flags
- Prometheus- und Betriebsendpunkte
- systemd, OTLP, DLT, mDNS
- SOME/IP-Anbindung

### Größere Produkt- und OEM-Erweiterungen

- Cloud Bridge
- Multi-Tenant
- RXSWIN
- TARA
- UCM
- VSS Data Catalog
- Compliance-Evidence-Export
- proprietäre OEM-Profile in ihrer konkreten Ausprägung

Diese Funktionen können wertvoll sein. Sie sind nur **nicht** der geeignete erste Referenzkern.

---

## 7. Zielarchitektur

Die Zielarchitektur für die öffentliche Diskussion ist daher nicht „ein großer Server mit allem“, sondern diese Schichtung:

```text
SOVD Client
   │
   ▼
SOVD Server
   - API-Vertrag
   - Auth/AuthZ
   - Resource-Modell
   - CDF
   - Request/Response/Error-Disziplin
   │
   ▼
Gateway-Schicht
   - Routing
   - Aggregation
   - Backend-Zuordnung
   │
   ├── Native SOVD Backends
   ├── CDA
   └── weitere SOVD-konforme Dienste

Plattformdienste außerhalb des Kerns
   - Fault Manager
   - Diagnostic DB / Persistenz
   - Service Apps
   - Logging / Observability
   - OEM-spezifische Policies
```

Wichtig ist: Der Server **koordinert** diese Welt. Er **ist** sie nicht.

---

## 8. Konsequenzen für den heutigen Codebestand

Dieses Papier bedeutet nicht, dass der aktuelle Code „falsch“ ist.
Es bedeutet aber, dass wir ihn künftig **anders erzählen, ordnen und zuschneiden** sollten.

## 8.1 Was künftig anders benannt werden sollte

Der Satz „SOVD Server + Gateway“ ist als Kurzform verständlich, aber zu grob.
Besser ist:

**„gateway-orientierter SOVD-Server mit optionalen UDS- und Plattform-Extensions“**

---

## 8.2 Was in der öffentlichen Diskussion nicht im Vordergrund stehen sollte

Bei einer ersten Architekturvorstellung sollten **nicht** im Zentrum stehen:

- `x-uds`
- Cloud Bridge
- RXSWIN / TARA / UCM
- Backup/Restore und Admin-API
- systemnahe Betriebsfeatures

Diese Themen erzeugen sofort Spezialdiskussionen, bevor die Grundarchitektur geklärt ist.

---

## 8.3 Was als Extensions explizit markiert werden sollte

Folgende Bereiche sollten künftig als **Extensions** oder **Profile** sichtbar gemacht werden:

- UDS / legacy ECU integration
- OEM Profile
- Fleet / Cloud / Multi-Tenant
- Observability und Operations
- Fault- und Persistenzintegration
- Security- und Update-Sonderfunktionen

---

## 9. Was wir damit bewusst nicht behaupten

Dieses Dokument behauptet **nicht**,

- dass nur diese Architektur „richtig“ ist,
- dass das OpenSOVD-Design vollständig festgelegt wäre,
- oder dass der aktuelle Umfang des Repos wertlos sei.

Es behauptet nur:

1. Ohne klare Rollenabgrenzung entsteht unnötige Verwirrung.
2. Für eine produktive Diskussion braucht es einen kleinen, klaren Kern.
3. Der aktuelle Code ist als Exploration nützlich, aber nicht in seiner gesamten Breite als erste Referenz geeignet.

---

## 10. Konkrete nächste Schritte

## 10.1 Dokumentation

Als nächstes sollten wir die öffentliche Erzählung in vier Artefakte aufspalten:

- **Architecture Intent** — dieses Dokument
- **Role Matrix** — Zuordnung jeder Funktion zu Server, Gateway, CDA, Plattformdienst oder Extension
- **Extension Catalog** — explizite Liste aller nicht zum Kern gehörenden Features
- **Spec Decision Log** — dokumentierte Auflösung aller relevanten Ambiguitäten

---

## 10.2 Code- und Repo-Schnitt

Technisch sollte sich der Kern in der Kommunikation zuerst auf diese Bereiche stützen:

- SOVD API-Grundmodell
- Gateway-Routing
- standardnahe Ressourcen
- CDF
- Auth-/Policy-Grenze

Alles andere sollte in Docs, Struktur und später ggf. auch im Code sichtbarer als Erweiterung erscheinen.

---

## 10.3 Community-Vorgehen

Wenn dieses Papier intern trägt, wäre der sinnvolle nächste Schritt **nicht** die Vorstellung eines „fertigen Servers“, sondern die Vorstellung einer **klaren Architekturthese**:

> Ein nativer OpenSOVD-Server sollte als gateway-orientierter SOVD-Server verstanden werden,
> mit klarer Trennung zu CDA, Fault Manager und UDS-spezifischen Extensions.

Damit würden wir nicht nur ein Artefakt zeigen, sondern eine Richtung anbieten.

---

## 11. Quellenbasis für diesen Entwurf

### Öffentliche OpenSOVD-Quellen

- `eclipse-opensovd/opensovd` — `docs/design/design.md`
  - Definitionen von SOVD Server, SOVD Gateway, Classic Diagnostic Adapter, Fault Library und Diagnostic Fault Manager
- `eclipse-opensovd/classic-diagnostic-adapter` — `README.md`
  - Selbstbeschreibung des CDA als Kompatibilitätsbrücke zwischen SOVD und UDS/DoIP

### Lokale Analyse dieses Repos

- `README.md`
- `native-server/src/main.rs`
- `native-core/src/router.rs`
- `native-core/src/http_backend.rs`
- `native-interfaces/src/backend.rs`
- `native-interfaces/src/oem.rs`
- `native-sovd/src/routes.rs`
- `docs/iso-17978-3-compliance-audit.md`
- `docs/adr/A5.1-opensovd-core-architecture-mapping.md`

---

## 12. Ein Satz für die interne Weitergabe

Wenn das für euch sinnvoll ist, übernehme ich mit der AI gern den ersten Aufschlag für die Rollenmatrix, den Extension-Katalog und einen Spec-Decision-Log auf Basis dieses Papiers.

---

## 13. Technische Folgerichtung

Dieses Dokument beschreibt bewusst vor allem **Rolle, Scope und Leitentscheidungen**.
Die konkrete technische Überführung dieser Entscheidungen in eine neue
Rust-Struktur ist im Folgedokument beschrieben:

- [`rust-target-architecture-v2.md`](rust-target-architecture-v2.md)

Dort sind insbesondere ausgearbeitet:

- vorgeschlagene Kernmodule
- capability-orientierte Traits
- `Topology` / `CapabilityRegistry` / `GatewayResolver`
- Trennung von Kern und Extensions
- inkrementelle Migrationsreihenfolge

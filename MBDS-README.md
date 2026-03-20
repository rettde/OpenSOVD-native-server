# MBDS-Proprietary — Mercedes-Benz Diagnostic Server Spezifika

**Vertraulich — nur für internes Mercedes-Benz MBDS-Team**

---

## Inhalt dieses Archivs

| Datei | Beschreibung |
|-------|-------------|
| `oem_mbds.rs` | OEM-Profil für MBDS S-SOVD: EntityIdPolicy, CdfPolicy, AuthPolicy, DiagPolicy |
| `MBDS_CONFORMANCE_AUDIT.md` | MBDS-Konformitätsaudit (15 Code-Änderungen über ISO/ASAM hinaus) |
| `ADR-0001-mbds-specific-adaptations.md` | Architecture Decision Record: Refactoring-Plan für MBDS-Anpassungen |

## Installation

1. `oem_mbds.rs` nach `native-sovd/src/` kopieren
2. `build.rs` erkennt die Datei automatisch (`has_oem_mbds` cfg-Flag)
3. Kein Cargo-Feature nötig — der Build aktiviert das MBDS-Profil von selbst

```bash
cp oem_mbds.rs /path/to/OpenSOVD-native/native-sovd/src/
cargo build -p opensovd-native-server
# → "Using OEM profile: MbdsOemProfile" in der Build-Ausgabe
```

## Was das MBDS-Profil ändert

- **EntityIdPolicy** — Mercedes-spezifische Entity-ID-Validierung
- **CdfPolicy** — Anpassungen am OpenAPI/CDF-Schema
- **AuthPolicy** — Erweiterte Authentifizierungsregeln
- **DiagPolicy** — MBDS-spezifische Diagnose-Workflows

## Ohne MBDS-Profil

Ohne `oem_mbds.rs` nutzt der Server automatisch das `SampleOemProfile` aus
`oem_sample.rs` — eine vollständig dokumentierte Open-Source-Vorlage.

## Zugehöriges Repository

https://github.com/rettde/OpenSOVD-native-server (öffentlich, ohne MBDS-Inhalte)

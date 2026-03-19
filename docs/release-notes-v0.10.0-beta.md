# OpenSOVD-native-server v0.10.0-beta

## Wave 4 — AI-Ready Diagnostic Data (Semantic Layer Enablement)

This release implements the full Wave 4 feature set, enabling SOVD as a structured,
machine-readable data source for AI-assisted vehicle diagnostics.

### Architecture Decisions

- **A4.1 Ontology reference standard** — COVESA VSS as primary semantic reference; `x-vendor.*` prefix for OEM extensions ([ADR](docs/adr/A4.1-ontology-reference-standard.md))
- **A4.2 `DataCatalogProvider` trait** — Pluggable semantic metadata provider with default `StaticDataCatalogProvider`
- **A4.3 Batch export format** — NDJSON (newline-delimited JSON) for ML pipeline compatibility ([ADR](docs/adr/A4.3-batch-export-format.md))

### New Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/components/{id}/snapshot` | Batch diagnostic snapshot (NDJSON) with all signal values + semantic metadata |
| `GET` | `/export/faults` | Fault export (NDJSON) with `?severity=` and `?componentId=` filters |
| `GET` | `/schema/data-catalog` | Full semantic schema introspection across all components |
| `GET` | `/components/{id}/data/subscribe` | SSE stream: data-change + fault-change + keepalive events |

### Data Model Extensions

- **`SovdDataCatalogEntry`** — `normalRange`, `semanticRef` (VSS path), `samplingHint`, `classificationTags`
- **`SovdFault`** — `affectedSubsystem`, `correlatedSignals[]`, `classificationTags[]`

### Enterprise Hardening

- **E4.1** Schema version tracking (`schemaVersion` in all exports)
- **E4.2** Export access control (audit log on every export request)
- **E4.3** Reproducibility metadata (`_meta` NDJSON preamble with `exportTimestamp`, `serverVersion`, `schemaVersion`, `componentFirmwareVersions`)

### Quality

- **312 tests** (up from 295), all passing
- Clippy clean
- 4 schema stability regression tests (T4.1)
- 7 Wave 4 endpoint integration tests

### Dependencies

- New: `async-stream` 0.3 (SSE stream generation)

---

**Full Changelog**: https://github.com/rettde/OpenSOVD-native-server/compare/v0.9.0-beta...v0.10.0-beta

# Architectural Decision Records (ADR)

This directory contains all Architectural Decision Records for the OpenSOVD-native-server project.
ADRs document significant architectural choices, their context, alternatives considered, and consequences.

Format follows [Michael Nygard's ADR template](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions).

---

## Index

### Wave 1 — Foundation & Security

| ID | Title | Status | Key files |
|----|-------|--------|-----------|
| [A1.1](A1.1-graceful-shutdown.md) | Graceful Shutdown | ✅ Accepted | `native-server/src/main.rs` |
| [A1.2](A1.2-health-probes.md) | Health Probes | ✅ Accepted | `native-health/src/lib.rs` |
| [A1.3](A1.3-body-size-limit.md) | Request Body Size Limit | ✅ Accepted | `native-sovd/src/routes.rs` |
| [A1.4](A1.4-config-validation.md) | Startup Configuration Validation | ✅ Accepted | `native-server/src/main.rs` |
| [A1.5](A1.5-appstate-subgroups.md) | AppState Sub-Grouping | ✅ Accepted | `native-sovd/src/state.rs` |
| [A1.6](A1.6-error-catalog.md) | SOVD Error Catalog | ✅ Accepted | `native-interfaces/src/sovd.rs`, `native-sovd/src/routes.rs` |

### Wave 2 — Operational Hardening

| ID | Title | Status | Key files |
|----|-------|--------|-----------|
| [A2.1](A2.1-storage-backend-trait.md) | StorageBackend Trait | ✅ Accepted | `native-interfaces/src/storage.rs` |
| [A2.2](A2.2-backend-trait-diet.md) | ComponentBackend Trait Diet | ✅ Accepted | `native-interfaces/src/backend.rs` |
| [A2.3](A2.3-secrets-abstraction.md) | Secrets Abstraction | ✅ Accepted | `native-interfaces/src/secrets.rs` |
| [A2.4](A2.4-otlp-export.md) | OpenTelemetry OTLP Export | ✅ Accepted | `native-sovd/src/routes.rs`, `native-server/src/main.rs` |
| [A2.5](A2.5-per-client-rate-limiting.md) | Per-Client Rate Limiting | ✅ Accepted | `native-sovd/src/rate_limit.rs` |

### Wave 3 — Cloud Bridge, Multi-Tenant, Variant-Aware

| ID | Title | Status | Key files |
|----|-------|--------|-----------|
| [A3.1](A3.1-cloud-bridge-topology.md) | Cloud Bridge Topology | ✅ Accepted | `native-sovd/src/bridge.rs` |
| [A3.2](A3.2-multi-tenant-isolation.md) | Multi-Tenant Data Isolation | ✅ Accepted | `native-interfaces/src/tenant.rs` |
| [A3.3](A3.3-tenant-context-middleware.md) | TenantContext Middleware | ✅ Accepted | `native-interfaces/src/tenant.rs`, `native-sovd/src/auth.rs` |
| [A3.4](A3.4-bridge-transport-trait.md) | BridgeTransport Trait | ✅ Accepted | `native-interfaces/src/bridge.rs`, `native-sovd/src/bridge.rs` |
| [A3.5](A3.5-api-versioning.md) | API Versioning Contract | ✅ Accepted | `native-sovd/src/routes.rs` |

### Wave 4 — Data Catalog & Batch Export

| ID | Title | Status | Key files |
|----|-------|--------|-----------|
| [A4.1](A4.1-ontology-reference-standard.md) | Ontology Reference Standard (COVESA VSS) | ✅ Accepted | `native-interfaces/src/sovd.rs` |
| [A4.2](A4.2-data-catalog-provider-trait.md) | DataCatalogProvider Trait | ✅ Accepted | `native-interfaces/src/data_catalog.rs`, `native-sovd/src/state.rs` |
| [A4.3](A4.3-batch-export-format.md) | Batch Export Format (NDJSON) | ✅ Accepted | `native-sovd/src/routes.rs` |

### Other

| ID | Title | Status | Key files |
|----|-------|--------|-----------|
| [ADR-0001](ADR-0001-mbds-specific-adaptations.md) | MBDS-Specific Adaptations | ✅ Accepted | `native-sovd/src/oem_mbds.rs` (proprietary, .gitignored) |

---

## Conventions

- **File naming:** `{ID}-{slug}.md` (e.g. `A2.1-storage-backend-trait.md`)
- **ID scheme:** `A{wave}.{seq}` — `A` = Architecture decision, wave number, sequence within wave
- **Status values:** `Proposed` → `Accepted` → `Superseded` / `Deprecated`
- **Template fields:** Status, Date, Deciders, Context, Decision, Alternatives Considered, Consequences, Implementation

## When to write an ADR

Write an ADR when a decision:
1. Affects multiple crates or modules
2. Constrains future implementation choices
3. Has multiple viable alternatives that were evaluated
4. Would be hard to reverse once downstream consumers depend on it

Small, localized implementation choices (e.g. "use `DashMap` vs `HashMap`") do not need an ADR
unless they have cross-cutting implications.

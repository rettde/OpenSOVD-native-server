# Release Notes — OpenSOVD-native-server v0.7.0-beta

**Date:** 2026-03-19
**License:** Apache-2.0
**Rust toolchain:** 1.75+
**Status:** Beta — feature-complete for Wave 1, pending E1.x hardening

---

## Highlights

**Wave 1 is complete.** This beta delivers the first production-targeted release
of the OpenSOVD-native-server with enterprise readiness, full SOVD entity model
(Components, Apps, Funcs), and software-package lifecycle management.

| Metric | Value |
|--------|-------|
| REST API endpoints | 60+ (SOVD) + 9 vendor extensions + 4 operational |
| Automated tests | 230 (0 failures) |
| ISO 17978-3 coverage | 51 mandatory requirements |
| Clippy | Clean (pedantic, `#![deny(warnings)]`) |
| Workspace crates | 7 (`native-interfaces`, `native-core`, `native-sovd`, `native-health`, `native-comm-someip`, `native-server`, `demo-ecu`) |

---

## What's New (since v0.5.0)

### 1. OEM Plugin Interface

Trait-based vendor customization without forking the server.

- **`OemProfile`** supertrait combining `AuthPolicy`, `EntityIdPolicy`,
  `DiscoveryPolicy`, `CdfPolicy`
- **Auto-detection** via `build.rs` — drop an `oem_*.rs` file, get
  `cfg(has_oem_<name>)` at compile time. No Cargo feature flags.
- Proprietary profiles are `.gitignore`d; open-source builds use
  `SampleOemProfile` with full documentation of every customization point
- `DefaultProfile` (permissive, standard SOVD) ships in `native-interfaces`

### 2. Enterprise Readiness (A1.1–A1.6)

| ID | Feature | Detail |
|----|---------|--------|
| A1.1 | **Graceful shutdown** | 10 s connection draining on TLS, audit log flush |
| A1.2 | **Health probes** | `GET /healthz` (liveness), `GET /readyz` (readiness with subsystem checks) |
| A1.3 | **Request limits** | Body size limit + per-endpoint timeout (tower middleware) |
| A1.4 | **Config validation** | Fail-fast at startup: TLS consistency, port range, backend URL reachability |
| A1.5 | **AppState sub-grouping** | `DiagState`, `SecurityState`, `RuntimeState` — keeps the shared state manageable |
| A1.6 | **Typed error taxonomy** | `SovdErrorCode` enum with stable machine-readable codes + HTTP status mapping |

### 3. Full Apps / Funcs Entities — W1.3 (ISO 17978-3 §4.2.3)

Diagnostic applications and cross-component functions are now first-class
SOVD entities with full nested resource routes.

**New types:**
- `SovdApp` — id, name, description, version, status (`Running` / `Stopped` / `Error`)
- `SovdFunc` — id, name, description, sourceComponents
- `EntityBackend` trait — separate from `ComponentBackend`, with default
  implementations for all 12 methods

**New endpoints (11):**

| Method | Path | Description |
|--------|------|-------------|
| GET | `/apps` | List applications (OData pagination) |
| GET | `/apps/{id}` | Get single app |
| GET | `/apps/{id}/capabilities` | App capabilities |
| GET | `/apps/{id}/data` | App data catalog |
| GET | `/apps/{id}/data/{data_id}` | Read app data value |
| GET | `/apps/{id}/operations` | List app operations |
| POST | `/apps/{id}/operations/{op_id}` | Execute app operation |
| GET | `/funcs` | List functions (OData pagination) |
| GET | `/funcs/{id}` | Get single func |
| GET | `/funcs/{id}/data` | Func data catalog |
| GET | `/funcs/{id}/data/{data_id}` | Read func data value |

### 4. Software-Package Lifecycle — W1.4 (SOVD §5.5.10)

Full OTA lifecycle from upload to activation / rollback.

**Extended status model:**
```
Available → Downloading → Downloaded → Installing → Installed → Activated
                                                         ↓
                                                    RollingBack → Failed
```

**New / extended types:**
- `SovdSoftwarePackageStatus` — 8 lifecycle states (was: 4)
- `SovdSoftwarePackage` — new fields: `previousVersion`, `progress` (0–100),
  `componentId`, `updatedAt`, `error`
- `SovdSoftwarePackageManifest` — OTA upload metadata (name, version,
  downloadUrl, checksum, size)

**New backend methods:**
- `activate_software_package` / `rollback_software_package` / `get_software_package_status`
- All with default "not supported" implementations for backward compatibility

**New endpoints (2):**

| Method | Path | Description |
|--------|------|-------------|
| POST | `.../software-packages/{id}/activate` | Activate an installed package |
| POST | `.../software-packages/{id}/rollback` | Rollback to previous version |

**Runtime state:**
- In-memory `package_store` (`DashMap<String, SovdSoftwarePackage>`) for
  real-time progress tracking across activate/rollback operations
- Audit trail entries for all lifecycle actions

### 5. Fine-Grained Authorization — W1.1

- `AuthPolicy` trait on `OemProfile` — vendor-specific token validation,
  scope checks, VIN binding
- Auth middleware delegates all policy decisions to the OEM profile
- Removed hardcoded MBDS scopes/VIN/403 logic from `auth.rs`

### 6. Diagnostic Audit Trail — W1.2

- `AuditLog` — in-memory ring buffer (10 000 entries) + optional JSONL file sink
- `SovdAuditAction` enum for type-safe action classification
- `GET /sovd/v1/audit` endpoint with OData pagination
- Audit entries for: connect, disconnect, writeData, writeConfig,
  installPackage, acquireLock, releaseLock, clearFaults

---

## Architecture

```
Client → SOVD REST API (axum)
              │
              ├── auth_middleware (JWT / API-key / OIDC)
              ├── entity_id_validation_middleware (OemProfile::EntityIdPolicy)
              │
              ├── /sovd/v1/components/* → ComponentBackend (trait object)
              │                              └── ComponentRouter (gateway)
              │                                    ├── SovdHttpBackend (→ CDA)
              │                                    └── SovdHttpBackend (→ demo-ecu)
              │
              ├── /sovd/v1/apps/*       → EntityBackend (trait object)
              ├── /sovd/v1/funcs/*      → EntityBackend (trait object)
              │
              ├── /healthz, /readyz     → HealthMonitor
              ├── /openapi.json         → OpenAPI 3.1 spec
              └── /metrics              → Prometheus
```

### Workspace Crates

| Crate | Purpose |
|-------|---------|
| `native-interfaces` | Shared traits (`ComponentBackend`, `EntityBackend`, `OemProfile`), SOVD types, error definitions |
| `native-core` | `ComponentRouter` (gateway), `AuditLog`, `DiagLog`, `FaultManager`, `LockManager`, `SovdHttpBackend` |
| `native-sovd` | axum router, REST handlers, auth middleware, OEM profiles, OpenAPI, state management |
| `native-health` | `HealthMonitor` (CPU, memory, uptime) |
| `native-comm-someip` | vSomeIP FFI bindings (feature-gated) |
| `native-server` | Binary entry point, config loading, TLS, mDNS |
| `examples/demo-ecu` | Mock ECU backend (BMS + Climate Controller) |

---

## Test Summary

```
native-interfaces   33 tests   ✅
native-core         53 tests   ✅
native-health        6 tests   ✅
native-sovd        137 tests   ✅  (incl. 20 new W1.3/W1.4 tests)
doctests              1 test    ✅
─────────────────────────────────
Total              230 tests   0 failures
```

### New Tests in This Release

**W1.3 Apps/Funcs (14 tests):**
- `list_apps_returns_empty_collection`, `get_app_returns_404_for_unknown`
- `list_app_data_returns_empty_for_unknown`, `list_app_operations_returns_empty`
- `get_app_capabilities_returns_404`, `read_app_data_returns_404`
- `list_funcs_returns_empty_collection`, `get_func_returns_404_for_unknown`
- `list_func_data_returns_empty`, `read_func_data_returns_404`
- `apps_pagination_top_skip`, `funcs_pagination_top_skip`
- `sovd_app_serialization_roundtrip`, `sovd_func_serialization_roundtrip`

**W1.4 Software Packages (6 tests):**
- `activate_software_package_returns_error`
- `rollback_software_package_returns_error`
- `sovd_software_package_extended_fields`
- `sovd_software_package_manifest_roundtrip`
- `sovd_software_package_status_variants`
- `sovd_app_status_variants`

---

## Breaking Changes

None for REST API consumers. Internal trait changes:

| Change | Impact |
|--------|--------|
| `AppState` now requires `entity_backend: Arc<dyn EntityBackend>` | Backend integrators must provide an `EntityBackend` impl (can use `ComponentRouter` which has a default empty impl) |
| `AppState.runtime` now includes `package_store: Arc<DashMap<…>>` | State construction sites must add the field |
| `SovdSoftwarePackage` has new `Option` fields | Fully backward-compatible for serialization (`skip_serializing_if = "Option::is_none"`) |
| `ComponentBackend` has 3 new default methods | No breakage — existing impls get "not supported" defaults |

---

## Known Limitations

- `EntityBackend` default impl returns empty collections — real apps/funcs
  require a backend override (e.g., from CDA or a local registry)
- `package_store` is server-local, not replicated across gateway instances
- `LockManager` is server-local, not forwarded to HTTP backends
- `FaultManager` reads from local store, not from remote backends
- SSE `subscribe_faults` uses delta-detection polling (5 s), not true push
- E1.1–E1.3 hardening (audit hash chain, JSON logging, RED metrics)
  deferred to Wave 2

---

## Upgrade Guide

### From v0.5.0

1. Update `AppState` construction to include `entity_backend` and
   `runtime.package_store`:
   ```rust
   let router = Arc::new(ComponentRouter::new(vec![backend]));
   AppState {
       backend: router.clone(),
       entity_backend: router,  // NEW
       // ...
       runtime: RuntimeState {
           // ...
           package_store: Arc::new(DashMap::new()),  // NEW
       },
   }
   ```

2. If you implement `ComponentBackend`, the 3 new software-package methods
   have default "not supported" implementations — no code change required
   unless you want to support them.

3. OEM profiles: if you have a custom profile, implement the new
   `OemProfile` supertrait (see `oem_sample.rs` for the template).

---

## What's Next — Wave 2

| Track | Items |
|-------|-------|
| **Architecture** | StorageBackend trait, backend trait diet, secrets abstraction, OTLP export |
| **Features** | KPI/system-info, historical storage, fault debouncing, mode/session model |
| **Enterprise** | TLS hot-reload, deployment packaging, backup/restore, feature flags |
| **Testing** | Load tests, fault injection |

See [`docs/integrated-roadmap.md`](integrated-roadmap.md) for full details.

---

*Built with Rust 🦀 • Part of [Eclipse OpenSOVD](https://github.com/eclipse-opensovd)*

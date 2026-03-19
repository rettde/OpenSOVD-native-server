# Changelog

All notable changes to OpenSOVD-native-server are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.8.1] — 2026-03-19

### Cleanup & Documentation Hygiene

### Removed
- **`docs/release-notes-v0.7.0-beta.md`** — v0.7.0 was never released (skipped to v0.8.0)
- **`docs/wave-1-implementation-plan.md`** — completed, superseded by integrated-roadmap
- **`docs/wave-2-implementation-plan.md`** — completed, superseded by integrated-roadmap

### Fixed
- **Stale code comments** — removed references to deleted `LocalUdsBackend`,
  `native-comm-doip`, and `local-uds` feature in `state.rs`, `diag.rs`, `routes.rs`
- **Stale version numbers** — README test count (227→269), demo-ecu serverVersion
  (0.5.0→0.8.1), security-audit/compliance-audit version headers
- **`MBDS_CONFORMANCE_AUDIT.md`** — moved to `.gitignore` (proprietary content)

### Changed
- **`docs/architecture.md`** — major rewrite: removed all references to deleted crates
  (`native-comm-doip`, `native-comm-uds`, `SovdTranslator`, `OtaFlashOrchestrator`),
  updated to reflect current gateway-only architecture with `ComponentRouter` +
  `SovdHttpBackend`. Updated dependency graph, data flow, threading model, config,
  extensions, CDA alignment, and test sections. (782→593 lines)
- **`docs/integrated-roadmap.md`** — status section updated to v0.8.1 / Wave 2 complete
- All version references across docs updated to v0.8.1

---

## [0.8.0] — 2026-03-19

### Wave 2 Complete — Pluggable Infrastructure + Observability

### Architecture
- **A2.1 StorageBackend trait** — pluggable key-value persistence abstraction
  (`native-interfaces/src/storage.rs`). `InMemoryStorage` (BTreeMap) as default
  implementation. 11 unit tests.
- **A2.2 ComponentBackend trait diet** — extracted `ExtendedDiagBackend` to keep
  the core trait minimal; extended diagnostics are opt-in.
- **A2.3 Secrets abstraction layer** — `SecretProvider` trait with `EnvSecretProvider`
  (reads `SOVD_*` env vars) and `StaticSecretProvider` (tests). 8 unit tests.
  (`native-interfaces/src/secrets.rs`)
- **A2.5 Per-client rate limiting** — token-bucket `RateLimiter` (DashMap) with
  Axum middleware integration. Configurable `max_requests` / `window_secs`.
  5 unit tests. (`native-sovd/src/rate_limit.rs`)

### Features
- **W2.1 KPI / system-info endpoint** — `GET /sovd/v1/system-info` aggregates
  health, components, faults, audit chain integrity, and rate limiter stats.
  1 integration test.
- **W2.3 Fault debouncing + FaultGovernor** — DFM-side debounce layer suppresses
  rapid-fire duplicate fault reports within a configurable window. Implements the
  fault-lib design requirement for multi-fault aggregation debouncing in the DFM.
  8 unit tests. (`native-core/src/fault_governor.rs`)
- **W2.4 Richer mode/session model** — `SovdModeDescriptor` with UDS session
  mapping (`udsSession`), display names, descriptions, and security access flags.
  `SovdMode.activeSince` timestamp. Backward-compatible (new fields skip when empty).

### Observability
- **E1.1 Audit log hash chaining** — SHA-256 chain integrity with `verify_chain()`.
  6 new tests.
- **E1.2 Structured JSON logging** — `logging.format = "json"` config option for
  SIEM-ready output with trace correlation (flattened events, target fields).
- **E1.3 RED metrics per endpoint** — `sovd_http_requests_total` counter and
  `sovd_http_request_duration_seconds` histogram with method/path/status labels.
- **A2.4 OpenTelemetry OTLP export** — optional `otlp` feature flag enables
  `tracing-opentelemetry` layer with gRPC OTLP export. Config: `logging.otlp_endpoint`.

### Shared Library Alignment
- **DltLayer → DltTextLayer** rename to clarify this is a lightweight text-format
  fallback for environments without `libdlt`. `TODO(dlt-tracing-lib)` marker for
  future migration to `eclipse-opensovd/dlt-tracing-lib` when it reaches stable Rust.
- **fault_bridge.rs** — `TODO(fault-lib-stable)` marker for migration to
  `eclipse-opensovd/fault-lib` when it drops nightly-only features.
- **fault_governor.rs** — cross-references fault-lib design doc's DFM debouncing
  requirement.

### Testing
- **269 tests** across the workspace (all passing, clippy clean):
  - `native-interfaces` — 52 tests (+19: storage, secrets)
  - `native-core` — 67 tests (+14: fault_governor, audit chain)
  - `native-health` — 6 tests
  - `native-sovd` — 143 tests (+6: system-info, rate limiting)
  - `native-server` — 1 test
- Clippy pedantic clean across entire workspace

---

## [0.6.0] — 2026-03-19

### Wave 1 Complete — Enterprise Readiness + Full Entity Model

### Architecture
- **OEM Plugin Interface** (CDA-inspired trait hierarchy):
  - `OemProfile` supertrait combining `AuthPolicy`, `EntityIdPolicy`, `DiscoveryPolicy`, `CdfPolicy`
  - Defined in `native-interfaces/src/oem.rs`, injected as `Arc<dyn OemProfile>` into `AppState`
  - `DefaultProfile` (permissive, standard SOVD) in `native-interfaces`
  - `SampleOemProfile` template in `native-sovd/src/oem_sample.rs` with extensive documentation
- **Auto-detection of proprietary OEM profiles** via `build.rs`:
  - Any `src/oem_*.rs` file (except `oem_sample.rs`) is auto-detected at compile time
  - Emits `cfg(has_oem_<name>)` — no Cargo feature flags needed
  - Proprietary profiles are `.gitignore`d, open-source builds use `SampleOemProfile`
- **AuthState**: bundles `AuthConfig` + `Arc<dyn OemProfile>` for the auth middleware
- MBDS-specific fields (`required_vin`, `allowed_scopes`) removed from `AuthConfig` → live in OEM profile
- **A1.1 Graceful shutdown** with connection draining (10s grace period on TLS, audit log flush)
- **A1.2 Health probes**: `/healthz` (liveness) + `/readyz` (readiness with subsystem checks)
- **A1.3 Request body size limit + per-endpoint timeout** (already existed, verified)
- **A1.4 Config validation at startup** (fail-fast: TLS consistency, port range, backend URLs)
- **A1.5 AppState sub-grouping**: `DiagState`, `SecurityState`, `RuntimeState` sub-structs
- **A1.6 Typed error taxonomy**: `SovdErrorCode` enum with stable codes, HTTP status mappings

### W1.3 — Full Apps / Funcs Entities (ISO 17978-3 §4.2.3)
- **`SovdApp`** type: id, name, description, version, status (Running/Stopped/Error)
- **`SovdFunc`** type: id, name, description, sourceComponents
- **`EntityBackend` trait** with default implementations for all methods
- 11 new REST endpoints:
  - `GET /apps`, `GET /apps/{id}`, `GET /apps/{id}/capabilities`
  - `GET /apps/{id}/data`, `GET /apps/{id}/data/{data_id}`
  - `GET /apps/{id}/operations`, `POST /apps/{id}/operations/{op_id}`
  - `GET /funcs`, `GET /funcs/{id}`
  - `GET /funcs/{id}/data`, `GET /funcs/{id}/data/{data_id}`
- Full OData pagination support on collection endpoints
- `ComponentRouter` implements `EntityBackend` (default empty — ready for backends to override)

### W1.4 — Software-Package Lifecycle (SOVD §5.5.10)
- Extended `SovdSoftwarePackageStatus`: Available → Downloading → Downloaded → Installing → Installed → Activated → RollingBack → Failed
- Extended `SovdSoftwarePackage` with lifecycle fields: previousVersion, progress, componentId, updatedAt, error
- **`SovdSoftwarePackageManifest`** type for OTA upload metadata
- New backend methods: `activate_software_package`, `rollback_software_package`, `get_software_package_status`
- 2 new REST endpoints: `POST .../software-packages/{id}/activate`, `POST .../software-packages/{id}/rollback`
- In-memory `package_store` (DashMap) for real-time progress tracking
- Audit trail integration for all lifecycle actions

### Refactoring
- `auth_middleware` delegates token validation/claim checks to `AuthPolicy`
- `entity_id_validation_middleware` delegates to `EntityIdPolicy` via closure capture
- `build_openapi_json_with_policy` accepts `&dyn CdfPolicy` for `x-sovd-*` extensions
- `serve_docs` handler extracts `CdfPolicy` from `AppState`
- Removed hardcoded `validate_entity_id()` function
- Removed hardcoded MBDS scopes/VIN/403-status from auth module
- Error helpers refactored to use `SovdErrorCode` enum

### Documentation
- `ADR-0001` updated with implemented architecture, isolation strategy, verification results
- `oem_sample.rs`: step-by-step guide (Copy → Rename → Implement → Register) with
  examples for VIN-binding, scope-ceiling, region-restriction, workshop-ID,
  entity-ID format, CDF extensions, proximity proof, and more
- Integrated roadmap with Wave 4 (AI-Ready Diagnostic Data) based on semantic layer vision

### Testing
- **230 tests** across the workspace (all passing, clippy clean):
  - `native-interfaces` — 33 tests
  - `native-core` — 53 tests
  - `native-health` — 6 tests
  - `native-sovd` — 137 tests (incl. 20 new W1.3/W1.4 tests)
  - 1 doctest
- Clippy pedantic clean across entire workspace

---

## [0.5.0] — 2026-03-14

### Architecture
- **Pure gateway mode**: `ComponentBackend` trait with gateway pattern (`ComponentRouter`)
- **SovdHttpBackend**: proxies SOVD REST requests to external CDA / SOVD backends
- **FaultBridge**: DFM-compatible fault flow from `FaultSink` to `FaultManager`
- **Feature gates**: `persist`, `vsomeip-ffi` for modular builds
- **demo-ecu**: example mock ECU backend (BMS + Climate Controller) for testing the gateway

### ISO 17978-3 Conformance (10.0/10)
- Full OData JSON error envelope (`SovdErrorEnvelope`) per OData §9.4
- `@odata.context`, `@odata.count` on all collection responses
- `$top`, `$skip`, `$filter`, `$orderby`, `$select` query options
- `$metadata` JSON Entity Data Model endpoint
- ETag / `If-None-Match` conditional requests (304 Not Modified)
- `PATCH` partial data update (JSON merge)
- Structured error model with `code`, `message`, `target`, `details`, `innererror`
- Async operation lifecycle: `202 Accepted` + `Location` header + execution store
- SSE fault subscription with delta-detection
- Proximity challenge flow (SOVD §7.8)
- Lock expiry enforcement with background reaper
- Lock conflict `Retry-After` hints

### Security (Phase 7 — Senior Audit)
- `CallerIdentity` extractor: reads `AuthenticatedClient` from JWT/API-key auth middleware, falls back to `x-sovd-client-id` header
- Lock ownership enforcement: only lock owner can release (SOVD §7.4)
- `acquire_lock` derives owner from auth context, not client-spoofable request body
- `release_lock` verifies caller identity before releasing
- Auth middleware: API key (constant-time compare), JWT (HS256/RS256), OIDC (JWKS discovery + caching)
- All auth failures return JSON error bodies (SOVD §5.4)

### Quality
- `SovdFault.display_code` serializes as `displayCode` (camelCase)
- `PartialEq`/`Eq` derives on `SovdDataAccess`, `SovdProximityChallengeStatus`, `SovdLogLevel`
- `#[must_use]` on `SovdErrorEnvelope::new`
- Clippy pedantic clean across entire workspace

### Operational
- OpenAPI 3.1 spec at `GET /openapi.json`
- Prometheus metrics at `GET /metrics`
- Concurrency limiting (200 in-flight requests)
- Request body size limit + timeout middleware
- Configurable CORS origins
- TLS support via `axum-server` + `rustls`

### Testing
- **152 tests** across the workspace (all passing)
  - `native-interfaces` — 33 tests
  - `native-core` — 40 tests
  - `native-health` — 6 tests
  - `native-sovd` — 73 tests

### Known Limitations
- `LockManager` is server-local, not forwarded to HTTP backends
- `FaultManager` reads from local store, not from remote backends
- SSE `subscribe_faults` uses delta-detection polling (5s), not true push
- UDS vendor extensions in `ComponentBackend` trait (under `/x-uds/` prefix)

---

### Removed
- **Standalone mode**: `native-comm-doip`, `native-comm-uds`, `LocalUdsBackend`, `SovdTranslator`, `ota` modules removed
- **`local-uds` feature gate**: no longer needed — server is gateway-only
- DoIP/UDS configuration sections from `opensovd-native-server.toml`

## [0.1.0] — 2025-01-01

- Initial workspace structure matching OpenSOVD CDA patterns
- vSomeIP FFI bindings (`native-comm-someip`)
- Health monitoring (`native-health`)
- Basic SOVD REST API (`native-sovd`)

# Changelog

All notable changes to OpenSOVD-native-server are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.6.0] — 2026-03-18

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

### Refactoring
- `auth_middleware` delegates token validation/claim checks to `AuthPolicy`
- `entity_id_validation_middleware` delegates to `EntityIdPolicy` via closure capture
- `build_openapi_json_with_policy` accepts `&dyn CdfPolicy` for `x-sovd-*` extensions
- `serve_docs` handler extracts `CdfPolicy` from `AppState`
- Removed hardcoded `validate_entity_id()` function
- Removed hardcoded MBDS scopes/VIN/403-status from auth module

### Documentation
- `ADR-0001` updated with implemented architecture, isolation strategy, verification results
- `oem_sample.rs`: step-by-step guide (Copy → Rename → Implement → Register) with
  examples for VIN-binding, scope-ceiling, region-restriction, workshop-ID,
  entity-ID format, CDF extensions, proximity proof, and more

### Testing
- **168 tests** with OEM profile present, **161 tests** without (open-source build)
- New tests in `oem_sample.rs`: `sample_profile_uses_standard_defaults`, `sample_profile_metadata`
- Clippy pedantic clean across entire workspace (both configurations)

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

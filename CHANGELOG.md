# Changelog

All notable changes to OpenSOVD-native-server are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.5.0] — 2026-03-14

### Architecture
- **Backend abstraction**: `ComponentBackend` trait with gateway pattern (`ComponentRouter`)
- **Standard mode**: `SovdHttpBackend` proxies to external CDA via SOVD REST API
- **Standalone mode**: `LocalUdsBackend` wraps embedded UDS/DoIP (feature-gated `local-uds`)
- **FaultBridge**: DFM-compatible fault flow from `FaultSink` to `FaultManager`
- **Feature gates**: `local-uds`, `persist`, `vsomeip-ffi` for modular builds

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
- **227 tests** across the workspace (all passing)
  - `native-interfaces` — 33 tests
  - `native-core` — 75 tests
  - `native-comm-uds` — 40 tests
  - `native-health` — 6 tests
  - `native-sovd` — 73 tests

### Known Limitations
- `LockManager` is server-local, not forwarded to HTTP backends
- `FaultManager` reads from local store, not from remote backends
- SSE `subscribe_faults` uses delta-detection polling (5s), not true push
- UDS vendor extensions in `ComponentBackend` trait (under `/x-uds/` prefix)

---

## [0.1.0] — 2025-01-01

- Initial workspace structure matching OpenSOVD CDA patterns
- UDS communication layer (`native-comm-uds`)
- DoIP communication layer (`native-comm-doip`)
- vSomeIP FFI bindings (`native-comm-someip`)
- Health monitoring (`native-health`)
- Basic SOVD REST API (`native-sovd`)

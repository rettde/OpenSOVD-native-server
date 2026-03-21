# Changelog

All notable changes to OpenSOVD-native-server are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.17.1-rc] — 2026-03-20

### Security
- **RUSTSEC-2026-0044** — `aws-lc-sys` 0.38.0 → 0.39.0 (X.509 Name Constraints bypass)
- **RUSTSEC-2026-0048** — `aws-lc-sys` 0.38.0 → 0.39.0 (CRL scope logic error, severity 7.4)
- **RUSTSEC-2026-0049** — `rustls-webpki` 0.103.9 → 0.103.10 (CRL Distribution Point matching)
- `cargo audit` now **0 vulnerabilities** (3 unmaintained warnings — transitive via `sled`)
- `cargo fmt` enforced across workspace

### Internal
- All 7 workspace crates bumped to **0.17.1-rc**

---

## [0.17.0-rc] — 2026-03-20

### Code Review Fixes
- All critical, medium, and low priority issues from v0.16.0 code review addressed
- **16 new regression tests** covering every review fix (E1–E5, C1–C4, C6, D4, P1–P4)
- **Test coverage:** 81.4% lines / 73.9% functions (484 tests total)

### Internal
- All 7 workspace crates bumped to **0.17.0-rc**
- `cargo-llvm-cov` coverage tooling integrated

---

## [0.16.0] — 2026-03-20

### Security Fixes (Critical)
- **E1 — CSPRNG for UDS Security Access seeds (ISO 14229 §9):** Replaced predictable `SystemTime`-based seed generator (`rand_seed()`) with cryptographically secure `rand::thread_rng().gen()` (OS-level CSPRNG). Previous implementation used truncated nanosecond timestamps, making seeds guessable from response timing. New dependency: `rand = "0.8"`.

### Performance Fixes (Critical)
- **P1 — FIFO eviction for bounded DashMap stores:** Replaced non-deterministic DashMap iteration eviction in `evict_and_insert()` with proper FIFO (insertion-order) eviction using a companion `VecDeque<String>` queue. Affected stores: `execution_store` (operation executions) and `proximity_store` (proximity challenges). Previously, `DashMap::iter().next()` could evict recent entries while keeping stale ones, since DashMap iteration order is shard-dependent and non-deterministic.

### Correctness Fixes (Medium)
- **E2 — Stable ETag hashing:** Replaced `DefaultHasher` (SipHash, unstable across Rust versions) with SHA-256 (truncated to 16 hex chars) for deterministic cross-version ETags in `read_data`. New dependency: `sha2` (workspace).
- **E3 — Compliance evidence reflects actual config:** `compliance_evidence` endpoint now reports `authEnabled` from runtime config instead of hardcoded `true`. Removed misleading `mTlsCapable: true`. Added `auth_enabled: bool` field to `SecurityState`.
- **E4 — Caller identity in disconnect:** `disconnect_component` now extracts `CallerIdentity` from the request instead of hardcoding `"anonymous"` in audit log entries, matching the pattern used in `connect_component`.
- **C2/C3 — Scoped execution lookups:** `get_execution` and `cancel_execution` now verify that the execution belongs to the requested `component_id` and `operation_id`, preventing cross-component data access through guessed execution IDs.

### Performance Fixes (Medium)
- **P2 — Cached OData serialization:** `apply_odata_filter` pre-serializes items once (O(N)) instead of per-comparison. `apply_odata_orderby` pre-extracts sort keys (O(N) serializations) instead of O(N log N × 2) inside the sort closure.
- **P3 — Cached canary env var:** `canary_routing_middleware` now reads `SOVD_DEPLOYMENT_LABEL` once via `OnceLock` instead of calling `std::env::var()` on every request.
- **P4 — No history writes on fault reads:** Removed `guarded_history_fault()` calls from `list_faults` (GET). History is recorded only on state-changing operations (`clear_faults` DELETE), avoiding O(faults × poll_rate) duplicate writes.

### Code Quality (Medium)
- **C1 — Merged component query params:** `list_components` now uses a single `ComponentListParams` struct instead of two separate `Query` extractors (`PaginationParams` + `VariantFilter`), avoiding double query-string parsing.

### Observability (Low)
- **E5 — Audit suppression logging:** `guarded_audit` now emits a `tracing::debug!` event when the audit feature flag is disabled, making suppressed audit-worthy events observable in diagnostic logs instead of silently dropped.

### Code Quality (Low)
- **C4 — Configurable store capacity:** Replaced hardcoded `MAX_ENTRIES = 10_000` in `evict_and_insert` with `RuntimeState.max_store_entries`, allowing operators to tune bounded-store capacity per deployment.
- **C6 — Component validation in proximity GET:** `get_proximity_challenge` now validates that the component in the URL path actually exists instead of ignoring the `_component_id` path segment.
- **D4 — `deny(unsafe_code)` workspace-wide:** Added `[workspace.lints.rust] unsafe_code = "deny"` so all crates reject `unsafe` blocks by default.

### Internal
- All 7 workspace crates bumped to **0.16.0**

---

## [0.15.0] — 2026-03-20

### Phase 3 Future Work — RXSWIN Tracking, TARA, UDS Security Access, UCM Campaigns

#### F15: UNECE R156 RXSWIN-Tracking
- **`RxswinEntry`** — per-component RXSWIN identifier with authority, approval ref, software version mapping
- **`RxswinReport`** — vehicle-level aggregated RXSWIN report (VIN, timestamp, all entries)
- **`UpdateProvenanceEntry`** — update provenance log recording origin and integrity of each software update (UNECE R156 §7.1)
- Endpoints: `GET /sovd/v1/rxswin`, `GET /sovd/v1/rxswin/report`, `GET /sovd/v1/rxswin/{component_id}`, `GET /sovd/v1/update-provenance`
- DashMap-backed RXSWIN store + RwLock provenance log in `RuntimeState`
- **5 tests** (empty collection, report, not found, store+retrieve, provenance empty)

#### F16: ISO/SAE 21434 TARA — Threat Analysis & Risk Assessment
- **`TaraAsset`** — asset inventory entry (category, component IDs, relevance level)
- **`TaraThreatEntry`** — threat entry with STRIDE category, affected assets, residual risk, mitigation, status
- **`TaraExport`** — full TARA export document (ISO/SAE 21434 §15 work product) with summary statistics
- **`TaraThreatStatus`** enum: `identified`, `mitigated`, `accepted`, `transferred`
- Endpoints: `GET /sovd/v1/tara/assets`, `GET /sovd/v1/tara/threats`, `GET /sovd/v1/tara/export`
- **5 tests** (empty assets, empty threats, empty export, populated export, status variants)

#### F17: ISO 14229 UDS Security Access (0x27)
- **`UdsSecurityLevel`** — security level descriptor with protected services list
- **`UdsSecurityAccessRequest`** / **`UdsSecurityAccessResponse`** — seed/key protocol types
- Standard 3-level security model: Workshop (0x01), Engineering (0x03), OEM (0x05)
- Endpoints: `GET /sovd/v1/x-uds/components/{id}/security-levels`, `POST /sovd/v1/x-uds/components/{id}/security-access`
- Audit trail integration for both requestSeed and sendKey phases
- **7 tests** (list levels, 404, requestSeed, sendKey granted, sendKey denied, invalid phase, serialization)

#### F18: AUTOSAR UCM — Update Campaign Manager
- **`UcmCampaign`** — campaign with lifecycle status, target components, progress, transfer states
- **`UcmCampaignStatus`** enum: 9 states (created → transferring → processing → activating → activated; rollingBack → rolledBack; failed, cancelled)
- **`UcmTransferState`** / **`UcmTransferPhase`** — per-component transfer tracking (9 phases)
- Endpoints: `GET/POST /sovd/v1/ucm/campaigns`, `GET /sovd/v1/ucm/campaigns/{id}`, `POST .../execute`, `POST .../rollback`
- Full lifecycle orchestration: create → execute (sets processing + transfer states) → rollback
- Audit trail integration for campaign create, execute, rollback
- **8 tests** (empty, create, empty targets rejected, lifecycle, not found, execute not found, status variants, transfer phase variants)

### Fixes
- **MSRV corrected:** `rust-version` bumped from `1.75` to `1.88` — matches actual dependency floor (`time 0.3.47` requires 1.88.0). README badge and prerequisites updated.
- **Cross-compilation fix:** Removed global `target-cpu=native` from `.cargo/config.toml` — caused `Illegal Instruction` crashes when cross-compiled binaries were deployed on ARM targets (DRIVE AGX, AAOS IVI). CPU tuning is now per-target (`cortex-a78ae` for Orin, `cortex-a76` for AAOS).

### Deployment (F19)
- **`deploy/opensovd-native.service`** — production-ready systemd unit with `Type=notify`, `WatchdogSec=30`, resource limits (`MemoryMax=256M`, `CPUQuota=50%`), security hardening (`NoNewPrivileges`, `ProtectSystem=strict`)
- **`systemd` feature flag** (`--features systemd`) — enables sd_notify integration:
  - `READY=1` sent after listener socket is bound
  - `WATCHDOG=1` heartbeat at `WatchdogSec/2` interval (reads `WATCHDOG_USEC` from environment)
  - `STOPPING=1` sent on graceful shutdown
  - Without feature or on non-Linux: all calls are compile-time no-ops
- New module: `native-server/src/watchdog.rs`
- New dependency: `sd-notify = "0.4"` (optional, workspace)

### OpenAPI CDF (Capability Description File)
- F15-F18 paths added to `openapi.rs`: `/rxswin`, `/rxswin/report`, `/rxswin/{component_id}`, `/update-provenance`, `/tara/assets`, `/tara/threats`, `/tara/export`, `/x-uds/components/{component_id}/security-levels`, `/x-uds/components/{component_id}/security-access`, `/ucm/campaigns`, `/ucm/campaigns/{campaign_id}`, `/ucm/campaigns/{campaign_id}/execute`, `/ucm/campaigns/{campaign_id}/rollback`
- 10 new schemas: `RxswinEntry`, `RxswinReport`, `UpdateProvenanceEntry`, `TaraAsset`, `TaraThreatEntry`, `TaraExport`, `UdsSecurityLevel`, `UdsSecurityAccessRequest`, `UdsSecurityAccessResponse`, `UcmCampaign`
- 4 new tags: `RXSWIN`, `TARA`, `UDS-Security`, `UCM`
- **7 new CDF contract tests** (F15 paths, F16 paths, F17 paths+requestBody, F18 paths+methods, F15-F18 schemas, F15-F18 tags)

### CI Hardening
- **`build-systemd`** job — build + clippy with `--features systemd`
- **`msrv`** job — `cargo check` with Rust 1.88.0 toolchain
- **`cross-check`** job — `cargo check --target aarch64-unknown-linux-gnu` with cross-linker

### Documentation
- **README** endpoint table updated: +5 core endpoints (system-info, audit, audit/export, compliance-evidence, openapi.json), +5 software packages, +4 RXSWIN, +3 TARA, +2 UDS Security, +5 UCM
- **README** feature flags table: `systemd` feature added
- **[HowTo: Deploy on NVIDIA DRIVE AGX](docs/howto-deploy-nvidia-drive.md)** — cross-compilation, Docker container, systemd service, in-vehicle config
- **[HowTo: Deploy on AAOS IVI](docs/howto-deploy-aaos.md)** — NDK toolchain, ADB deployment, Android init `.rc` service, SELinux policy, Vendor APEX

### Stats
- **476 tests** (workspace), all passing
- Clippy pedantic clean (workspace)
- All 7 crates at 0.15.0

---

## [0.13.0-beta] — 2026-03-20

### Phase 2 Future Work — Prometheus Config, Vault Secrets, WebSocket Bridge, Firmware Signing, mTLS

#### F7: Prometheus Scrape Endpoint (config-gated)
- `MetricsConfig` struct (`enabled`, `path`) in `AppConfig` — gates `/metrics` endpoint via config
- `build_router` accepts `metrics_enabled` parameter; disabled config returns 404 on `/metrics`

#### F4: HashiCorp Vault Secret Provider (`vault` feature)
- **`VaultSecretProvider`** — `SecretProvider` implementation using Vault KV v2 HTTP API
- Time-based cache with configurable TTL (default 5 min), invalidation support
- `SecretsConfig` in `AppConfig`: `provider` selector (`env`/`vault`/`static`), Vault address, mount, prefix
- Auto-populates `auth.jwt_secret` and `auth.api_key` from Vault at startup
- Falls back to `VAULT_TOKEN` env var if no token in config
- **10 unit tests** (cache, TTL expiry, invalidation, unreachable server, defaults)

#### F3: WebSocket Bridge Transport (`ws-bridge` feature)
- **`WsBridgeTransport`** — `BridgeTransport` implementation using `tokio-tungstenite`
- Full WebSocket cloud↔vehicle tunnel: handshake, request forwarding, heartbeat (Ping/Pong), disconnect
- Per-session read/write task pair with mpsc channels and oneshot response correlation
- `start_accept_loop()` binds TCP listener, upgrades to WS, registers sessions
- When `ws-bridge` feature enabled, bridge mode auto-selects WS transport over in-memory
- **8 integration tests** (connect, roundtrip, heartbeat, disconnect, multiple vehicles, unknown session)

#### F12: SW Package Signature Verification (ISO 24089)
- **`FirmwareVerifier`** trait in `native-interfaces` with `verify()` and `algorithm()` methods
- **`Ed25519Verifier`** — production implementation using `ed25519-dalek` (from hex or raw bytes)
- **`NoopVerifier`** — passthrough for testing / unsigned packages
- `VerificationResult` struct: `valid`, `digest` (SHA-256 hex), `detail`
- Signature gate in `activate_software_package` handler — blocks activation when verifier is active
- `FirmwareConfig` in server TOML: `verify` flag + `public_key_hex` (32-byte Ed25519)
- `signature` field added to `SovdSoftwarePackageManifest`
- **11 unit tests** (valid sig, invalid sig, tampered payload, wrong key, malformed sig, hex parsing, noop, digest, serialization)

#### F14: mTLS Backend Connections
- `client_cert_path` / `client_key_path` on `SovdHttpBackendConfig` — PEM client identity for Gateway → CDA
- `ca_cert_path` — pinned CA certificate for server verification
- `danger_accept_invalid_certs` — development-only flag for self-signed certs
- reqwest `Identity::from_pem()` + `Certificate::from_pem()` wiring in `SovdHttpBackend::new()`
- **4 unit tests** (missing cert, missing CA, danger flag, default no-mTLS)

#### HowTo Guides (Out of Scope features)
- `docs/howto-f5-e2e-test-suite.md` — Testcontainers + CDA + demo-ecu
- `docs/howto-f8-someip-real-transport.md` — vsomeip build, config, validation
- `docs/howto-f15-oidc-e2e-validation.md` — Keycloak realm setup, token flow

### Stats
- **433 tests** (workspace), all passing
- Clippy clean (workspace)

---

## [0.11.0] — 2026-03-20

### Phase 1 Future Work — Persistent Storage, OTLP Tracing, SBOM

#### F1: Persistent Storage (`persist` feature)
- **`SledStorage`** — `StorageBackend` implementation using embedded `sled` key-value database
- Feature-gated behind `persist` Cargo feature (`native-core/persist`, `native-server/persist`)
- `StorageConfig` in `AppConfig` with `backend` selector (`memory` / `sled`) and `sled_path` option
- Wired into `HistoryService` in `main()` — selects backend at startup based on config
- **13 unit tests** covering put/get/delete/list/count/flush/reopen scenarios

#### F2: OTLP Distributed Tracing (`otlp` feature)
- Fixed `opentelemetry_sdk` 0.27 API: `TracerProvider` builder, `with_simple_exporter`, trait imports
- Restructured tracing init with macro-based OTLP layer to avoid type unification across JSON/plain branches
- **`#[tracing::instrument]`** on 11 key route handlers: `list_components`, `get_component`, `list_data`, `read_data`, `write_data`, `list_faults`, `clear_faults`, `execute_operation`, `acquire_lock`, `release_lock`, `export_faults`
- Docker Compose observability stack: `deploy/docker-compose.observability.yml` (Jaeger all-in-one)

#### F6: SBOM / Supply Chain
- CI job: `cargo-cyclonedx` generates CycloneDX JSON SBOM artifact (UNECE R156 / ISO 24089)

### Stats
- **411 tests** (398 base + 13 sled), all passing
- Clippy clean (workspace + `--features otlp` + `--features persist`)

---

## [0.10.1] — 2026-03-20

### Enterprise Hardening Audit — Feature-Flag Integration & Gap Fixes

#### Feature Flags Wired Into All Handlers (E2.4)
- **`guarded_audit()` helper** — all 15+ audit recording call sites now check the `audit` feature flag before writing; flag-admin endpoint always audits (chicken-and-egg safe)
- **`guarded_history_fault()` helper** — all fault-to-history recording gated by `history` feature flag
- **Audit → History forwarding** — every audit entry is automatically forwarded to `HistoryService` when both `audit` and `history` flags are enabled

#### Historical Diagnostic Storage Completeness (W2.2)
- **Audit entries in HistoryService** — `record_audit()` now called on every audit write (was missing)
- **Live fault export → history** — `export_faults` live path records faults to history (was missing)
- **Background compaction task** — `tokio::spawn` interval task runs every 6 hours, prunes entries older than `retention_days` (was manual-only)

#### Backup/Restore Integration Tests (E2.3)
- `backup_returns_json_snapshot` — validates snapshot schema (version, created_at, faults, audit_entries)
- `restore_invalid_json_returns_400` — corrupted input handling

#### Feature Flag Integration Tests (E2.4)
- `feature_flags_list_returns_default_flags` — verifies 4+ default flags
- `feature_flag_disable_audit_suppresses_recording` — proves audit suppression
- `feature_flag_disable_history_suppresses_recording` — proves history suppression
- `feature_flag_set_toggle_via_admin_api` — admin API toggle round-trip
- `feature_flag_unknown_flag_returns_404` — error handling

#### Code Quality
- Clippy warnings fixed: `map_or` → `is_none_or`, `sort` → `sort_unstable`, underscore-prefixed bindings, inlined format args
- All `unwrap()` calls on fallible paths replaced with proper error handling

### Stats
- **398 tests** (up from 391), all passing
- Clippy clean across entire workspace
- All roadmap items (Waves 1–4) now fully complete — no open items

---

## [0.10.0] — 2026-03-20

### Wave 4 — Data Catalog & Batch Export

### Architecture Decisions
- **A4.1 Ontology reference standard** — ADR: COVESA VSS as primary semantic reference, `x-vendor.*` prefix for OEM extensions
- **A4.2 `DataCatalogProvider` trait** — Pluggable semantic metadata provider with `StaticDataCatalogProvider` default impl (6 tests)
- **A4.3 Batch export format** — ADR: NDJSON (newline-delimited JSON) as primary export format

### Features
- **W4.1 Semantic metadata on data catalog** — `SovdDataCatalogEntry` extended with `normalRange`, `semanticRef` (VSS path), `samplingHint`, `classificationTags`
- **W4.2 Batch diagnostic snapshot** — `GET /components/{id}/snapshot` returns all signal values + metadata as NDJSON; `GET /export/faults` for fault export with severity filtering
- **W4.3 Fault ontology enrichment** — `SovdFault` extended with `affectedSubsystem`, `correlatedSignals[]`, `classificationTags[]` for structured fault correlation
- **W4.4 Schema introspection** — `GET /schema/data-catalog` returns full semantic schema across all components (COVESA VSS ontology reference)
- **W4.5 SSE data-change stream** — `GET /components/{id}/data/subscribe` provides real-time data-change + fault-change + keepalive events via Server-Sent Events

### Enterprise Hardening
- **E4.1 Data contract versioning** — `schemaVersion` field in `DataCatalogProvider` trait and all export preambles
- **E4.2 Export access control** — All export endpoints (`/snapshot`, `/export/faults`) audited via `AuditLog`
- **E4.3 Reproducibility metadata** — NDJSON `_meta` preamble with `exportTimestamp`, `serverVersion`, `schemaVersion`, `componentFirmwareVersions`

### Test Infrastructure
- **T4.1 Schema stability regression tests** — 4 tests verifying `SovdDataCatalogEntry` and `SovdFault` JSON shapes (camelCase, optional field omission)
- **7 Wave 4 endpoint integration tests** — snapshot NDJSON, fault export, severity filter, schema introspection, SSE subscribe, 404 handling

### Stats
- **312 tests** (up from 295), all passing
- Clippy clean
- New dependency: `async-stream` 0.3 (SSE stream generation)

---

## [0.9.0] — 2026-03-20

### Wave 3 — Cloud Bridge, Multi-Tenant, Variant-Aware, Zero-Trust

### Architecture Decisions
- **A3.1 Cloud bridge topology** — ADR: feature-gated same-binary approach
  (`docs/adr/A3.1-cloud-bridge-topology.md`)
- **A3.2 Multi-tenant data isolation** — ADR: policy + namespace isolation
  (`docs/adr/A3.2-multi-tenant-isolation.md`)
- **A3.3 TenantContext middleware** — `TenantContext` struct, `TenantIsolation`
  enum, `MultiTenantConfig` in `native-interfaces/src/tenant.rs`. JWT `tenant_id`
  claim extraction in auth middleware. `TenantId` extractor in routes.
- **A3.4 BridgeTransport trait** — async trait for cloud-to-vehicle communication
  with `BridgeSession`, `BridgeError`, `BridgeConfig` (`native-interfaces/src/bridge.rs`)
- **A3.5 API versioning contract** — ADR: URL prefix versioning, deprecation policy
  (`docs/adr/A3.5-api-versioning.md`)

### Features
- **W3.1 Cloud bridge mode** — `InMemoryBridgeTransport` implementation,
  bridge REST API at `/sovd/v1/x-bridge/` (sessions, forward, heartbeat, disconnect,
  status). Wired into `main.rs` with `bridge` config section. (`native-sovd/src/bridge.rs`)
- **W3.2 Multi-tenant fleet model** — `TenantId` extractor, tenant-scoped audit
  logging via `scoped_key()`, `MultiTenantConfig` in server config.
- **W3.3 Variant-aware discovery** — `SovdComponent` extended with `softwareVersion`,
  `hardwareVariant`, `installationVariant` fields. `VariantFilter` query params
  on `GET /components` (`?variant=premium&softwareVersion=2.1.0`).
- **W3.4 Zero-trust hardening** — Signed audit export endpoint
  (`GET /audit/export`) with hash chain integrity proof. mTLS already in place
  from Wave 1.

### Enterprise Readiness
- **E3.1 Client SDK generation** — `scripts/generate-sdk.sh` shell script using
  openapi-generator-cli (or Docker fallback). Supports Python, TypeScript, Rust,
  Java, Go, C#.
- **E3.2 Compliance evidence export** — `GET /compliance-evidence` endpoint
  returning aggregated security posture, audit integrity, component status, and
  regulatory compliance evidence (ISO 17978-3, UNECE R155, ISO 27001).
- **E3.3 Canary/blue-green routing** — `canary_routing_middleware` reads
  `X-Deployment-Target` header; returns 421 if mismatched. Response includes
  `X-Served-By` header. Deployment label via `SOVD_DEPLOYMENT_LABEL` env var.

### Tests
- **295 tests** (up from 269): +8 bridge transport tests, +15 tenant context tests,
  +3 variant/compliance tests. All pass, clippy clean.

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
- Integrated roadmap with Wave 4 (Data Catalog & Batch Export) planning

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

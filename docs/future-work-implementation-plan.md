# Future Work — Implementation Plan

**Date:** 2026-03-20
**Scope:** F1–F8 enhancement items from [integrated-roadmap.md](integrated-roadmap.md)
**Prerequisite:** All Waves 1–4 complete, 398 tests, ISO 17978-3 conformant

---

## Overview

| ID | Item | Effort | Phase | Depends on |
|----|------|--------|-------|------------|
| F1 | Persistent storage (sled) | M | 1 | — |
| F2 | OTLP tracing | S | 1 | — |
| F3 | WebSocket bridge | L | 2 | F1 |
| F4 | Vault integration | M | 2 | — |
| F5 | E2E test suite | L | 3 | F1, F3 |
| F6 | SBOM / supply chain | S | 1 | — |
| F7 | Prometheus scrape endpoint | S | 2 | — |
| F8 | SOME/IP real transport | L | 3 | — (hardware) |

**Effort key:** S = 1–2 days, M = 3–5 days, L = 1–2 weeks

---

## Phase 1 — Foundation (no external dependencies)

### F1 — Persistent Storage (`SledStorage`)

**Goal:** Fault, audit, and history data survive server restarts.

**Current state:**
- `StorageBackend` trait exists (`native-interfaces/src/storage.rs`) with `get`, `put`, `delete`, `list_keys`, `list`, `count`, `flush`
- `InMemoryStorage` (BTreeMap) is the only implementation
- `sled = "0.34"` is already in workspace `Cargo.toml`
- `HistoryService`, `AuditLog`, `FaultManager` all use `StorageBackend`

**Implementation steps:**

1. **Create `native-core/src/sled_storage.rs`**
   - Implement `StorageBackend` for `sled::Db`
   - `get` → `sled::Tree::get`
   - `put` → `sled::Tree::insert`
   - `delete` → `sled::Tree::remove`
   - `list_keys` / `list` → `sled::Tree::scan_prefix`
   - `flush` → `sled::Db::flush`
   - Constructor: `SledStorage::open(path: &Path) -> Result<Self, sled::Error>`

2. **Feature-gate in `native-core/Cargo.toml`**
   ```toml
   [features]
   persist = ["sled"]

   [dependencies]
   sled = { workspace = true, optional = true }
   ```

3. **Config extension in `native-server/src/main.rs`**
   ```toml
   [storage]
   backend = "sled"        # "memory" (default) | "sled"
   sled_path = "./data/sovd.sled"
   ```

4. **Wire into `AppState` construction**
   - If `backend = "sled"` → `Arc::new(SledStorage::open(...))`
   - Else → `Arc::new(InMemoryStorage::new())`
   - Pass to `HistoryService`, `AuditLog`, etc.

5. **Tests**
   - Unit: open/close, put/get, prefix scan, flush, reopen-and-read
   - Integration: start server with sled, write faults, restart, verify faults persist
   - Use `tempfile::TempDir` for test isolation

**Acceptance criteria:**
- [ ] `cargo test --workspace --features persist` passes
- [ ] Server starts with `backend = "sled"`, writes data, restarts, data is still there
- [ ] `backend = "memory"` remains the default (no behavior change)
- [ ] Existing 398 tests still pass without the feature

**Effort:** 3–5 days

---

### F2 — OTLP Tracing

**Goal:** Distributed tracing via OpenTelemetry to Jaeger, Tempo, or Grafana Cloud.

**Current state:**
- `otlp` feature flag exists in `native-server/Cargo.toml`
- `LoggingConfig.otlp_endpoint` field exists
- OpenTelemetry crates are in workspace `Cargo.toml`
- OTLP layer wiring exists in `main.rs` (behind `#[cfg(feature = "otlp")]`)

**Implementation steps:**

1. **Verify existing wiring compiles**
   ```bash
   cargo build -p opensovd-native-server --features otlp
   ```

2. **Add span instrumentation to key handlers**
   - `#[tracing::instrument]` on route handlers in `native-sovd/src/routes.rs`
   - Propagate `trace_id` into audit log entries (field already exists)
   - Add `otel.status_code` attribute on error paths

3. **Docker Compose dev stack**
   - Create `deploy/docker-compose.observability.yml`
   - Services: opensovd-native-server + Jaeger (all-in-one) or Grafana Tempo
   - Pre-configured `SOVD__LOGGING__OTLP_ENDPOINT=http://jaeger:4317`

4. **Documentation**
   - Add "Observability" section to README with setup instructions
   - Document environment variables and config options

**Acceptance criteria:**
- [ ] `cargo build --features otlp` compiles without warnings
- [ ] `docker compose -f deploy/docker-compose.observability.yml up` shows traces in Jaeger UI
- [ ] Trace spans visible for: request lifecycle, backend forwarding, fault operations

**Effort:** 1–2 days

---

### F6 — SBOM / Supply Chain

**Goal:** Generate Software Bill of Materials for UNECE R156 / ISO 24089 compliance.

**Current state:**
- No SBOM generation in CI
- `Cargo.lock` is committed (provides dependency list)

**Implementation steps:**

1. **Add CI job to `.github/workflows/ci.yml`**
   ```yaml
   sbom:
     name: SBOM (CycloneDX)
     runs-on: ubuntu-latest
     steps:
       - uses: actions/checkout@v4
       - uses: dtolnay/rust-toolchain@stable
       - name: Install cargo-cyclonedx
         run: cargo install cargo-cyclonedx
       - name: Generate SBOM
         run: cargo cyclonedx --format json --all
       - name: Upload SBOM artifact
         uses: actions/upload-artifact@v4
         with:
           name: sbom-cyclonedx
           path: "**/bom.json"
   ```

2. **Add `.sbom/` to `.gitignore`** (local generation artifacts)

3. **Document in README** — brief note under CI section

**Acceptance criteria:**
- [ ] CI generates valid CycloneDX JSON for all workspace crates
- [ ] SBOM artifact downloadable from GitHub Actions

**Effort:** 1 day

---

## Phase 2 — Production Readiness

### F3 — WebSocket Bridge (`WsBridgeTransport`)

**Goal:** Real cloud↔vehicle tunneling over WebSocket, replacing `InMemoryBridgeTransport`.

**Current state:**
- `BridgeTransport` trait exists (`native-interfaces/src/bridge.rs`) with `accept_remote`, `forward_to_vehicle`, `heartbeat`, `disconnect`, `active_sessions`
- `InMemoryBridgeTransport` in `native-sovd/src/bridge.rs` (in-process stub)
- `BridgeConfig` with `enabled`, `listen_addr`, `max_sessions`, `heartbeat_interval_secs`
- Bridge REST API routes already mounted at `/sovd/v1/x-bridge/*`

**Implementation steps:**

1. **Add dependencies to workspace `Cargo.toml`**
   ```toml
   tokio-tungstenite = "0.24"
   ```

2. **Create `native-core/src/ws_bridge.rs`**
   - `WsBridgeTransport` struct holding `tokio::sync::RwLock<HashMap<String, WsSession>>`
   - `accept_remote()` — accept WebSocket connection on `listen_addr`, register session
   - `forward_to_vehicle()` — serialize `SovdBridgeRequest` as JSON, send over WS, await response
   - `heartbeat()` — send WS ping, await pong within timeout
   - `disconnect()` — send close frame, remove session
   - TLS: reuse existing `rustls` config for WS over TLS (wss://)

3. **Config extension**
   ```toml
   [bridge]
   enabled = true
   transport = "websocket"   # "memory" (default) | "websocket"
   listen_addr = "0.0.0.0:8443"
   ```

4. **Wire into `main.rs`**
   - If `transport = "websocket"` → `WsBridgeTransport::new(config)`
   - Else → `InMemoryBridgeTransport::new()`
   - Spawn accept loop as background task

5. **Tests**
   - Unit: connect/disconnect, message round-trip, heartbeat timeout
   - Integration: two server instances, one cloud + one vehicle, forward a SOVD request

**Acceptance criteria:**
- [ ] Two server instances communicate over WebSocket
- [ ] Heartbeat detects dead connections within 2× interval
- [ ] TLS (wss://) works with existing cert config
- [ ] `transport = "memory"` remains default

**Effort:** 1–2 weeks

---

### F4 — Vault Integration (`VaultSecretProvider`)

**Goal:** Retrieve secrets from HashiCorp Vault instead of environment variables.

**Current state:**
- `SecretProvider` trait exists (`native-interfaces/src/secrets.rs`) with `get_secret(name)`, `has_secret(name)`
- `EnvSecretProvider` (reads `SOVD_*` env vars) — default
- `StaticSecretProvider` — for tests
- Secrets used for: JWT signing key, API key, TLS passwords

**Implementation steps:**

1. **Add optional dependency**
   ```toml
   # Cargo.toml (workspace)
   vaultrs = { version = "0.7", optional = true }
   ```

2. **Create `native-core/src/vault_provider.rs`**
   - `VaultSecretProvider` struct with `vaultrs::client::VaultClient`
   - Constructor: `VaultSecretProvider::new(vault_addr, token_or_role, mount, path_prefix)`
   - `get_secret("jwt_secret")` → `vault kv get secret/sovd/jwt_secret`
   - Cache with TTL (e.g. 5 min) to avoid per-request Vault calls
   - Fallback: log warning, return `None` (fail-open for non-critical, fail-closed for auth secrets)

3. **Config extension**
   ```toml
   [secrets]
   provider = "vault"       # "env" (default) | "vault" | "static"
   vault_addr = "http://vault:8200"
   vault_mount = "secret"
   vault_path_prefix = "sovd/"
   # Auth: VAULT_TOKEN env var or AppRole
   ```

4. **Feature-gate** — `vault` feature in `native-core/Cargo.toml`

5. **Tests**
   - Unit: mock Vault HTTP responses, verify get/has/cache/TTL
   - Integration (optional): `docker compose` with Vault dev server

**Acceptance criteria:**
- [ ] `cargo build --features vault` compiles
- [ ] Server reads JWT secret from Vault at startup
- [ ] Cache prevents per-request Vault roundtrips
- [ ] `provider = "env"` remains default

**Effort:** 3–5 days

---

### F7 — Prometheus Scrape Endpoint

**Goal:** Expose `/metrics` for Prometheus pull-based monitoring.

**Current state:**
- `metrics` (0.24) and `metrics-exporter-prometheus` (0.16) are in workspace deps
- RED metrics recorded in `red_metrics_middleware` (`routes.rs`): `sovd_http_requests_total`, `sovd_http_request_duration_seconds`
- No scrape endpoint exposed yet

**Implementation steps:**

1. **Initialize Prometheus exporter in `main.rs`**
   ```rust
   let builder = metrics_exporter_prometheus::PrometheusBuilder::new();
   let handle = builder.install_recorder().expect("prometheus recorder");
   ```

2. **Add `/metrics` route in `routes.rs`**
   ```rust
   .route("/metrics", get(move || async move { handle.render() }))
   ```

3. **Config**
   ```toml
   [metrics]
   enabled = true       # default: false
   path = "/metrics"    # default
   ```

4. **Tests**
   - `GET /metrics` returns `text/plain` with Prometheus format
   - Contains `sovd_http_requests_total` and `sovd_http_request_duration_seconds` lines

**Acceptance criteria:**
- [ ] `curl localhost:8080/metrics` returns valid Prometheus text format
- [ ] Grafana dashboard can scrape and display request rate + latency
- [ ] Endpoint disabled by default

**Effort:** 1–2 days

---

## Phase 3 — Validation & Hardening

### F5 — E2E Test Suite

**Goal:** Full round-trip tests through the SOVD gateway with real backend processes.

**Current state:**
- 398 unit/integration tests using mock backends
- `demo-ecu` example exists as a standalone binary
- No process-level E2E tests

**Implementation steps:**

1. **Add `tests/e2e/` directory at workspace root**

2. **Add testcontainers dependency**
   ```toml
   [dev-dependencies]
   testcontainers = "0.20"
   ```

3. **Create E2E test scenarios**
   - `e2e_gateway_roundtrip` — start demo-ecu + SOVD server, query components/data/faults through gateway
   - `e2e_fault_lifecycle` — inject fault via demo-ecu, read via SOVD, clear, verify
   - `e2e_auth_required` — start with `auth.enabled = true`, verify 401 without token, 200 with token
   - `e2e_tls_handshake` — start with TLS config, verify HTTPS works, HTTP rejected
   - `e2e_persistence_restart` (requires F1) — write data, stop server, restart, verify data
   - `e2e_bridge_tunnel` (requires F3) — cloud→vehicle round-trip over WebSocket

4. **CI integration**
   ```yaml
   e2e:
     name: E2E Tests
     runs-on: ubuntu-latest
     needs: [build]
     steps:
       - uses: actions/checkout@v4
       - uses: dtolnay/rust-toolchain@stable
       - uses: Swatinem/rust-cache@v2
       - run: cargo test --test e2e -- --test-threads=1
   ```

5. **Timeouts and cleanup**
   - Each test spawns processes with `tokio::process::Command`
   - Timeout: 30s per test
   - Cleanup: kill child processes on drop

**Acceptance criteria:**
- [ ] At least 4 E2E scenarios pass in CI
- [ ] Tests are isolated (random ports, temp dirs)
- [ ] Tests complete in < 2 min total

**Effort:** 1–2 weeks

---

### F8 — SOME/IP Real Transport

**Goal:** Validate `native-comm-someip` FFI bindings against real COVESA/vsomeip.

**Current state:**
- `native-comm-someip/src/lib.rs` — stub mode without `vsomeip-ffi` feature
- `ffi.rs` and `runtime.rs` exist behind `#[cfg(feature = "vsomeip-ffi")]`
- Requires `libvsomeip3` system library (C++ shared lib)
- No CI test coverage for FFI path (no `libvsomeip3` in GitHub Actions)

**Implementation steps:**

1. **Set up vsomeip build environment**
   - Dockerfile with `libvsomeip3` built from source (COVESA/vsomeip `3.5.x`)
   - Or use pre-built packages if available for Ubuntu 22.04

2. **Verify FFI compilation**
   ```bash
   cargo build -p native-comm-someip --features vsomeip-ffi
   ```

3. **Create integration test with vsomeip loopback**
   - `SomeIpServiceProxy` sends SOME/IP request to a local vsomeip service
   - Verify: service discovery, request/response, event subscription
   - Requires vsomeip JSON config for test service IDs

4. **CI with Docker**
   ```yaml
   someip:
     name: SOME/IP FFI Tests
     runs-on: ubuntu-latest
     container:
       image: ghcr.io/rettde/opensovd-vsomeip:latest
     steps:
       - uses: actions/checkout@v4
       - run: cargo test -p native-comm-someip --features vsomeip-ffi
   ```

5. **Documentation**
   - Build instructions for vsomeip
   - Required env vars (`VSOMEIP_CONFIGURATION`)
   - Known limitations

**Acceptance criteria:**
- [ ] `cargo build --features vsomeip-ffi` compiles against `libvsomeip3`
- [ ] At least one round-trip test (offer service → find service → request/response)
- [ ] CI runs in Docker with pre-built vsomeip

**Effort:** 1–2 weeks (depends on vsomeip build complexity)

---

## Implementation Order (Recommended)

```
Phase 1 (Q2 2026)         Phase 2 (Q3 2026)         Phase 3 (Q4 2026)
─────────────────         ─────────────────         ─────────────────
F6  SBOM (1d)             F7  Prometheus (2d)       F5  E2E tests (2w)
F2  OTLP (2d)             F4  Vault (5d)            F8  SOME/IP (2w)
F1  Sled storage (5d)     F3  WS bridge (2w)
```

**Rationale:**
- Phase 1 items have no external dependencies and strengthen the foundation
- F1 (storage) is prerequisite for F5 (persistence E2E test) and useful for F3 (bridge session state)
- F6 (SBOM) is a quick CI-only change, good warm-up
- F8 (SOME/IP) is last because it requires hardware access and a vsomeip build environment

---

## Dependency Graph

```
F6 (SBOM)          ──── standalone
F2 (OTLP)          ──── standalone
F7 (Prometheus)     ──── standalone
F4 (Vault)          ──── standalone
F1 (Sled storage)   ──── standalone
F3 (WS bridge)      ──── benefits from F1 (session persistence)
F5 (E2E tests)      ──── requires F1 (persistence test), benefits from F3 (bridge test)
F8 (SOME/IP)        ──── standalone (hardware dependency)
```

---

## Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| sled 0.34 is in maintenance mode (sled 1.0 unreleased) | Low — 0.34 is stable, widely used | Pin to 0.34, add migration path in StorageBackend trait |
| vsomeip build breaks on newer GCC/Ubuntu | Medium — blocks F8 | Freeze Docker image, pin vsomeip version |
| Vault API changes | Low — vaultrs is actively maintained | Pin vaultrs version, integration test with Vault dev server |
| testcontainers flakiness in CI | Medium — blocks F5 | Use `--test-threads=1`, generous timeouts, retry logic |
| WebSocket bridge security | High — exposes tunnel to internet | Require mTLS for wss://, session token rotation, rate limiting |

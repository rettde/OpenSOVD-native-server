# Implementation Plan — Open Roadmap Items

> **Baseline:** v0.10.0-beta (312 tests, Waves 1–4 core complete)  
> **Scope:** 8 items from the integrated roadmap that are not yet implemented.  
> **Prioritization:** Dependencies-first, then risk reduction, then nice-to-have.

---

## Phasing Overview

```
Phase 1 — Quick Wins (no Rust changes, CI/packaging)     ~2–3 days
  T1.1  OpenAPI contract test in CI
  E2.2  Deployment packaging (Dockerfile, systemd, Helm)

Phase 2 — Core Infrastructure (enables Phase 3)           ~3–4 days
  W2.2  Historical diagnostic storage
  E2.4  Feature flags / runtime toggle

Phase 3 — Security Hardening                               ~2–3 days
  E2.1  TLS certificate hot-reload
  E2.3  Backup/restore for diagnostic state

Phase 4 — Test Infrastructure (needs running system)       ~2–3 days
  T2.1  Load/stress test harness
  T2.2  Fault injection tests
```

**Total estimated effort: 9–13 days**

---

## Phase 1 — Quick Wins

### T1.1 — OpenAPI Contract Test in CI

**Goal:** Validate the running server's OpenAPI spec against the CDF reference.

**Context:**  
- `sovd-cdf-validator/` already exists in the repo (Node.js based)
- `openapi-spec.json` exists at repo root
- `native-sovd/src/openapi.rs` generates the spec at runtime via `build_openapi_json()`

**Implementation:**

1. **Add a CI step** in `.github/workflows/ci.yml`:

```yaml
  openapi-contract:
    name: OpenAPI Contract Test
    runs-on: ubuntu-latest
    needs: [build]
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - name: Install validator
        run: npm ci
        working-directory: sovd-cdf-validator
      - name: Generate fresh spec from code
        run: |
          cargo run --release -- --dump-openapi > /tmp/generated-openapi.json
        # Requires a --dump-openapi CLI flag (see below)
      - name: Validate against CDF schema
        run: npx sovd-cdf-validator /tmp/generated-openapi.json
        working-directory: sovd-cdf-validator
```

2. **Add `--dump-openapi` flag** to `native-server/src/main.rs`:
   - Before starting the server, check `std::env::args()` for `--dump-openapi`
   - If present: print `build_openapi_json()` to stdout and exit 0
   - ~10 lines of code in `main()`

3. **Alternative (simpler, no binary run):** Add a Rust integration test:

```rust
// native-sovd/tests/openapi_contract.rs
#[test]
fn openapi_spec_matches_reference() {
    let generated = native_sovd::openapi::build_openapi_json();
    let reference: serde_json::Value =
        serde_json::from_str(include_str!("../../openapi-spec.json")).unwrap();
    // Compare key paths, endpoints, schemas
    assert_eq!(generated["openapi"], reference["openapi"]);
    // ... structural comparison of paths
}
```

**Files changed:**
- `.github/workflows/ci.yml` — new job
- `native-server/src/main.rs` — `--dump-openapi` flag (Option A) OR
- `native-sovd/tests/openapi_contract.rs` — new test file (Option B, preferred)
- `openapi-spec.json` — regenerate from current code

**Tests:** 1 new CI check  
**Effort:** S (0.5 days)

---

### E2.2 — Deployment Packaging

**Goal:** Provide production-ready deployment artifacts for automotive fleet environments.

**Deliverables:**

#### 1. Dockerfile (distroless)

```dockerfile
# deploy/Dockerfile
FROM rust:1.80-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --workspace
RUN strip target/release/opensovd-native-server

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /build/target/release/opensovd-native-server /
COPY --from=builder /build/opensovd-native-server.toml /config/
EXPOSE 8080 8443
ENTRYPOINT ["/opensovd-native-server"]
```

#### 2. systemd unit

```ini
# deploy/opensovd-native-server.service
[Unit]
Description=OpenSOVD Native Server
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
ExecStart=/usr/local/bin/opensovd-native-server
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5
LimitNOFILE=65536
Environment=SOVD_SERVER__HOST=0.0.0.0
Environment=SOVD_SERVER__PORT=8080
WorkingDirectory=/etc/opensovd
User=opensovd
Group=opensovd
ProtectSystem=strict
ReadWritePaths=/var/lib/opensovd
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

#### 3. Helm chart (skeleton)

```
deploy/helm/opensovd/
  Chart.yaml
  values.yaml
  templates/
    deployment.yaml
    service.yaml
    configmap.yaml
    serviceaccount.yaml
    hpa.yaml
```

**Files created:**
- `deploy/Dockerfile`
- `deploy/opensovd-native-server.service`
- `deploy/helm/opensovd/` — chart skeleton
- `deploy/README.md` — deployment guide

**Tests:** CI builds Docker image (no push)  
**Effort:** M (1.5 days)

---

## Phase 2 — Core Infrastructure

### W2.2 — Historical Diagnostic Storage

**Goal:** Persist faults, KPIs, and audit entries with time-range query support.

**Context:**
- `StorageBackend` trait (A2.1) already exists with `get/put/delete/list/count/flush`
- `AuditLog` uses in-memory `VecDeque` (capped at 10,000 entries)
- `FaultManager` uses in-memory `DashMap`
- `InMemoryStorage` uses `BTreeMap` (sorted keys — prefix scan works)

**Design:**

The `StorageBackend` trait is key-value only. For time-range queries, we use a **key encoding convention**:

```
{namespace}:{entity_type}:{timestamp_millis}:{id}
```

Example keys:
```
faults:hpc:1710900000000:P0123
audit:_global:1710900000000:seq-42
kpi:hpc:1710900000000:cpu_temp
```

BTreeMap/sled range scans on byte-sorted keys naturally support time-range queries.

**Implementation:**

1. **`HistoryService`** — new struct in `native-core/src/history.rs`:

```rust
pub struct HistoryService {
    store: Arc<dyn StorageBackend>,
}

impl HistoryService {
    pub fn record_fault(&self, fault: &SovdFault, timestamp: i64);
    pub fn record_audit(&self, entry: &SovdAuditEntry);
    pub fn query_faults(&self, component_id: Option<&str>,
                        from: i64, to: i64) -> Vec<SovdFault>;
    pub fn query_audit(&self, from: i64, to: i64) -> Vec<SovdAuditEntry>;
    pub fn compact(&self, before: i64);  // TTL-based eviction
}
```

2. **Wire into `AppState`** — add `history: Arc<HistoryService>` to `DiagState`

3. **Instrument handlers** — fault read/clear/audit handlers call `history.record_*()`

4. **New query endpoints** (extend existing):
   - `GET /export/faults?from=&to=` — already partially exists (W4.2), enhance with real time-range from HistoryService
   - `GET /audit?from=&to=` — extend existing `/audit` endpoint with optional time filter

5. **Configuration:**

```toml
[storage]
backend = "memory"        # "memory" | "sled"
path = "/var/lib/opensovd/data"  # sled only
retention_days = 90       # auto-compact
```

**Files changed/created:**
- `native-core/src/history.rs` — new: HistoryService
- `native-core/src/lib.rs` — export HistoryService
- `native-sovd/src/state.rs` — add `history` to DiagState
- `native-sovd/src/routes.rs` — instrument fault/audit handlers, time-range query params
- `native-server/src/main.rs` — wire HistoryService, storage config

**Tests:** ~12 new tests (history record/query/compact/edge cases)  
**Effort:** M (2 days)

---

### E2.4 — Feature Flags / Runtime Toggle

**Goal:** Enable/disable endpoint groups at runtime without recompilation.

**Design:**

```toml
[features]
flash = false               # Disable flash/OTA endpoints
extended_diagnostics = true  # x-uds vendor extensions
bridge = false               # Cloud bridge (already gated by bridge.enabled)
export = true                # Batch export endpoints (W4.2)
sse = true                   # SSE streaming endpoints
audit_endpoint = true        # /audit endpoint (may be restricted in production)
```

**Implementation:**

1. **`FeatureGate`** — new struct in `native-interfaces/src/feature_gate.rs`:

```rust
pub struct FeatureGate {
    flags: HashMap<String, bool>,
}

impl FeatureGate {
    pub fn from_config(config: &FeatureConfig) -> Self;
    pub fn is_enabled(&self, feature: &str) -> bool;
}
```

2. **Middleware** — `feature_gate_middleware` checks `FeatureGate` before routing:
   - Returns `501 Not Implemented` with `SOVD-ERR-501` for disabled features
   - Maps URL path prefixes to feature names

3. **Wire into `AppState`** — add `Arc<FeatureGate>` to `SecurityState`

4. **Optional:** Expose `GET /sovd/v1/system-info` features section showing active/disabled flags

**Files changed/created:**
- `native-interfaces/src/feature_gate.rs` — new: FeatureGate + FeatureConfig
- `native-interfaces/src/lib.rs` — export
- `native-sovd/src/routes.rs` — feature_gate_middleware
- `native-sovd/src/state.rs` — FeatureGate in SecurityState
- `native-server/src/main.rs` — wire config

**Tests:** ~6 tests (enabled/disabled/default/unknown feature)  
**Effort:** S (1 day)

---

## Phase 3 — Security Hardening

### E2.1 — TLS Certificate Hot-Reload

**Goal:** Reload TLS certificates without server restart when files change on disk.

**Context:**
- `axum_server::tls_rustls::RustlsConfig` already supports `.reload_from_pem_file()` 
- Current code at `native-server/src/main.rs:431-458` creates `RustlsConfig` once

**Implementation:**

1. **File watcher** using `notify` crate (inotify on Linux, kqueue on macOS):

```rust
// native-server/src/tls_reload.rs
pub async fn watch_tls_certs(
    tls_config: RustlsConfig,
    cert_path: PathBuf,
    key_path: PathBuf,
) {
    use notify::{Watcher, RecursiveMode, Event, EventKind};
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                let _ = tx.blocking_send(());
            }
        }
    }).expect("file watcher init");
    watcher.watch(&cert_path, RecursiveMode::NonRecursive).unwrap();
    watcher.watch(&key_path, RecursiveMode::NonRecursive).unwrap();

    while rx.recv().await.is_some() {
        // Debounce: wait 500ms for writes to settle
        tokio::time::sleep(Duration::from_millis(500)).await;
        match tls_config.reload_from_pem_file(&cert_path, &key_path).await {
            Ok(()) => info!("TLS certificates reloaded"),
            Err(e) => warn!("TLS reload failed (keeping old certs): {e}"),
        }
    }
}
```

2. **Spawn watcher** in `main()` after TLS config is created (1 line: `tokio::spawn(watch_tls_certs(…))`)

3. **Config option:**

```toml
[server]
tls_auto_reload = true  # default: false
```

**Dependencies:** `notify = "7"` in `native-server/Cargo.toml`

**Files changed/created:**
- `native-server/src/tls_reload.rs` — new: file watcher + reload logic
- `native-server/src/main.rs` — spawn watcher, config option
- `native-server/Cargo.toml` — add `notify` dependency

**Tests:** 2 integration tests (reload success, reload with invalid cert = keeps old)  
**Effort:** S (1 day)

---

### E2.3 — Backup/Restore for Diagnostic State

**Goal:** Export and import diagnostic state for workshop handover, ECU replacement, or migration.

**Design:**

State to export:
- Fault history (from HistoryService / FaultManager)
- Audit trail (from AuditLog)
- Lock state (from LockManager)
- Software package status (from package_store)

Format: Single JSON file (or NDJSON for streaming large exports).

**Implementation:**

1. **Export endpoint** — `GET /sovd/v1/x-admin/backup`:

```rust
async fn create_backup(State(state): State<AppState>) -> impl IntoResponse {
    let backup = BackupManifest {
        version: env!("CARGO_PKG_VERSION"),
        created_at: Utc::now().to_rfc3339(),
        faults: state.diag.fault_manager.list_all(),
        audit: state.security.audit_log.entries(),
        locks: state.diag.lock_manager.list_all(),
        packages: state.runtime.package_store.iter()
            .map(|e| e.value().clone()).collect(),
    };
    Json(backup)
}
```

2. **Import endpoint** — `POST /sovd/v1/x-admin/restore`:
   - Accepts `BackupManifest` JSON body
   - Validates version compatibility
   - Merges or replaces state (configurable: `mode=merge|replace`)

3. **CLI support** — `--backup <path>` and `--restore <path>` flags

**Files changed/created:**
- `native-sovd/src/backup.rs` — new: BackupManifest, export/import logic
- `native-sovd/src/routes.rs` — mount `/x-admin/backup` and `/x-admin/restore`
- `native-server/src/main.rs` — CLI flags

**Tests:** ~4 tests (roundtrip, version mismatch, merge vs replace)  
**Effort:** M (1.5 days)

---

## Phase 4 — Test Infrastructure

### T2.1 — Load/Stress Test Harness

**Goal:** Prove the server handles 200+ concurrent clients under sustained load.

**Deliverables:**

1. **k6 load test scripts** — `tests/load/`:

```
tests/load/
  k6-discovery.js        # GET /components, /version-info
  k6-data-read.js        # GET /components/{id}/data/{did} — high concurrency
  k6-fault-read.js       # GET /components/{id}/faults
  k6-mixed-workload.js   # Realistic mix: 60% reads, 20% faults, 10% ops, 10% audit
  k6-sse-subscribe.js    # SSE connections (long-lived)
  thresholds.json        # Pass/fail criteria
```

Thresholds:
```json
{
  "http_req_duration": ["p(95) < 50", "p(99) < 200"],
  "http_req_failed": ["rate < 0.01"],
  "iterations": ["rate > 500"]
}
```

2. **Criterion benchmarks** — `benches/`:

```rust
// native-sovd/benches/router_throughput.rs
fn bench_list_components(c: &mut Criterion) { ... }
fn bench_read_data(c: &mut Criterion) { ... }
fn bench_audit_log_record(c: &mut Criterion) { ... }
```

3. **CI integration** (optional, nightly only):

```yaml
  load-test:
    name: Load Test (nightly)
    if: github.event_name == 'schedule'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: grafana/k6-action@v0.3.0
        with:
          filename: tests/load/k6-mixed-workload.js
```

**Files created:**
- `tests/load/*.js` — k6 scripts
- `tests/load/thresholds.json`
- `tests/load/README.md` — how to run
- `native-sovd/benches/router_throughput.rs` — criterion benchmarks

**Effort:** M (1.5 days)

---

### T2.2 — Fault Injection Tests

**Goal:** Verify graceful degradation when backends are unreachable or returning errors.

**Scenarios:**
1. **Backend unreachable** — HTTP backend returns connection refused
2. **Backend timeout** — Backend hangs for >30s
3. **Backend 500** — Backend returns internal server error
4. **Partial failure** — One of N backends fails, others work
5. **Storage corruption** — `StorageBackend.get()` returns garbage
6. **Disk full** — `StorageBackend.put()` fails (audit, fault persistence)
7. **Rate limiter exhaustion** — All clients exceed limits simultaneously

**Implementation:**

1. **`FailingBackend`** — test helper in `native-core/src/test_utils.rs`:

```rust
pub struct FailingBackend {
    pub fail_mode: FailMode,
}

pub enum FailMode {
    ConnectionRefused,
    Timeout(Duration),
    InternalError,
    Intermittent { fail_rate: f64 },
}
```

2. **Integration tests** — `native-sovd/tests/fault_injection.rs`:

```rust
#[tokio::test]
async fn backend_unreachable_returns_502() { ... }

#[tokio::test]
async fn partial_backend_failure_degrades_gracefully() { ... }

#[tokio::test]
async fn rate_limit_exhaustion_returns_429() { ... }

#[tokio::test]
async fn audit_log_continues_on_storage_failure() { ... }
```

3. **Chaos mode** (optional config for manual testing):

```toml
[chaos]
enabled = false
fail_rate = 0.1           # 10% of backend calls fail
latency_injection_ms = 500  # add 500ms to all calls
```

**Files changed/created:**
- `native-core/src/test_utils.rs` — new: FailingBackend, FailMode
- `native-sovd/tests/fault_injection.rs` — new: ~8 integration tests
- `native-server/src/main.rs` — optional chaos config (low priority)

**Tests:** ~8 new integration tests  
**Effort:** M (1.5 days)

---

## Dependency Graph

```
Phase 1 (no deps)           Phase 2 (parallel)          Phase 3            Phase 4
─────────────────           ──────────────────          ──────────         ──────────
T1.1 OpenAPI test ─┐
                    ├──→ W2.2 Historical storage ──→ E2.3 Backup ──→ T2.2 Fault inject
E2.2 Deployment   ─┘
                         E2.4 Feature flags
                                                    E2.1 TLS reload    T2.1 Load tests
```

## Summary

| ID | Item | Phase | Effort | New Tests | Key Dependency |
|----|------|-------|--------|-----------|----------------|
| T1.1 | OpenAPI contract test | 1 | S | 1 | sovd-cdf-validator |
| E2.2 | Deployment packaging | 1 | M | — | — |
| W2.2 | Historical storage | 2 | M | ~12 | StorageBackend (A2.1) |
| E2.4 | Feature flags | 2 | S | ~6 | — |
| E2.1 | TLS hot-reload | 3 | S | 2 | `notify` crate |
| E2.3 | Backup/restore | 3 | M | ~4 | W2.2 (optional) |
| T2.1 | Load tests | 4 | M | — | k6 / criterion |
| T2.2 | Fault injection | 4 | M | ~8 | — |
| **Total** | | | **9–13 days** | **~33 tests** | |

**Post-completion target:** ~345 tests, all open items closed, v0.11.0 release.

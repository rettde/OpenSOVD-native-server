# Release Notes — OpenSOVD-native-server v0.8.0-beta

**Date:** 2026-03-19
**License:** Apache-2.0
**Rust toolchain:** 1.75+
**Status:** Beta — Wave 2 complete (pluggable infrastructure + observability)

---

## Highlights

**Wave 2 is complete.** This release adds pluggable infrastructure abstractions
(storage, secrets, rate limiting), full observability (hash-chained audit,
JSON logging, RED metrics, OTLP export), fault debouncing, and richer
mode/session modelling with UDS session mapping.

| Metric | Value |
|--------|-------|
| REST API endpoints | 60+ (SOVD) + 9 vendor extensions + 5 operational |
| Automated tests | 269 (0 failures) |
| ISO 17978-3 coverage | 51 mandatory requirements |
| Clippy | Clean (pedantic, `#![deny(warnings)]`) |
| Workspace crates | 7 |
| New feature flags | `otlp` (OpenTelemetry OTLP export) |

---

## What's New (since v0.7.0-beta / v0.6.0)

### 1. Pluggable Storage Backend (A2.1)

Generic key-value persistence abstraction for components that need durable state.

- **`StorageBackend` trait** — `get`, `put`, `delete`, `list`, `list_keys`, `count`, `flush`
- **`InMemoryStorage`** — default implementation using `BTreeMap` + `Mutex`
- 11 unit tests covering all operations, prefix filtering, edge cases
- Location: `native-interfaces/src/storage.rs`

### 2. Secrets Abstraction Layer (A2.3)

Abstracts how the server retrieves sensitive values (JWT keys, API keys, TLS passwords).

- **`SecretProvider` trait** — `get_secret(name)` → `Option<String>`
- **`EnvSecretProvider`** — reads `SOVD_*` environment variables (configurable prefix)
- **`StaticSecretProvider`** — hardcoded values for tests/demos
- 8 unit tests
- Location: `native-interfaces/src/secrets.rs`

### 3. Per-Client Rate Limiting (A2.5)

Token-bucket rate limiter integrated as Axum middleware.

- **`RateLimiter`** — DashMap-backed, per-client token buckets
- **`rate_limit_middleware`** — extracts client identity from auth context
- Configurable via `rate_limit.enabled`, `rate_limit.max_requests`, `rate_limit.window_secs`
- Returns `429 Too Many Requests` with structured SOVD error envelope
- 5 unit tests
- Location: `native-sovd/src/rate_limit.rs`

### 4. Fault Debouncing — FaultGovernor (W2.3)

DFM-side debounce layer that suppresses rapid-fire duplicate fault reports.

- Debounce key: `(fault_id, component_id)` pair
- Configurable debounce window duration
- Cleared faults reset debounce state (re-occurrence always reported)
- Counters: `total_received`, `total_suppressed`, `tracked_faults`
- `reap_stale()` for memory cleanup
- Implements fault-lib design requirement: "Debouncing needs to be also possible
  in the DFM if there is a multi-fault aggregation"
- 8 unit tests
- Location: `native-core/src/fault_governor.rs`

### 5. System KPI Endpoint (W2.1)

`GET /sovd/v1/system-info` — aggregated runtime KPIs in a single response:

```json
{
  "health": { "status": "ok", "uptime_secs": 3600, ... },
  "components": { "count": 5 },
  "faults": { "active_count": 2 },
  "audit": {
    "entry_count": 42,
    "chain_integrity": { "status": "ok", "verified": 42 }
  },
  "rate_limiter": { "tracked_clients": 3 }
}
```

### 6. Richer Mode/Session Model (W2.4)

Enhanced `SovdMode` with UDS session mapping for ISO 14229-1 alignment.

- **`SovdModeDescriptor`** — per-mode metadata:
  - `id`, `name`, `description`
  - `udsSession` (e.g. `0x01` default, `0x02` programming, `0x03` extended)
  - `requiresSecurityAccess` flag
- **`activeSince`** — ISO 8601 timestamp on `SovdMode`
- Backward-compatible: new fields use `skip_serializing_if` defaults

### 7. Observability Stack

| ID | Feature | Detail |
|----|---------|--------|
| E1.1 | **Audit hash chaining** | SHA-256 chain integrity, `verify_chain()` method, 6 tests |
| E1.2 | **Structured JSON logging** | `logging.format = "json"` — flattened events with targets, SIEM-ready |
| E1.3 | **RED metrics** | `sovd_http_requests_total` + `sovd_http_request_duration_seconds` with method/path/status labels |
| A2.4 | **OTLP export** | `--features otlp` enables `tracing-opentelemetry` with gRPC export. Config: `logging.otlp_endpoint` |

### 8. Shared Library Alignment

Cross-referenced with Eclipse OpenSOVD shared libraries (`fault-lib`, `dlt-tracing-lib`, `cpp-bindings`):

- **`DltLayer` → `DltTextLayer`** — renamed to clarify lightweight text-format fallback
  vs. full `tracing-dlt` binary-protocol layer
- **`fault_bridge.rs`** — `TODO(fault-lib-stable)` migration marker
- **`fault_governor.rs`** — cross-references fault-lib design doc §DFM debouncing
- No true duplication found; both shared libs require nightly Rust (edition 2024)

### 9. ComponentBackend Trait Diet (A2.2)

- Extracted `ExtendedDiagBackend` for optional extended diagnostic operations
- Core `ComponentBackend` stays minimal — only basic SOVD operations required

---

## New Files

| File | Purpose |
|------|---------|
| `native-interfaces/src/storage.rs` | `StorageBackend` trait + `InMemoryStorage` |
| `native-interfaces/src/secrets.rs` | `SecretProvider` trait + Env/Static providers |
| `native-sovd/src/rate_limit.rs` | Token-bucket rate limiter + config |
| `native-core/src/fault_governor.rs` | DFM-side fault debouncing |

---

## Test Summary

```
native-interfaces   52 tests   ✅  (+19: storage, secrets)
native-core         67 tests   ✅  (+14: fault_governor, audit chain)
native-health        6 tests   ✅
native-sovd        143 tests   ✅  (+6: system-info, rate limiting)
native-server        1 test    ✅
─────────────────────────────────
Total              269 tests   0 failures
```

---

## Configuration Changes

New config keys in `opensovd-native-server.toml`:

```toml
[logging]
format = "json"                          # "text" (default) or "json"
otlp_endpoint = "http://localhost:4317"  # optional, requires --features otlp

[rate_limit]
enabled = true
max_requests = 100
window_secs = 60
```

---

## Feature Flags

| Flag | Default | Description |
|------|---------|-------------|
| `otlp` | off | OpenTelemetry OTLP trace export (pulls in opentelemetry + tonic) |
| `vsomeip-ffi` | off | vSomeIP FFI bindings |
| `persist` | off | sled-based persistent FaultManager |

---

## Breaking Changes

None for REST API consumers. Internal changes:

| Change | Impact |
|--------|--------|
| `DltLayer` → `DltTextLayer` | Update imports if using the DLT layer directly |
| `SecurityState` has new `rate_limiter` field | State construction must add `rate_limiter: None` (or configured) |
| `SovdMode` has new optional fields | Fully backward-compatible for serialization |

---

## Known Limitations

- `StorageBackend` not yet wired into `AuditLog` or `FaultManager` (integration planned for Wave 3)
- `SecretProvider` not yet consumed by auth module (integration planned for Wave 3)
- `FaultGovernor` not yet wired into the request pipeline (available as library)
- OTLP export requires the `otlp` feature flag and has not been tested against
  a live collector in CI
- `DltTextLayer` emits text-format only, not DLT binary protocol

---

## What's Next — Wave 3

| Track | Items |
|-------|-------|
| **Integration** | Wire StorageBackend into AuditLog + FaultManager, wire SecretProvider into auth |
| **Features** | Historical fault storage, configuration snapshots, bulk data transfer |
| **Enterprise** | TLS hot-reload, deployment packaging, backup/restore |
| **Testing** | Load tests, fault injection, OTLP integration tests |

---

*Built with Rust 🦀 • Part of [Eclipse OpenSOVD](https://github.com/eclipse-opensovd)*

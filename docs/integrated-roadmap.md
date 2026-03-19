# Integrated Roadmap — Architecture, Features & Enterprise Readiness

> OpenSOVD-native-server v0.7+ development roadmap.
> Designed for **enterprise fleet diagnostics** across the full vehicle lifecycle.

---

## Design Philosophy

1. **Refactor before building** — make architectural decisions *before* implementing features that depend on them, not after.
2. **Architecture track runs parallel** — every Wave has an "Arch" gate that lands first.
3. **Enterprise quality is not a bolt-on** — graceful shutdown, observability, secrets, and hardening ship alongside features, not in a separate "hardening sprint" after the fact.
4. **Fleet-scale thinking from day one** — multi-tenant data isolation, per-client rate limiting, and audit integrity are designed into the foundation, even if full multi-tenant deployment is Wave 3.

---

## Legend

| Symbol | Meaning |
|--------|---------|
| 🏗️ ARCH | Architecture / refactoring gate — must land before dependent features |
| 🔧 FEAT | Feature implementation |
| 🛡️ ENTER | Enterprise readiness / hardening |
| 🧪 TEST | Test infrastructure or strategy change |
| ✅ | Done |
| ⏳ | Next up |

---

## Wave 1 — Authorization, Audit, Entity Model, SW Packages

### Wave 1 Arch Gate (land first)

| ID | Type | Item | Rationale | Effort |
|----|------|------|-----------|--------|
| A1.1 | 🏗️ | **Graceful shutdown + connection draining** | Must exist before any production deployment. Adding it later means every integration test and deployment script changes. `axum::serve` with `graceful_shutdown` + `SIGTERM` handler + audit log flush. | S |
| A1.2 | 🏗️ | **Liveness vs. readiness health probes** | `/healthz` (process alive) + `/readyz` (backends reachable, audit sink writable). K8s, systemd, fleet orchestrators all need this. Changing health semantics later breaks monitoring. | S |
| A1.3 | 🏗️ | **Request body size limit + per-endpoint timeout** | `DefaultBodyLimit` on axum + `tower::timeout`. Prevents abuse before rate limiting exists. Trivial now, painful to retrofit after flash/OTA upload routes exist. | S |
| A1.4 | 🏗️ | **Config validation at startup (fail-fast)** | Validate TLS certs exist, backend URLs parse, auth config is consistent. Log structured error and exit non-zero. Every enterprise deployment needs deterministic startup. | S |
| A1.5 | 🏗️ | **`AppState` sub-grouping** | Group `AppState` fields into logical sub-states: `DiagState { fault_manager, lock_manager, diag_log }`, `SecurityState { audit_log, oem_profile }`, `RuntimeState { execution_store, proximity_store, health }`. Prevents the struct from growing into an unstructured bag as Waves 2–3 add fields. Do this *before* adding `entity_backend`, `package_store`, `kpi_provider`, `history_store`, `tenant_context`. | M |
| A1.6 | 🏗️ | **Error taxonomy: publish SOVD error code catalog** | Document every `SOVD-ERR-*` code the server can return, with HTTP status, meaning, and stability guarantee. Enterprise integrators need this *before* they write client code against new endpoints. | S |

### Wave 1 Features (after Arch Gate)

| ID | Type | Item | Status | Depends on | Effort |
|----|------|------|--------|------------|--------|
| W1.1 | 🔧 | Fine-grained AuthZ (`AuthzPolicy` in `OemProfile`) | ✅ Done | — | M |
| W1.2 | 🔧 | Diagnostic Audit Trail (`AuditLog`, `/audit` endpoint, handler instrumentation) | ✅ Done | W1.1 | M |
| W1.3 | 🔧 | Full Apps/Funcs entities (`EntityBackend` trait, real `/apps` + `/funcs` routes with nested resources) | ⏳ Next | A1.5 | L |
| W1.4 | 🔧 | Software-Package Lifecycle (upload, progress, activate, rollback) | Pending | A1.5 | M |

### Wave 1 Enterprise Hardening (parallel with features)

| ID | Type | Item | Rationale | Effort |
|----|------|------|-----------|--------|
| E1.1 | 🛡️ | **Audit log hash chaining** | Each `SovdAuditEntry` includes `prev_hash = SHA-256(previous entry)`. Deletions/modifications are detectable. Required for ISO 27001, UNECE R155 compliance. | S |
| E1.2 | 🛡️ | **Structured JSON logging with trace correlation** | Every log line includes `trace_id` from W3C `traceparent`. Switch from `tracing_subscriber::fmt` to JSON formatter. This is the foundation for fleet-scale log aggregation. | S |
| E1.3 | 🛡️ | **RED metrics per endpoint** | Add rate, error rate, and duration histogram per route using `metrics` crate. The Prometheus exporter already exists; this adds the instrumentation. | S |

### Wave 1 Test Infrastructure

| ID | Type | Item | Rationale | Effort |
|----|------|------|-----------|--------|
| T1.1 | 🧪 | **OpenAPI contract test in CI** | Validate the running server against `openapi-spec.json`. `sovd-cdf-validator` exists; wire it into GitHub Actions. Catches drift between implementation and spec. | S |

---

## Wave 2 — KPI, Historical Storage, Fault Governance

### Wave 2 Arch Gate (land first)

| ID | Type | Item | Rationale | Effort |
|----|------|------|-----------|--------|
| A2.1 | 🏗️ | **`StorageBackend` trait — pluggable persistence** | Before building historical KPI/fault storage, define a `StorageBackend` trait: `write_event()`, `query_range()`, `compact()`. Default impl: sled (embedded). Optional: SQLite, forward-to-external. This avoids hardcoding sled everywhere and having to rip it out for fleet deployments that use a central time-series DB. | M |
| A2.2 | 🏗️ | **`ComponentBackend` trait diet — extract `ExtendedDiagBackend`** | The current trait has 28 methods. Before adding KPI/system-info methods, split into: `ComponentBackend` (core SOVD: discovery, data, faults, operations, config, mode, lock, software) and `ExtendedDiagBackend` (vendor extensions: io_control, comm_control, dtc_setting, read/write_memory, flash). Keeps the core trait implementable for non-UDS backends. | M |
| A2.3 | 🏗️ | **Secrets abstraction layer** | Before Wave 2 adds more sensitive config (HMAC keys for audit, DB credentials for history store), add a `SecretSource` trait: `fn get_secret(key: &str) -> Result<String>`. Implementations: env var, file, Vault. No more plaintext secrets in TOML. | M |
| A2.4 | 🏗️ | **OpenTelemetry OTLP export** | Replace trace-header-only propagation with full OTLP span export. This must happen before Wave 2 adds async background jobs (KPI polling, fault debounce timers) which need proper tracing. | M |
| A2.5 | 🏗️ | **Per-client rate limiting** | Add `tower::RateLimit` keyed by client identity (from JWT `sub` or API key). Fleet deployments with many clients need protection against noisy neighbors *before* adding expensive historical queries. | M |

### Wave 2 Features

| ID | Type | Item | Depends on | Effort |
|----|------|------|------------|--------|
| W2.1 | 🔧 | **KPI / system-info resources** — `/components/{id}/kpis`, `/system-info` | A2.1, A2.2 | M |
| W2.2 | 🔧 | **Historical diagnostic storage** — time-range queries on faults, KPIs, audit | A2.1 | L |
| W2.3 | 🔧 | **Fault debouncing + operation cycle handling** — `FaultGovernor` wrapping `FaultBridge` | — | M |
| W2.4 | 🔧 | **Richer mode/session model** — UDS session semantics (default, extended, programming) mapped to SOVD modes | — | M |

### Wave 2 Enterprise Hardening

| ID | Type | Item | Rationale | Effort |
|----|------|------|-----------|--------|
| E2.1 | 🛡️ | **TLS certificate hot-reload** | Watch cert files, reload without restart. Fleet deployments rotate certs on schedule. | M |
| E2.2 | 🛡️ | **Deployment packaging** | Distroless container image, systemd unit, Helm chart. Automotive customers deploy on Yocto/QNX or K8s. | M |
| E2.3 | 🛡️ | **Backup/restore for diagnostic state** | Export/import fault history, audit log, lock state. Workshop handover, ECU replacement. | M |
| E2.4 | 🛡️ | **Feature flags / runtime toggle** | Enable/disable flash endpoints, extended diagnostics, KPI collection at runtime. Align with `OemProfile` or add `FeatureGate` layer. | M |

### Wave 2 Test Infrastructure

| ID | Type | Item | Rationale | Effort |
|----|------|------|-----------|--------|
| T2.1 | 🧪 | **Load/stress test harness** | Before claiming fleet-readiness, prove the server handles 200+ concurrent clients, sustained KPI polling, and audit log under pressure. Use `criterion` benchmarks + `k6` or `wrk` HTTP load tests. | M |
| T2.2 | 🧪 | **Fault injection tests** | Simulate backend failures (CDA unreachable, sled corruption, disk full). Verify graceful degradation. | M |

---

## Wave 3 — Cloud Bridge, Multi-Tenant, Variant-Aware, Zero-Trust

### Wave 3 Arch Gate (land first — these are ADR-level decisions)

| ID | Type | Item | Rationale | Effort |
|----|------|------|-----------|--------|
| A3.1 | 🏗️ | **ADR: Cloud bridge topology** | Decision: (a) same binary with feature-gated bridge mode, (b) separate `native-bridge` binary, (c) sidecar/proxy pattern. This affects crate structure, state management, and deployment model. Decide *before* writing code. | S (doc) |
| A3.2 | 🏗️ | **ADR: Multi-tenant data isolation strategy** | Decision: (a) policy-only isolation (single state, `AuthzPolicy` gates access), (b) per-tenant state partitioning (separate `AppState` per tenant), (c) tenant-scoped storage namespacing. Option (a) is cheapest but weakest; (c) is strongest but most complex. Fleet diagnostics needs at least (a)+(c) for audit and history. | S (doc) |
| A3.3 | 🏗️ | **`TenantContext` middleware + `OemProfile` per-tenant selection** | After ADR A3.2, implement tenant extraction from JWT/header and inject `TenantContext` into request extensions. `OemProfile` selection can become tenant-aware (different OEMs on same fleet server). | M |
| A3.4 | 🏗️ | **`BridgeTransport` trait for cloud connectivity** | After ADR A3.1, define the trait: `accept_remote()`, `forward_to_vehicle()`, `heartbeat()`. Implementations: WebSocket relay, gRPC tunnel, MQTT bridge. | L |
| A3.5 | 🏗️ | **API versioning contract** | Document: `/sovd/v1/` is stable, breaking changes require `/sovd/v2/`. Define deprecation policy. Enterprise integrators need this guarantee before building fleet infrastructure. | S (doc) |

### Wave 3 Features

| ID | Type | Item | Depends on | Effort |
|----|------|------|------------|--------|
| W3.1 | 🔧 | **Cloud bridge mode** — brokered remote diagnostics | A3.1, A3.4 | L |
| W3.2 | 🔧 | **Multi-tenant fleet/workshop model** — tenant-scoped data, audit, history | A3.2, A3.3 | L |
| W3.3 | 🔧 | **Variant-aware discovery** — filter entities by installation/software variant | A3.3 | M |
| W3.4 | 🔧 | **Zero-trust hardening** — mutual TLS for backend-to-backend, certificate pinning, signed audit export | A3.5 | M |

### Wave 3 Enterprise Hardening

| ID | Type | Item | Rationale | Effort |
|----|------|------|-----------|--------|
| E3.1 | 🛡️ | **Client SDK generation** | Auto-generate Python + TypeScript SDKs from `openapi-spec.json`. Fleet integrators don't hand-write HTTP clients. | M |
| E3.2 | 🛡️ | **Compliance evidence export** | Generate ISO 27001 / UNECE R155 evidence packages: audit chain, auth config summary, TLS posture, API coverage report. | M |
| E3.3 | 🛡️ | **Canary / blue-green deployment support** | HTTP header-based routing to canary instances. Fleet-scale OTA of the diagnostic server itself. | M |

---

## Sequencing Summary

```
                    ARCH GATES              FEATURES              ENTERPRISE
                    ──────────              ────────              ──────────
Wave 1  ┌─ A1.1 Graceful shutdown    W1.1 AuthZ ✅            E1.1 Audit hash chain
(now)   │  A1.2 Health probes        W1.2 Audit Trail ✅      E1.2 JSON logging
        │  A1.3 Body size limit      W1.3 Apps/Funcs ⏳       E1.3 RED metrics
        │  A1.4 Config validation    W1.4 SW Packages         T1.1 Contract test
        │  A1.5 AppState sub-groups
        └  A1.6 Error catalog
        
Wave 2  ┌─ A2.1 StorageBackend       W2.1 KPI/system-info     E2.1 TLS hot-reload
        │  A2.2 Backend trait diet    W2.2 Historical storage  E2.2 Deployment pkg
        │  A2.3 Secrets abstraction   W2.3 Fault debouncing    E2.3 Backup/restore
        │  A2.4 OTLP export          W2.4 Mode/session model  E2.4 Feature flags
        └  A2.5 Per-client rate limit                          T2.1 Load tests
                                                               T2.2 Fault injection

Wave 3  ┌─ A3.1 ADR: Bridge topology W3.1 Cloud bridge        E3.1 Client SDKs
        │  A3.2 ADR: Tenant isolation W3.2 Multi-tenant        E3.2 Compliance export
        │  A3.3 TenantContext MW      W3.3 Variant-aware       E3.3 Canary deploy
        │  A3.4 BridgeTransport trait W3.4 Zero-trust
        └  A3.5 API versioning
```

---

## Refactoring Decision Points

These are the critical moments where a refactoring decision **must** be made to avoid rework:

| Decision | When | Why now |
|----------|------|---------|
| **AppState sub-grouping** | Before W1.3 | W1.3 adds `entity_backend`, W1.4 adds `package_store`. Without grouping, the struct becomes unmanageable by Wave 2. |
| **StorageBackend trait** | Before W2.2 | W2.2 adds historical storage. If sled is hardcoded, Wave 3 multi-tenant can't use per-tenant namespacing. |
| **ComponentBackend diet** | Before W2.1 | W2.1 adds KPI methods. Adding them to the existing 28-method trait makes every mock/test backend implementation painful. |
| **Secrets abstraction** | Before W2.2 | W2.2 adds DB credentials for history store. If they go in TOML plaintext, security audit fails. |
| **Tenant isolation ADR** | Before W3.2 | W3.2 needs tenant-scoped storage. The storage schema from W2.2 must anticipate this, so the ADR should be written during Wave 2 even though implementation is Wave 3. |
| **Bridge topology ADR** | Before W3.1 | Determines whether `native-bridge` is a separate crate/binary or a mode of the existing server. Affects crate layout. Write the ADR during Wave 2. |

---

## Current Status (as of Wave 1 in progress)

| Item | Status | Tests |
|------|--------|-------|
| W1.1 Fine-Grained AuthZ | ✅ Complete | Covered |
| W1.2 Diagnostic Audit Trail | ✅ Complete | 208 tests, clippy clean |
| A1.1–A1.6 Arch Gate | ⏳ **Next** | — |
| E1.1–E1.3 Hardening | Pending | — |
| W1.3 Apps/Funcs | Blocked on A1.5 | — |
| W1.4 SW Packages | Blocked on A1.5 | — |

**Immediate next step:** Land the Wave 1 Arch Gate (A1.1–A1.6), then proceed to W1.3 Apps/Funcs.

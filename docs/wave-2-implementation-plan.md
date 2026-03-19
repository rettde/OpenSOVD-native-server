# Wave 2 — Implementation Plan

**Scope:** Arch gate refactorings, KPI/system-info, fault governance, mode/session model, deferred E1.x hardening.

**Target version:** 0.8.0

---

## Dependency & Ordering

```
A2.2 (trait diet) ──────────────┐
                                ├──→ W2.1 (KPI/system-info)
A2.1 (StorageBackend) ──────────┤
                                ├──→ W2.2 (historical storage) [deferred — L effort]
A2.3 (secrets) ─────────────────┘

A2.5 (rate limiting) ───── independent
A2.4 (OTLP export) ─────── independent

E1.1 (audit hash chain) ── independent
E1.2 (JSON logging) ─────── pairs with A2.4
E1.3 (RED metrics) ──────── independent

W2.3 (fault debouncing) ── independent
W2.4 (mode/session) ────── independent
```

**Execution order for this session:**
1. **A2.2** — Trait diet (extract `ExtendedDiagBackend`)
2. **A2.5** — Per-client rate limiting
3. **E1.1** — Audit log hash chaining
4. **E1.3** — RED metrics per endpoint
5. **W2.3** — Fault debouncing / FaultGovernor
6. **W2.4** — Richer mode/session model
7. **E1.2 + A2.4** — JSON logging + OTLP (paired)
8. **A2.1 + A2.3** — StorageBackend + secrets (paired, enables W2.1)
9. **W2.1** — KPI / system-info

W2.2 (historical storage) is L-effort and deferred to a later session.

---

## A2.2 — ComponentBackend Trait Diet

### Problem
`ComponentBackend` has 28+ methods. The 7 "extended diagnostics" methods
(`io_control`, `communication_control`, `dtc_setting`, `read_memory`,
`write_memory`, `flash`, `active_keepalives`) are UDS-specific vendor
extensions that non-UDS backends should not need to implement.

### Design
1. Extract `ExtendedDiagBackend` trait with those 7 methods
2. Give all methods **default** "not supported" implementations
3. `ComponentRouter` implements both traits, forwarding to owning backend
4. `SovdHttpBackend` implements both (it already does)
5. Mock backends in tests only need `ComponentBackend` (the lean trait)
6. Route handlers for `/x-uds/*` cast `state.backend` to `ExtendedDiagBackend`

### Methods moving to `ExtendedDiagBackend`
- `io_control`
- `communication_control`
- `dtc_setting`
- `read_memory`
- `write_memory`
- `flash`
- `active_keepalives`

### Files changed
- `native-interfaces/src/backend.rs` — split trait, export new trait
- `native-interfaces/src/lib.rs` — re-export `ExtendedDiagBackend`
- `native-core/src/router.rs` — implement `ExtendedDiagBackend` for `ComponentRouter`
- `native-core/src/http_backend.rs` — implement `ExtendedDiagBackend` for `SovdHttpBackend`
- `native-sovd/src/state.rs` — no change (backend is `Arc<dyn ComponentBackend>`, x-uds routes downcast)
- `native-sovd/src/routes.rs` — x-uds handlers use `ExtendedDiagBackend` via blanket or second state field
- Test mock backends — remove extended method impls

---

## A2.5 — Per-Client Rate Limiting

Add `tower::RateLimit` keyed by client identity (JWT `sub` or API key).

### Design
- New middleware layer after auth: extracts client ID, applies per-client bucket
- `DashMap<String, RateLimiter>` for per-client state
- Configurable: `rate_limit.requests_per_second`, `rate_limit.burst`
- Returns `429 Too Many Requests` with `Retry-After` header

---

## E1.1 — Audit Log Hash Chaining

Each `SovdAuditEntry` includes `prev_hash = SHA-256(previous entry)`.

### Design
- Add `hash: String` and `prev_hash: String` fields to `SovdAuditEntry`
- `AuditLog::record()` computes SHA-256 of `(prev_hash + serialized_entry)`
- First entry uses `prev_hash = "genesis"`
- Verification: `AuditLog::verify_chain() -> Result<(), ChainIntegrityError>`

---

## E1.3 — RED Metrics Per Endpoint

Rate, Error rate, Duration histogram per route.

### Design
- `metrics` crate labels: `method`, `path`, `status`
- Tower middleware layer records: `http_requests_total`, `http_request_duration_seconds`
- Existing `/metrics` Prometheus endpoint already scrapes the global recorder

---

## W2.3 — Fault Debouncing + FaultGovernor

### Design
- `FaultGovernor` wraps `FaultBridge`: debounce window, operation-cycle reset
- Configurable per-fault debounce interval
- `clear_faults` resets debounce timers

---

## W2.4 — Richer Mode/Session Model

Map UDS session types to SOVD mode semantics.

### Design
- Extend `SovdMode.available_modes` with structured mode descriptors
- Add `SovdModeDescriptor { id, name, description, session_type }`
- `session_type`: Default, Extended, Programming, SafetySystem
- `set_mode` validates transition rules

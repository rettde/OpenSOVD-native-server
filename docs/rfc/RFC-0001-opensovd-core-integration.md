# RFC-0001: Integration into Eclipse OpenSOVD Core

| Field         | Value |
|---------------|-------|
| **Status**    | Proposed |
| **Date**      | 2026-03-21 |
| **Authors**   | OpenSOVD-native-server maintainers |
| **Target**    | `eclipse-opensovd/opensovd-core` v0.5.0 |
| **Tracking**  | [opensovd#43](https://github.com/eclipse-opensovd/opensovd/issues/43), [opensovd-core#9](https://github.com/eclipse-opensovd/opensovd-core/issues/9), [opensovd-core#26](https://github.com/eclipse-opensovd/opensovd-core/issues/26), [opensovd-core#24](https://github.com/eclipse-opensovd/opensovd-core/issues/24) |

---

## 1. Summary

This RFC proposes integrating the OpenSOVD-native-server codebase into the
Eclipse OpenSOVD `opensovd-core` repository, aligning with the architecture
established on the `inc/liebherr` branch. The integration contributes fault
management, rate limiting, observability, vendor-extension endpoints, and
additional ISO 17978-3 conformance tooling — all identified as open needs in
the OpenSOVD MVP roadmap (26Q2–26Q4).

The result is a single coherent workspace at **v0.5.0** that extends
opensovd-core's current `Topology`/`DataProvider`/`ServerBuilder` architecture
with production-grade capabilities required by S-CORE v1.0.

---

## 2. Motivation

### 2.1 Community needs (unmet)

The following needs have been expressed in the Eclipse OpenSOVD project but
lack implementations today:

| Issue / Discussion | Need | MVP timeline |
|--------------------|------|-------------|
| opensovd-core#26 | DID / data-point extensibility | 26Q2 |
| opensovd-core#24 | mTLS support | 26Q2 |
| opensovd-core#9 | Gateway vs. Server architecture clarity | 26Q2 |
| opensovd#43 | ISO 17978-3 conformance guidelines | Ongoing |
| opensovd#38 | `/modes/security` vs `/modes/authentication` | Pending standardization |
| MVP roadmap 26Q4 | Rate limits, logging, observability | 26Q4 |
| MVP roadmap 26Q2 | Fault handling (DFM), DTC read/clear | 26Q2–Q3 |

### 2.2 What we bring

The OpenSOVD-native-server (v0.18.0-rc) provides:

- **489 tests**, Clippy pedantic clean, CI with 10 jobs
- **51/51 mandatory ISO 17978-3 requirements** implemented
- **CDF validation** via DSA sovd-cdf-validator (Redocly plugin)
- **Fault management**: `FaultBridge` → `FaultGovernor` → `FaultManager` pipeline
  with debouncing, hash-chained audit, and persistent storage
- **Auth**: OIDC/JWT authentication, RBAC authorization, OEM-profile plugin
- **Observability**: OTLP tracing, RED metrics middleware, structured JSON logging
- **Rate limiting**: per-client token-bucket keyed by JWT `sub`
- **Vendor extensions**: RXSWIN (UNECE R156), TARA (ISO 21434), UCM (AUTOSAR),
  UDS Security Access (ISO 14229 §9)
- **Enterprise**: multi-tenant isolation, cloud bridge, canary routing,
  signed audit export, compliance evidence endpoint

### 2.3 Why integrate (not fork)

- **Single Rust workspace** avoids duplicate abstractions (`Topology`, auth
  layers, entity model)
- **Shared CI** via eclipse-opensovd/cicd-workflows
- Contributors only learn one crate ecosystem
- OpenSOVD MVP roadmap items are addressed without parallel development

---

## 3. Design

### 3.1 Crate mapping

The integration maps our workspace crates onto the opensovd-core crate
structure (inc/liebherr branch). No existing opensovd-core crate is deleted;
our code is either merged into existing crates or contributed as new crates.

```
┌─────────────────────────┐       ┌──────────────────────────────┐
│  native-interfaces      │       │  opensovd-models  (existing) │
│  ├── sovd.rs (types)    │ ───▶  │  + fault models              │
│  ├── oem.rs             │       │  + vendor-ext models         │
│  ├── backend.rs         │       ├──────────────────────────────┤
│  ├── storage.rs         │       │  opensovd-core  (existing)   │
│  ├── secrets.rs         │ ───▶  │  + FaultProvider trait       │
│  ├── tenant.rs          │       │  + OperationProvider trait   │
│  ├── bridge.rs          │       │  + ModeProvider trait        │
│  └── data_catalog.rs    │       │  + LockProvider trait        │
└─────────────────────────┘       └──────────────────────────────┘

┌─────────────────────────┐       ┌──────────────────────────────┐
│  native-core            │       │  opensovd-providers (exist.) │
│  ├── http_backend.rs    │ ───▶  │  + HttpComponentProvider     │
│  ├── router.rs          │       │  + FaultBridge               │
│  ├── fault_bridge.rs    │       ├──────────────────────────────┤
│  ├── fault_governor.rs  │       │  opensovd-faults  (NEW)      │
│  ├── fault_manager.rs   │ ───▶  │  FaultGovernor, FaultManager │
│  └── audit_log.rs       │       │  DFM pipeline, hash-chain    │
└─────────────────────────┘       └──────────────────────────────┘

┌─────────────────────────┐       ┌──────────────────────────────┐
│  native-sovd            │       │  opensovd-server  (existing) │
│  ├── routes.rs          │ ───▶  │  + fault routes              │
│  ├── auth.rs            │       │  + vendor-ext routes         │
│  ├── rate_limit.rs      │       ├──────────────────────────────┤
│  ├── bridge.rs          │       │  opensovd-extra  (existing)  │
│  ├── openapi.rs         │ ───▶  │  + OIDC authenticator        │
│  └── oem_sample.rs      │       │  + RBAC authorizer           │
└─────────────────────────┘       │  + RateLimitLayer            │
                                  │  + OemProfile plugin         │
┌─────────────────────────┐       ├──────────────────────────────┤
│  native-server          │ ───▶  │  opensovd-cli/server (exist.)│
│  └── main.rs            │       │  (adopts ServerBuilder)      │
└─────────────────────────┘       ├──────────────────────────────┤
                                  │  opensovd-extensions  (NEW)  │
┌─────────────────────────┐       │  RXSWIN, TARA, UCM, UDS-SA  │
│  native-comm-someip     │       ├──────────────────────────────┤
│  └── (SOME/IP backend)  │ ───▶  │  opensovd-someip  (NEW)     │
└─────────────────────────┘       └──────────────────────────────┘
```

### 3.2 Trait alignment

The central design change is decomposing our monolithic `ComponentBackend`
trait into granular provider traits that extend opensovd-core's existing
`DataProvider`:

```rust
// ═══ Existing in opensovd-core (unchanged) ═══

#[async_trait]
pub trait DataProvider: Send + Sync + 'static {
    async fn list(&self, filter: DataFilter) -> Result<Vec<Metadata>>;
    async fn read(&self, data_id: &str, include_schema: bool) -> Result<Data>;
    async fn write(&self, data_id: &str, value: serde_json::Value) -> Result<()>;
    async fn categories(&self) -> Result<Vec<CategoryInfo>>;
    async fn groups(&self, cat: Option<&str>) -> Result<Vec<GroupInfo>>;
    async fn tags(&self) -> Result<Vec<TagInfo>>;
}

// ═══ New traits contributed by this RFC ═══

/// ISO 17978-3 §7.8 — Fault handling (DTC read, clear, subscribe)
#[async_trait]
pub trait FaultProvider: Send + Sync + 'static {
    async fn list_faults(&self, filter: FaultFilter) -> Result<Vec<SovdFault>>;
    async fn get_fault(&self, fault_id: &str) -> Result<SovdFault>;
    async fn clear_fault(&self, fault_id: &str) -> Result<()>;
    async fn clear_all_faults(&self) -> Result<u64>;
}

/// ISO 17978-3 §7.14 — Control of operations
#[async_trait]
pub trait OperationProvider: Send + Sync + 'static {
    async fn list_operations(&self) -> Result<Vec<OperationInfo>>;
    async fn execute(&self, op_id: &str, params: Value) -> Result<ExecutionHandle>;
    async fn get_execution(&self, op_id: &str, exec_id: &str) -> Result<Execution>;
}

/// ISO 17978-3 §7.16 — Target modes
#[async_trait]
pub trait ModeProvider: Send + Sync + 'static {
    async fn list_modes(&self) -> Result<Vec<SovdMode>>;
    async fn activate_mode(&self, mode_id: &str) -> Result<()>;
}

/// ISO 17978-3 §7.17 — Locking
#[async_trait]
pub trait LockProvider: Send + Sync + 'static {
    async fn acquire_lock(&self, entity_id: &str, timeout: Duration) -> Result<LockHandle>;
    async fn release_lock(&self, lock_id: &str) -> Result<()>;
}
```

The existing `ComponentBackend` continues to work internally via a blanket
adapter that delegates to the individual provider traits. This preserves
backward compatibility for existing backends while exposing the standard
interface to opensovd-core consumers.

### 3.3 Topology migration

Our `ComponentRouter` registry is replaced by opensovd-core's `Topology`:

| Current (native) | Target (opensovd-core) |
|-------------------|----------------------|
| `ComponentRouter::register(id, Arc<dyn ComponentBackend>)` | `Topology::write().add_component(Component::new(id, name))` + provider registry |
| `ComponentRouter::resolve(id) -> Arc<dyn ComponentBackend>` | `Topology::read().get_component(id)` + `ProviderRegistry::get::<dyn DataProvider>(id)` |
| Static registration at startup | `DiscoveryProvider` trait for runtime changes |

A new `ProviderRegistry` (inspired by `TypeMap`) holds `Arc<dyn T>` per
entity ID for each provider trait. The `ServerBuilder` accepts providers
via a new `.provider()` method:

```rust
let server = Server::builder()
    .listener(listener)
    .base_uri("http://0.0.0.0:8080/sovd")?
    .topology(topology)
    .provider::<dyn DataProvider>("ecu1", Arc::new(ecu1_data))
    .provider::<dyn FaultProvider>("ecu1", Arc::new(ecu1_faults))
    .discovery(Box::new(static_discovery))
    .authenticator(oidc_auth)
    .authorizer(rbac)
    .layer(rate_limit_layer)
    .build()?;
```

### 3.4 Auth adaptation

opensovd-core defines generic auth traits. Our OIDC/JWT implementation
becomes a concrete `Authenticator`:

```rust
/// OIDC/JWT bearer token authentication.
pub struct OidcAuthenticator { /* jwks_uri, issuer, audience */ }

impl Authenticator for OidcAuthenticator {
    type Identity = JwtClaims;
    fn authenticate(&self, parts: &Parts) -> Result<JwtClaims, AuthError> {
        // Extract Bearer token, validate signature + claims
    }
}

/// Role-based access control from JWT claims.
pub struct RbacAuthorizer { /* RbacConfig */ }

impl Authorizer<JwtClaims> for RbacAuthorizer {
    fn authorize(&self, claims: &JwtClaims, parts: &Parts) -> Result<(), AuthError> {
        // Check role against required permission for this route
    }
}
```

The `OemProfile` system remains as an extension point within `RbacAuthorizer`:
each OEM profile can customize entity ID validation, CDF policy, and
authorization rules via the existing sub-traits.

### 3.5 New crate: `opensovd-faults`

This crate addresses the most urgent MVP gap (26Q2–Q3):

```
opensovd-faults/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── bridge.rs       ← FaultBridge (UDS DTC → SovdFault mapping)
    ├── governor.rs     ← FaultGovernor (debounce, dedup, rate-limit)
    ├── manager.rs      ← FaultManager (lifecycle, persistence, queries)
    ├── audit.rs        ← AuditLog (SHA-256 hash-chain, sinks)
    └── storage.rs      ← FaultStore trait + InMemoryFaultStore
```

Depends on `opensovd-core` (for `FaultProvider`) and `opensovd-models`
(for `SovdFault`, `FaultFilter`). The fault pipeline is:

```
External event → FaultBridge → FaultGovernor → FaultManager → FaultStore
                  (mapping)     (debounce)      (lifecycle)    (persist)
```

### 3.6 New crate: `opensovd-extensions`

Vendor-extension endpoints that are not part of ISO 17978-3 but address
real-world regulatory and diagnostic needs:

```
opensovd-extensions/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── rxswin.rs         ← UNECE R156 RXSWIN tracking
    ├── tara.rs           ← ISO/SAE 21434 threat analysis
    ├── ucm.rs            ← AUTOSAR UCM campaign lifecycle
    ├── uds_security.rs   ← ISO 14229 §9 seed/key protocol
    └── provenance.rs     ← Update provenance log
```

All extension routes are registered under `/x-*` prefixed paths per
OpenAPI 3.x §4.8.1 (Specification Extensions). They are gated behind
a Cargo feature flag `extensions` so downstream consumers can opt in.

### 3.7 What is removed or replaced

| Current artifact | Action | Rationale |
|-----------------|--------|-----------|
| `native-sovd/src/openapi.rs` | **Remove** | opensovd-core uses schema-first approach (ISO YAML → code-gen). CDF validation remains as CI job. |
| `native-core/src/router.rs` (`ComponentRouter`) | **Replace** with `Topology` + `ProviderRegistry` | Avoids duplicating entity management |
| `native-sovd/src/bridge.rs` | **Adapt** to gateway architecture in `opensovd-cli/gateway` | Gateway is a separate binary in opensovd-core |
| `native-health/` | **Inline** as a route in `opensovd-server` | opensovd-core does not have a separate health crate |

---

## 4. Migration phases

### Phase 1 — Structure alignment (~1 day)

Mechanical refactoring without behavioral changes:

1. Rename crate directories: `native-*` → `opensovd-*`
2. Update `Cargo.toml` workspace members and dependency paths
3. Align SPDX headers to Eclipse Foundation format
4. Adopt workspace lint configuration from inc/liebherr (`clippy::pedantic = deny`)
5. Move binary from `native-server/` to `opensovd-cli/server/`
6. Inline `native-health` routes into `opensovd-server`

**Verification:** `cargo test --workspace` passes, `cargo clippy --workspace` clean.

### Phase 2 — Core trait integration (~3 days)

Adopt opensovd-core's abstractions:

1. **DataProvider adapter:** Implement `DataProvider` for our `HttpBackend`,
   mapping `list()`→`list_data()`, `read()`→`read_data()`, `write()`→`write_data()`
2. **Topology migration:** Replace `ComponentRouter` with `Topology` +
   `ProviderRegistry`; implement `DiscoveryProvider` (static config)
3. **New provider traits:** Extract `FaultProvider`, `OperationProvider`,
   `ModeProvider`, `LockProvider` from `ComponentBackend`
4. **Blanket adapter:** `impl<T: ComponentBackend> DataProvider for T` for
   backward compatibility

**Verification:** All existing tests pass through the new trait layer.

### Phase 3 — ServerBuilder adoption (~2 days)

Replace manual axum router construction with `ServerBuilder`:

1. Adopt `Server::builder()` pattern in `main.rs`
2. Register providers via `.provider::<dyn T>(entity_id, impl)`
3. Wire auth via `.authenticator()` / `.authorizer()` instead of custom middleware
4. Register rate limiting and OTLP as `.layer()` extensions

**Verification:** Server starts with identical behavior; integration tests pass.

### Phase 4 — Auth generalization (~1 day)

1. Implement `Authenticator` for `OidcAuthenticator`
2. Implement `Authorizer<JwtClaims>` for `RbacAuthorizer`
3. Adapt `OemProfile` to plug into `RbacAuthorizer` as extension
4. Support opensovd-extra's existing Rego authorizer as alternative

**Verification:** Auth tests pass; both OIDC and Rego paths functional.

### Phase 5 — Feature crate extraction (~4 days)

Extract standalone crates from our monolith:

1. `opensovd-faults` — fault pipeline (bridge → governor → manager → store)
2. `opensovd-extensions` — RXSWIN, TARA, UCM, UDS Security (feature-gated)
3. Contribute rate limiter + OTLP + audit log to `opensovd-extra`
4. `opensovd-someip` — SOME/IP communication backend (existing, rename only)

**Verification:** All 489 tests pass; feature flags work correctly.

### Phase 6 — Cleanup (~1 day)

1. Remove `openapi.rs` (code-gen replaced by schema-first)
2. Remove `ComponentRouter` (replaced by `Topology`)
3. Adapt bridge module to gateway binary architecture
4. Update all documentation, ADRs, and README

**Verification:** `cargo test --workspace`, CI green, documentation consistent.

### Timeline

```
Phase 1 ████░░░░░░░░░░░░░░░░░░░░  1 day
Phase 2 ░░░░████████████░░░░░░░░░  3 days
Phase 3 ░░░░░░░░░░░░░░░░████████░  2 days
Phase 4 ░░░░░░░░░░░░░░░░░░░░░░██  1 day
Phase 5 ░░░░░░░░░░░░░░░░░░░░░░░░  4 days (parallel with Phase 4)
Phase 6 ░░░░░░░░░░░░░░░░░░░░░░░░  1 day
────────────────────────────────────────
Total                               ~12 working days
```

---

## 5. Compatibility

### 5.1 opensovd-core compatibility

| opensovd-core API | Impact |
|-------------------|--------|
| `Topology` | Adopted as-is |
| `DataProvider` | Adopted as-is; our `HttpBackend` implements it |
| `DiscoveryProvider` | New `StaticDiscoveryProvider` contributed |
| `ServerBuilder` | Adopted as-is; extended with `.provider()` |
| `Authenticator` / `Authorizer` | Adopted; `OidcAuthenticator` + `RbacAuthorizer` contributed |
| `opensovd-models` | Extended with fault, mode, lock, operation models |

No existing opensovd-core API is changed or removed. All additions are
backward-compatible extensions.

### 5.2 Breaking changes (from native-server perspective)

| What changes | Migration path |
|-------------|---------------|
| `ComponentBackend` trait split into granular providers | Blanket adapter provided; existing backends compile unchanged |
| `ComponentRouter` removed | Use `Topology` + `ProviderRegistry` |
| `openapi.rs` removed | Use schema-first ISO YAML; CDF validation stays in CI |
| Crate names change (`native-*` → `opensovd-*`) | Search-and-replace in `use` statements |
| `AppState` structure changes | `ServerBuilder` handles state construction |

### 5.3 Feature flag matrix

| Flag | Crate | What it enables |
|------|-------|----------------|
| `faults` | opensovd-faults | Fault management pipeline |
| `extensions` | opensovd-extensions | RXSWIN, TARA, UCM, UDS-SA endpoints |
| `someip` | opensovd-someip | SOME/IP communication backend |
| `otlp` | opensovd-extra | OpenTelemetry OTLP trace export |
| `persist` | opensovd-faults | Sled-based persistent fault storage |
| `vault` | opensovd-extra | HashiCorp Vault secret provider |

---

## 6. Alternatives considered

### 6.1 Separate repository under eclipse-opensovd

Keep OpenSOVD-native-server as `eclipse-opensovd/opensovd-native` alongside
`opensovd-core`.

**Rejected because:**
- Duplicates core abstractions (Topology, auth, entity model)
- Two Rust workspaces with different conventions confuse contributors
- MVP roadmap items must be integrated into opensovd-core anyway

### 6.2 Full rewrite on top of opensovd-core

Discard native-server code; reimplement features directly in opensovd-core.

**Rejected because:**
- 489 tests and 51/51 ISO conformance would need to be rebuilt
- Estimated 6–8 weeks vs. 12 days for integration
- No structural advantage — the code is already Rust + axum + async-trait

### 6.3 Plugin architecture (dynamic loading)

Ship native-server features as `cdylib` plugins loaded at runtime.

**Rejected because:**
- Rust FFI boundary negates trait safety and generics
- No precedent in opensovd-core or S-CORE ecosystem
- Cargo feature flags achieve the same opt-in granularity statically

---

## 7. Open questions

1. **CDA integration point:** Should `opensovd-faults` depend on CDA's
   fault library (`eclipse-opensovd/fault-lib`), or provide its own
   `FaultBridge`? The current proposal keeps them independent with a shared
   `FaultProvider` trait.

2. **OpenAPI generation:** opensovd-core (inc/liebherr) does not generate
   an OpenAPI spec from code. Should the new fault and extension routes
   contribute to a centrally maintained YAML, or should we preserve
   code-generated OpenAPI as an option?

3. **Governance:** Which workstream owns the new crates? Proposal:
   `opensovd-faults` → Core workstream, `opensovd-extensions` → CDA
   workstream (since extensions overlap with UDS diagnostics).

4. **MSRV:** opensovd-core (inc/liebherr) uses `edition = "2024"`. Our
   codebase uses `edition = "2021"` with MSRV 1.88. Alignment to edition
   2024 is required.

---

## 8. References

- [Eclipse OpenSOVD MVP Roadmap](https://github.com/eclipse-opensovd/opensovd/blob/main/docs/design/mvp.md)
- [opensovd-core inc/liebherr branch](https://github.com/eclipse-opensovd/opensovd-core/tree/inc/liebherr) — target architecture
- [ISO 17978-3 Conformance Guidelines (opensovd#43)](https://github.com/eclipse-opensovd/opensovd/issues/43)
- [Gateway vs Server Architecture (opensovd-core#9)](https://github.com/eclipse-opensovd/opensovd-core/issues/9)
- [DID / DataProvider Extensibility (opensovd-core#26)](https://github.com/eclipse-opensovd/opensovd-core/issues/26)
- [mTLS Support (opensovd-core#24)](https://github.com/eclipse-opensovd/opensovd-core/issues/24)
- [RFC: Kotlin→Rust for odx-converter (opensovd#83)](https://github.com/eclipse-opensovd/opensovd/discussions/83)
- [SOVD CDF Validator — vendor-extension fix](https://github.com/dsagmbh/sovd-cdf-validator/pull/1)

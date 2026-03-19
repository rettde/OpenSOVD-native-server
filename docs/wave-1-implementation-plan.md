# Wave 1 — Implementation Plan

**Scope:** Fine-grained authorization, diagnostic audit trail, full `apps`/`funcs` entities, software-package lifecycle.

**Target version:** 0.7.0

---

## Table of Contents

1. [Overview](#1-overview)
2. [W1.1 — Fine-Grained Authorization (AuthZ)](#w11--fine-grained-authorization-authz)
3. [W1.2 — Diagnostic Audit Trail](#w12--diagnostic-audit-trail)
4. [W1.3 — Full Apps / Funcs Entities](#w13--full-apps--funcs-entities)
5. [W1.4 — Software-Package Lifecycle](#w14--software-package-lifecycle)
6. [Cross-Cutting Concerns](#6-cross-cutting-concerns)
7. [Dependency & Ordering](#7-dependency--ordering)
8. [Test Strategy](#8-test-strategy)
9. [Files Changed Summary](#9-files-changed-summary)

---

## 1. Overview

Wave 1 extends the server from "authenticate the caller" to "authorize every action and record it", while simultaneously filling the two biggest SOVD entity gaps (`apps`/`funcs`) and turning the software-packages endpoint from a thin passthrough into a real OTA lifecycle model.

### Design principles

- **Additive, not breaking.** All new features are opt-in via configuration or OEM profile hooks. Existing behavior is unchanged when not configured.
- **OemProfile as policy anchor.** New policy sub-traits extend the existing `OemProfile` hierarchy in `native-interfaces/src/oem.rs`. `DefaultProfile` gets permissive defaults.
- **Minimal new crates.** No new workspace crates unless strictly necessary. New modules are added to existing crates.

---

## W1.1 — Fine-Grained Authorization (AuthZ)

### Current state

- `auth.rs` validates credentials (API key / JWT / OIDC) and calls `AuthPolicy::validate_claims()`.
- `validate_claims` receives a flat `HashMap<String, Value>` and the request path.
- There is **no per-resource, per-operation, or per-entity authorization** — once authenticated, all actions are allowed.

### Design

Introduce a new sub-trait `AuthzPolicy` in the `OemProfile` hierarchy that the auth middleware calls **after** successful authentication.

```
┌─────────────────────────────────────────────────────────────────┐
│                         OemProfile                              │
│  AuthPolicy │ EntityIdPolicy │ DiscoveryPolicy │ CdfPolicy      │
│  ─── NEW ──────────────────────────────────────────────────── │
│  AuthzPolicy                                                    │
│    fn authorize(&self, ctx: &AuthzContext) -> AuthzDecision     │
└─────────────────────────────────────────────────────────────────┘
```

### Data types

```rust
// native-interfaces/src/oem.rs (new types)

/// Context for authorization decisions
pub struct AuthzContext {
    /// Authenticated caller identity (from JWT sub / API key)
    pub caller: String,
    /// Parsed JWT roles (from `roles` claim), empty if API key
    pub roles: Vec<String>,
    /// OAuth2 scopes (from `scope` / `scp` claim)
    pub scopes: Vec<String>,
    /// HTTP method: GET, POST, PUT, DELETE, PATCH
    pub method: String,
    /// Entity type being accessed: "component", "app", "func", "group", "server"
    pub entity_type: String,
    /// Entity ID (e.g. component_id, app_id), None for collection endpoints
    pub entity_id: Option<String>,
    /// Resource being accessed: "data", "faults", "operations", "configurations",
    ///   "software-packages", "lock", "mode", "logs", "proximity-challenge"
    pub resource: String,
    /// Sub-resource ID (e.g. data_id, fault_id, op_id), None for collections
    pub resource_id: Option<String>,
    /// Full request path (for fallback matching)
    pub path: String,
}

/// Authorization decision
pub enum AuthzDecision {
    /// Allow the request
    Allow,
    /// Deny with HTTP status and error message
    Deny { status: u16, code: String, message: String },
}
```

### New sub-trait

```rust
// native-interfaces/src/oem.rs

pub trait AuthzPolicy: Send + Sync {
    /// Authorize a request after authentication has succeeded.
    /// Default: allow everything (standard SOVD — no restrictions).
    fn authorize(&self, _ctx: &AuthzContext) -> AuthzDecision {
        AuthzDecision::Allow
    }
}
```

### Integration points

| File | Change |
|------|--------|
| `native-interfaces/src/oem.rs` | Add `AuthzContext`, `AuthzDecision`, `AuthzPolicy` trait. Add `AuthzPolicy` to `OemProfile` supertrait list. Add `as_authz_policy()` to `OemProfile`. Implement permissive default for `DefaultProfile`. |
| `native-sovd/src/auth.rs` | After successful auth (JWT/API key validated), build `AuthzContext` from request metadata and call `auth_state.oem_profile.as_authz_policy().authorize(&ctx)`. Reject with returned status/code on `Deny`. |
| `native-sovd/src/routes.rs` | Extract `AuthzContext` fields from matched route template. Add helper `fn build_authz_context(method, matched_path, uri, caller) -> AuthzContext` that parses entity_type/entity_id/resource/resource_id from the route structure. |
| `native-sovd/src/oem_sample.rs` | Add `AuthzPolicy` impl with documented example (e.g. read-only role can only GET). |

### Implementation steps

1. **Add types and trait** in `native-interfaces/src/oem.rs`
2. **Update `OemProfile`** supertrait bound and `DefaultProfile`
3. **Add `AuthzContext` builder** in `routes.rs` (parses matched path into semantic fields)
4. **Wire into `auth_middleware`** in `auth.rs` — call after auth success, before `next.run()`
5. **Update `oem_sample.rs`** with documented example
6. **Update `oem_mbds.rs`** (if present) — add `AuthzPolicy` impl
7. **Tests:** ~15 new tests in `auth.rs` and `routes.rs`

### Estimated effort: M (2–3 days)

---

## W1.2 — Diagnostic Audit Trail

### Current state

- `DiagLog` in `native-core/src/diag_log.rs` is a ring-buffer for diagnostic log entries (SOVD §7.10).
- It captures operational diagnostic events but is **not an audit trail** — no caller identity, no action classification, no tamper resistance.

### Design

Add a new `AuditLog` module in `native-core` that records every mutating or security-sensitive action with caller identity, timestamp, action type, target, and outcome.

### Data types

```rust
// native-interfaces/src/sovd.rs (new types)

/// Audit trail entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdAuditEntry {
    /// Unique entry ID (monotonic counter or UUID)
    pub id: String,
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Authenticated caller identity
    pub caller: String,
    /// Action performed
    pub action: SovdAuditAction,
    /// Target entity type + ID
    pub target: String,
    /// Resource (e.g. "data/rpm", "faults", "lock")
    pub resource: String,
    /// Outcome: "success", "denied", "error"
    pub outcome: String,
    /// Optional detail (e.g. error message, written value summary)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Trace ID (from W3C traceparent) for correlation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SovdAuditAction {
    ReadData,
    WriteData,
    ReadFaults,
    ClearFaults,
    ExecuteOperation,
    CancelExecution,
    AcquireLock,
    ReleaseLock,
    ReadConfig,
    WriteConfig,
    InstallPackage,
    ProximityChallenge,
    SetMode,
    Connect,
    Disconnect,
    AuthSuccess,
    AuthFailure,
    AuthzDenied,
}
```

### New module: `native-core/src/audit_log.rs`

```rust
pub struct AuditLog {
    entries: Mutex<VecDeque<SovdAuditEntry>>,
    max_entries: usize,
    /// Optional: also write to append-only file (one JSON line per entry)
    file_sink: Option<Mutex<std::io::BufWriter<std::fs::File>>>,
}
```

**Key methods:**

- `fn record(&self, entry: SovdAuditEntry)` — append + optional file flush
- `fn query(&self, filter: AuditFilter) -> Vec<SovdAuditEntry>` — caller, action, target, time range
- `fn recent(&self, count: usize) -> Vec<SovdAuditEntry>`

**File sink (tamper resistance):**

- Append-only JSONL file at a configurable path (e.g. `data/audit.jsonl`)
- Each line is a self-contained JSON object — easy to ship to SIEM/log aggregation
- Optional HMAC-SHA256 chaining: each entry includes `prev_hash` field for tamper detection

### REST API endpoint

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/sovd/v1/audit` | List audit entries (paginated, filterable by caller/action/target/time) |

### Integration points

| File | Change |
|------|--------|
| `native-interfaces/src/sovd.rs` | Add `SovdAuditEntry`, `SovdAuditAction` types |
| `native-core/src/audit_log.rs` | **New file** — `AuditLog` struct with memory + file backends |
| `native-core/src/lib.rs` | Add `pub mod audit_log; pub use audit_log::AuditLog;` |
| `native-sovd/src/state.rs` | Add `pub audit_log: Arc<AuditLog>` to `AppState` |
| `native-sovd/src/routes.rs` | Add `/audit` route. Instrument every mutating handler to call `state.audit_log.record(...)` |
| `native-sovd/src/auth.rs` | Record `AuthSuccess`, `AuthFailure`, `AuthzDenied` events |
| `native-server/src/main.rs` | Construct `AuditLog` (config: path, max_entries, hmac_key) and inject into `AppState` |
| `config/opensovd-native-server.toml` | Add `[audit]` section |

### Configuration

```toml
[audit]
enabled = true
max_entries = 10000
# Optional: append-only file for tamper-resistant persistence
file_path = "data/audit.jsonl"
# Optional: HMAC key for hash chaining (hex-encoded)
# hmac_key = "..."
```

### Implementation steps

1. **Add types** in `native-interfaces/src/sovd.rs`
2. **Create `audit_log.rs`** in `native-core`
3. **Wire into `AppState`** and `main.rs`
4. **Add `/audit` route** in `routes.rs`
5. **Instrument mutating handlers** — wrap each handler's success/error path with audit record
6. **Instrument auth middleware** — record auth/authz events
7. **Add HMAC chaining** (optional, behind config flag)
8. **Tests:** ~20 new tests (unit + integration)

### Estimated effort: M (2–3 days)

---

## W1.3 — Full Apps / Funcs Entities

### Current state

- `/apps` and `/funcs` exist as routes but return **empty collections** (stubs).
- `DiscoveryPolicy` has `apps_enabled()` and `funcs_enabled()` toggles.
- The `ComponentBackend` trait and `ComponentRouter` are purely component-centric — no `AppBackend` or `FuncBackend` abstraction exists.
- No SOVD types for `SovdApp` or `SovdFunc` exist in `native-interfaces/src/sovd.rs`.

### Design

SOVD entities (ISO 17978-3 §4.2.3) define a hierarchy:

```
SOVDServer
 ├── Components  (ECUs, CDAs — already implemented)
 ├── Apps         (diagnostic applications)
 ├── Funcs        (cross-component diagnostic functions)
 └── Areas        (E/E architecture zones — deferred, often OEM-forbidden)
```

Apps and Funcs share the same resource structure as Components (data, faults, operations, configurations, logs) but represent higher-level abstractions:

- **App** = a diagnostic application hosted on the HPC (e.g. "SOTA Manager", "Flash Master", "Health Monitor")
- **Func** = a cross-component function that aggregates data from multiple sources (e.g. "powertrain-status", "battery-health")

### New SOVD types

```rust
// native-interfaces/src/sovd.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdApp {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub version: String,
    pub status: SovdAppStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SovdAppStatus {
    Running,
    Stopped,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdFunc {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Component IDs this function aggregates
    #[serde(rename = "sourceComponents")]
    pub source_components: Vec<String>,
}
```

### New backend trait: `EntityBackend`

Rather than bloating `ComponentBackend`, introduce a new trait for non-component entities:

```rust
// native-interfaces/src/backend.rs

#[async_trait]
pub trait EntityBackend: Send + Sync {
    // ── Apps ───────────────────────────────────────────────
    fn list_apps(&self) -> Vec<SovdApp>;
    fn get_app(&self, app_id: &str) -> Option<SovdApp>;
    fn list_app_data(&self, app_id: &str) -> Result<Vec<SovdDataCatalogEntry>, DiagServiceError>;
    async fn read_app_data(&self, app_id: &str, data_id: &str) -> Result<serde_json::Value, DiagServiceError>;
    fn list_app_operations(&self, app_id: &str) -> Result<Vec<SovdOperation>, DiagServiceError>;
    async fn execute_app_operation(&self, app_id: &str, op_id: &str, params: Option<&[u8]>) -> Result<serde_json::Value, DiagServiceError>;
    fn get_app_capabilities(&self, app_id: &str) -> Result<SovdCapabilities, DiagServiceError>;

    // ── Funcs ──────────────────────────────────────────────
    fn list_funcs(&self) -> Vec<SovdFunc>;
    fn get_func(&self, func_id: &str) -> Option<SovdFunc>;
    fn list_func_data(&self, func_id: &str) -> Result<Vec<SovdDataCatalogEntry>, DiagServiceError>;
    async fn read_func_data(&self, func_id: &str, data_id: &str) -> Result<serde_json::Value, DiagServiceError>;
}
```

**Default implementations** return empty/not-found — backends only override what they support.

### New routes

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/sovd/v1/apps` | List apps (paginated) |
| `GET` | `/sovd/v1/apps/{app_id}` | Get app details |
| `GET` | `/sovd/v1/apps/{app_id}/capabilities` | App capabilities |
| `GET` | `/sovd/v1/apps/{app_id}/data` | List app data |
| `GET` | `/sovd/v1/apps/{app_id}/data/{data_id}` | Read app data |
| `GET` | `/sovd/v1/apps/{app_id}/operations` | List app operations |
| `POST` | `/sovd/v1/apps/{app_id}/operations/{op_id}` | Execute app operation |
| `GET` | `/sovd/v1/funcs` | List funcs (paginated) |
| `GET` | `/sovd/v1/funcs/{func_id}` | Get func details |
| `GET` | `/sovd/v1/funcs/{func_id}/data` | List func data |
| `GET` | `/sovd/v1/funcs/{func_id}/data/{data_id}` | Read func data |

### Integration points

| File | Change |
|------|--------|
| `native-interfaces/src/sovd.rs` | Add `SovdApp`, `SovdAppStatus`, `SovdFunc` types |
| `native-interfaces/src/backend.rs` | Add `EntityBackend` trait with default impls |
| `native-interfaces/src/lib.rs` | Re-export `EntityBackend` |
| `native-core/src/router.rs` | Make `ComponentRouter` also implement `EntityBackend` by aggregating from backends (same pattern as `list_groups()`) |
| `native-sovd/src/state.rs` | Add `pub entity_backend: Arc<dyn EntityBackend>` to `AppState` (or reuse `backend` if we merge) |
| `native-sovd/src/routes.rs` | Replace `list_apps`/`list_funcs` stubs with real handlers. Add nested resource routes. Guard with `DiscoveryPolicy`. |
| `native-sovd/src/openapi.rs` | Add `/apps` and `/funcs` paths with nested resources to CDF |
| `examples/demo-ecu/` | Add example apps and funcs (e.g. "health-monitor" app, "powertrain-status" func) |

### Implementation steps

1. **Add SOVD types** (`SovdApp`, `SovdFunc`) in `sovd.rs`
2. **Add `EntityBackend` trait** in `backend.rs` with default impls
3. **Implement `EntityBackend` for `ComponentRouter`** (aggregate from backends)
4. **Implement `EntityBackend` for `SovdHttpBackend`** (forward to external servers)
5. **Wire into `AppState`**
6. **Replace route stubs** with real handlers + add nested resource routes
7. **Update OpenAPI/CDF** in `openapi.rs`
8. **Update demo-ecu** with sample apps/funcs
9. **Respect `DiscoveryPolicy`** — return 404 when disabled
10. **Tests:** ~25 new tests

### Estimated effort: L (3–5 days)

---

## W1.4 — Software-Package Lifecycle

### Current state

- `SovdSoftwarePackage` has: `id`, `name`, `version`, `description`, `status` (Available/Installing/Installed/Failed).
- `ComponentBackend` has `list_software_packages()` and `install_software_package()`.
- Routes: `GET .../software-packages`, `POST .../software-packages/{id}`, `GET .../software-packages/{id}/status`.
- **Missing:** manifest upload, progress tracking, rollback, activation, campaign model.

### Design

Extend the existing model into a full lifecycle:

```
Available → Downloading → Downloaded → Installing → Installed → Activated
                ↓              ↓            ↓            ↓
              Failed         Failed       Failed      Rollback → Previous
```

### Extended types

```rust
// native-interfaces/src/sovd.rs (extend existing)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SovdSoftwarePackageStatus {
    Available,
    Downloading,
    Downloaded,
    Installing,
    Installed,
    Activated,
    RollingBack,
    Failed,
}

/// Extended software package with lifecycle fields
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdSoftwarePackage {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: SovdSoftwarePackageStatus,
    // ── New fields ──
    /// Previous version (for rollback)
    #[serde(skip_serializing_if = "Option::is_none", rename = "previousVersion")]
    pub previous_version: Option<String>,
    /// Installation progress (0–100)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    /// Target component ID
    #[serde(rename = "componentId")]
    pub component_id: String,
    /// Timestamp of last status change
    #[serde(skip_serializing_if = "Option::is_none", rename = "updatedAt")]
    pub updated_at: Option<String>,
    /// Error detail (when status == Failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
```

### New backend methods

```rust
// native-interfaces/src/backend.rs (extend ComponentBackend)

/// Upload a software package manifest (metadata + optional payload reference)
async fn upload_software_package(
    &self,
    component_id: &str,
    manifest: &SovdSoftwarePackageManifest,
) -> Result<SovdSoftwarePackage, DiagServiceError> {
    Err(DiagServiceError::RequestNotSupported("upload not supported".into()))
}

/// Activate an installed package (make it the running version)
async fn activate_software_package(
    &self,
    component_id: &str,
    package_id: &str,
) -> Result<SovdSoftwarePackage, DiagServiceError> {
    Err(DiagServiceError::RequestNotSupported("activate not supported".into()))
}

/// Rollback to previous version
async fn rollback_software_package(
    &self,
    component_id: &str,
    package_id: &str,
) -> Result<SovdSoftwarePackage, DiagServiceError> {
    Err(DiagServiceError::RequestNotSupported("rollback not supported".into()))
}

/// Get detailed status with progress
fn get_software_package_status(
    &self,
    component_id: &str,
    package_id: &str,
) -> Result<SovdSoftwarePackage, DiagServiceError> {
    Err(DiagServiceError::NotFound("package not found".into()))
}
```

### New/extended routes

| Method | Path | Description | Status |
|--------|------|-------------|--------|
| `GET` | `.../software-packages` | List packages | Exists, extend response |
| `POST` | `.../software-packages` | Upload manifest (new) | **New** |
| `POST` | `.../software-packages/{id}` | Install package | Exists |
| `GET` | `.../software-packages/{id}` | Get package details (new) | **New** |
| `GET` | `.../software-packages/{id}/status` | Get status + progress | Exists, extend |
| `POST` | `.../software-packages/{id}/activate` | Activate package | **New** |
| `POST` | `.../software-packages/{id}/rollback` | Rollback to previous | **New** |
| `DELETE` | `.../software-packages/{id}` | Remove package | **New** |

### Manifest type

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdSoftwarePackageManifest {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Download URL (for pull-based OTA)
    #[serde(skip_serializing_if = "Option::is_none", rename = "downloadUrl")]
    pub download_url: Option<String>,
    /// SHA-256 checksum of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    /// Package size in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}
```

### Progress tracking

Add an in-memory `DashMap<String, SovdSoftwarePackage>` to `AppState` (keyed by `{component_id}/{package_id}`) that backends update during async install/download. The `GET .../status` endpoint reads from this store for real-time progress.

### Integration points

| File | Change |
|------|--------|
| `native-interfaces/src/sovd.rs` | Extend `SovdSoftwarePackage`, add `SovdSoftwarePackageManifest`, extend `SovdSoftwarePackageStatus` |
| `native-interfaces/src/backend.rs` | Add `upload_software_package`, `activate_software_package`, `rollback_software_package`, `get_software_package_status` with default impls |
| `native-core/src/router.rs` | Forward new methods to backends |
| `native-sovd/src/state.rs` | Add `pub package_store: Arc<DashMap<String, SovdSoftwarePackage>>` |
| `native-sovd/src/routes.rs` | Add new routes, extend existing handlers |
| `native-sovd/src/openapi.rs` | Add new paths to CDF |
| `native-server/src/main.rs` | Initialize `package_store` |
| `examples/demo-ecu/` | Implement mock lifecycle (simulate download → install → activate) |

### Implementation steps

1. **Extend types** in `sovd.rs` (backward-compatible: new fields are `Option`)
2. **Add backend methods** with default impls
3. **Wire through `ComponentRouter`**
4. **Add `package_store`** to `AppState`
5. **Add new routes** and extend existing handlers
6. **Update OpenAPI/CDF**
7. **Implement mock lifecycle** in demo-ecu
8. **Tests:** ~15 new tests

### Estimated effort: M (2–3 days)

---

## 6. Cross-Cutting Concerns

### 6.1 Configuration changes

```toml
# config/opensovd-native-server.toml additions

[audit]
enabled = true
max_entries = 10000
file_path = "data/audit.jsonl"
# hmac_key = "hex-encoded-key"

[entities]
# Enable/disable entity types (overridden by OemProfile::DiscoveryPolicy)
apps_enabled = true
funcs_enabled = true
```

### 6.2 OpenAPI / CDF updates

`native-sovd/src/openapi.rs` must be extended with:

- `/apps` and nested resource paths
- `/funcs` and nested resource paths
- `/audit` path
- Extended `software-packages` paths (upload, activate, rollback)
- New schema definitions for `SovdApp`, `SovdFunc`, `SovdAuditEntry`, `SovdSoftwarePackageManifest`

### 6.3 OemProfile updates

Every OEM profile (`oem_sample.rs`, `oem_mbds.rs`) must implement:

- `AuthzPolicy` (new sub-trait)
- Updated `DefaultProfile` with permissive defaults

The existing 4 sub-traits remain unchanged.

### 6.4 Version bump

Bump all workspace crates to `0.7.0` after Wave 1 is complete.

---

## 7. Dependency & Ordering

Implementation order matters — later items depend on earlier ones.

```
W1.1 AuthZ ──────────┐
                      ├──→ W1.2 Audit Trail (needs caller identity from AuthZ)
W1.3 Apps/Funcs ──────┤
                      │
W1.4 Software Pkgs ───┘    (independent, but audit records all actions)
```

**Recommended sequence:**

| Step | Item | Rationale |
|------|------|-----------|
| 1 | W1.1 AuthZ | Foundation — all later audit records include authz context |
| 2 | W1.2 Audit Trail | Uses caller identity from W1.1 to record actions |
| 3 | W1.3 Apps/Funcs | Independent of audit, but audit will automatically cover new endpoints |
| 4 | W1.4 Software Pkgs | Independent, but benefits from audit + authz being in place |

Each step should be committed separately with passing tests.

---

## 8. Test Strategy

### Unit tests (per module)

| Module | Focus | Est. count |
|--------|-------|------------|
| `AuthzPolicy` / `AuthzContext` | Policy decisions for various role/resource combos | 15 |
| `AuditLog` | Record, query, capacity eviction, file sink, HMAC chain | 20 |
| `SovdApp` / `SovdFunc` types | Serialization roundtrips | 10 |
| `EntityBackend` default impls | Empty/not-found defaults | 5 |
| `SovdSoftwarePackage` extended | New fields serialize correctly | 8 |
| `SovdSoftwarePackageManifest` | Roundtrip, optional fields | 5 |

### Integration tests (route-level)

| Area | Focus | Est. count |
|------|-------|------------|
| AuthZ routes | Deny write for read-only role, allow admin | 10 |
| Audit endpoint | GET /audit, pagination, filtering | 8 |
| Apps routes | CRUD, nested data/operations, discovery toggle | 12 |
| Funcs routes | List, get, nested data | 8 |
| Software lifecycle | Upload → install → activate → rollback flow | 10 |

### Total estimated new tests: ~110

---

## 9. Files Changed Summary

| Crate | File | Type |
|-------|------|------|
| `native-interfaces` | `src/oem.rs` | Modify — add `AuthzPolicy`, `AuthzContext`, `AuthzDecision` |
| `native-interfaces` | `src/sovd.rs` | Modify — add `SovdApp`, `SovdFunc`, `SovdAuditEntry`, extend `SovdSoftwarePackage` |
| `native-interfaces` | `src/backend.rs` | Modify — add `EntityBackend`, extend `ComponentBackend` |
| `native-interfaces` | `src/lib.rs` | Modify — re-export new types |
| `native-core` | `src/audit_log.rs` | **New** — `AuditLog` with memory + file backends |
| `native-core` | `src/lib.rs` | Modify — add `pub mod audit_log` |
| `native-core` | `src/router.rs` | Modify — implement `EntityBackend`, forward new software-pkg methods |
| `native-sovd` | `src/auth.rs` | Modify — wire in `AuthzPolicy` after auth |
| `native-sovd` | `src/routes.rs` | Modify — add apps/funcs/audit routes, extend software-pkg routes, instrument audit |
| `native-sovd` | `src/state.rs` | Modify — add `audit_log`, `entity_backend`, `package_store` |
| `native-sovd` | `src/openapi.rs` | Modify — add new paths/schemas to CDF |
| `native-sovd` | `src/oem_sample.rs` | Modify — add `AuthzPolicy` impl |
| `native-server` | `src/main.rs` | Modify — construct and inject new state members |
| `examples/demo-ecu` | multiple | Modify — add example apps, funcs, software lifecycle |
| `config/` | `opensovd-native-server.toml` | Modify — add `[audit]` and `[entities]` sections |
| `Cargo.toml` (workspace) | — | Modify — version bump to 0.7.0 |

### New dependencies (estimated)

- `hmac` + `sha2` (optional, for audit HMAC chaining) — small, well-maintained
- No other new external dependencies expected

---

## Summary

| Feature | New types | New routes | New tests | Effort |
|---------|-----------|------------|-----------|--------|
| W1.1 AuthZ | 3 | 0 (middleware) | ~15 | M |
| W1.2 Audit | 2 + 1 module | 1 | ~20 | M |
| W1.3 Apps/Funcs | 3 + 1 trait | ~11 | ~25 | L |
| W1.4 Software Pkgs | 2 extended | ~4 new | ~15 | M |
| **Total** | **~10** | **~16** | **~75–110** | **~10–14 days** |

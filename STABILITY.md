# API Stability Policy

This document defines the stability guarantees for OpenSOVD-native-server
starting with version **1.0.0**.

---

## Versioning

This project follows [Semantic Versioning 2.0.0](https://semver.org/):

- **MAJOR** (x.0.0) — breaking changes to stable APIs
- **MINOR** (1.x.0) — new features, backward-compatible
- **PATCH** (1.0.x) — bug fixes, security patches, dependency updates

Pre-release versions (`-rc`, `-alpha`) carry no stability guarantees.

---

## Stable APIs (covered by semver)

The following interfaces are considered stable from 1.0.0 onward.
Breaking changes require a major version bump.

### HTTP API

| Surface | Guarantee |
|---------|-----------|
| All `/sovd/v1/*` endpoints | Path, method, request/response schema stable |
| OData query parameters | `$top`, `$skip`, `$filter`, `$orderby`, `$select` semantics stable |
| Error envelope format | `SovdErrorEnvelope` JSON shape stable |
| HTTP status code semantics | Status codes per endpoint stable |
| Security headers | Present on every response |

New endpoints may be added in minor releases. Existing endpoints will not
be removed or have their contracts changed without a major version bump.

### Rust Traits (public API for OEM integrators)

| Trait | Crate | Stability |
|-------|-------|-----------|
| `ComponentBackend` | `native-interfaces` | **Stable** |
| `EntityBackend` | `native-interfaces` | **Stable** |
| `ExtendedDiagBackend` | `native-interfaces` | **Stable** |
| `OemProfile` | `native-interfaces` | **Stable** |
| `AuthPolicy` | `native-interfaces` | **Stable** |
| `AuthzPolicy` | `native-interfaces` | **Stable** |
| `EntityIdPolicy` | `native-interfaces` | **Stable** |
| `DiscoveryPolicy` | `native-interfaces` | **Stable** |
| `CdfPolicy` | `native-interfaces` | **Stable** |
| `FaultSink` | `native-interfaces` | **Stable** |
| `FirmwareVerifier` | `native-interfaces` | **Stable** |
| `StorageBackend` | `native-interfaces` | **Stable** |
| `SecretProvider` | `native-interfaces` | **Stable** |
| `AuditSink` | `native-interfaces` | **Stable** |

New methods on stable traits will use default implementations to avoid
breaking existing OEM profiles.

### Configuration (TOML)

All documented configuration keys in `config/opensovd.toml` are stable.
New keys may be added with defaults that preserve existing behavior.
Existing keys will not be removed or change semantics without a major bump.

---

## Experimental APIs (not covered by semver)

The following are explicitly **unstable** and may change in any release:

| Surface | Reason |
|---------|--------|
| `persist` feature flag | Depends on `sled` (unmaintained upstream transitive deps) |
| `ws-bridge` feature flag | Bridge protocol subject to change |
| `/x-admin/*` endpoints | Internal admin API, not part of SOVD standard |
| `/x-bridge/*` endpoints | Cloud bridge session management, protocol evolving |
| `BridgeTransport` trait | Transport abstraction still maturing |
| `RbacPolicy` struct | RBAC model may be revised |

Experimental features are documented as such in `Cargo.toml` descriptions
and the README feature flag table.

---

## Deprecation Policy

Before removing a stable API in a future major version:

1. The feature is marked `#[deprecated]` for at least one minor release
2. A migration path is documented in the CHANGELOG
3. The deprecation warning includes the replacement API

---

## Minimum Supported Rust Version (MSRV)

The MSRV is **1.88.0** and is tested in CI. MSRV bumps are minor-version
changes (not patch).

---

## Platform Support

| Target | Tier | CI tested |
|--------|------|-----------|
| `x86_64-unknown-linux-gnu` | 1 | Yes |
| `aarch64-unknown-linux-gnu` | 2 | Cross-check only |
| `x86_64-apple-darwin` | 2 | Local dev |
| `aarch64-apple-darwin` | 2 | Local dev |

Tier 1: fully tested in CI. Tier 2: compiles, not integration-tested.

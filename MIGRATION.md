# Migration Guide: 0.x → 1.0

This document covers breaking changes and required actions when upgrading
from any 0.x release to 1.0.

---

## Breaking Changes

### 1. Entity ID Validation (B2)

**Affected:** All URL path parameters (`{component_id}`, `{fault_id}`, `{execution_id}`, etc.)

**Before (0.x):** The default `EntityIdPolicy` accepted any non-empty string, including
special characters, spaces, and path traversal sequences (`../`).

**After (1.0):** The default policy restricts entity IDs to the safe character set
`[a-zA-Z0-9_.-]` with a maximum length of 128 characters. Requests with IDs outside
this set receive `400 Bad Request`.

**Action required:**
- Verify that all entity IDs used by your backends and clients conform to `[a-zA-Z0-9_.-]{1,128}`.
- If your OEM profile overrides `EntityIdPolicy::validate_entity_id()`, review whether
  your custom rules are at least as strict as the new baseline.
- URL-encoded characters (`%2F`, `%20`, etc.) in path segments are **not** decoded before
  validation — they will be rejected.

### 2. Security Headers (B3)

**Affected:** All HTTP responses.

The following headers are now included on every response:

| Header | Value |
|--------|-------|
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `DENY` |
| `Cache-Control` | `no-store` |
| `Strict-Transport-Security` | `max-age=31536000; includeSubDomains` |

**Action required:**
- If your reverse proxy or load balancer sets these headers, remove duplicates to
  avoid double-header issues.
- `Cache-Control: no-store` means HTTP caches will not store responses. If you relied
  on caching SOVD responses at a proxy layer, configure the proxy to add its own
  cache headers downstream.
- `Strict-Transport-Security` has no effect over plain HTTP. It only activates when
  the client connects via HTTPS.

### 3. JWKS Cache Size Limit (B4)

**Affected:** OIDC authentication (`oidc_issuer_url` config).

JWKS responses from the identity provider are now limited to **64 keys**. If the
provider returns more than 64 keys, only the first 64 are cached (with a warning log).

**Action required:**
- No action for standard identity providers (Keycloak, Entra ID, Auth0 typically
  serve < 10 keys).
- If you operate a custom OIDC provider with key rotation that accumulates many old
  keys, ensure it serves ≤ 64 active keys.

### 4. Experimental Features

The following feature flags are now marked as **experimental** and excluded from
semver stability guarantees (see [STABILITY.md](STABILITY.md)):

| Feature | Reason |
|---------|--------|
| `persist` | Depends on `sled` which has unmaintained transitive dependencies |
| `ws-bridge` | Bridge protocol still evolving |

**Action required:**
- If you depend on `persist` or `ws-bridge` in production, pin your dependency to an
  exact version (`=1.0.0`) to avoid surprises from minor releases.

---

## Configuration Changes

No configuration keys were removed or renamed in 1.0. All existing `opensovd.toml`
files remain compatible without modification.

---

## Dependency Changes

| Dependency | 0.17.x | 1.0 | Reason |
|-----------|--------|-----|--------|
| `aws-lc-sys` | 0.38.0 | 0.39.0 | RUSTSEC-2026-0044, RUSTSEC-2026-0048 |
| `rustls-webpki` | 0.103.9 | 0.103.10 | RUSTSEC-2026-0049 |

---

## Trait Changes for OEM Integrators

### `EntityIdPolicy::validate_entity_id()` (default implementation changed)

The default implementation in `native-interfaces` now performs character-set and
length validation. OEM profiles that **do not** override this method automatically
inherit the stricter baseline.

OEM profiles that **do** override this method (e.g., MBDS with DDAG rules) are
unaffected — their custom implementation takes precedence.

```rust
// Old default (0.x): any non-empty string accepted
fn validate_entity_id(&self, id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Entity ID must not be empty".to_owned());
    }
    Ok(())
}

// New default (1.0): safe charset + length limit
fn validate_entity_id(&self, id: &str) -> Result<(), String> {
    if id.is_empty() { return Err(...); }
    if id.len() > 128 { return Err(...); }
    if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.') {
        return Err(...);
    }
    Ok(())
}
```

No other public traits have changed signatures. New methods added to existing traits
always include default implementations to preserve backward compatibility.

---

## Recommended Post-Upgrade Verification

```bash
# 1. Build and lint
cargo clippy --workspace -- -D warnings

# 2. Run full test suite
cargo test --workspace

# 3. Check for vulnerabilities
cargo audit

# 4. Verify your entity IDs
# Search your backend/client code for IDs that may contain special characters:
grep -rn 'component_id\|fault_id\|execution_id' your-client-code/
```

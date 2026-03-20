# HowTo: F15 — OIDC E2E Validation with Keycloak

**Status:** Out of Scope (requires external infrastructure)

---

## Goal

Validate the full OIDC authentication flow end-to-end against a real Identity Provider
(Keycloak), ensuring that the existing `oidc_issuer_url` configuration in `AuthConfig`
works correctly with RS256 JWKS discovery, token validation, issuer verification,
role/scope extraction, and tenant isolation.

## Prerequisites

| Requirement | Details |
|-------------|---------|
| **Keycloak instance** | v24+ (Quarkus-based), can run via Docker |
| **Realm** | `sovd` realm with OIDC client `opensovd-native` |
| **Roles** | `admin`, `operator`, `reader` (matching `RbacRole`) |
| **Test users** | At least 3 users with different roles |
| **Network** | Server must reach Keycloak's `/.well-known/openid-configuration` |

## Step-by-Step

### 1. Start Keycloak

```bash
docker run -d --name keycloak \
  -p 8180:8080 \
  -e KC_BOOTSTRAP_ADMIN_USERNAME=admin \
  -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin \
  quay.io/keycloak/keycloak:24.0 start-dev
```

### 2. Configure Keycloak Realm

```bash
# Create realm
curl -s -X POST http://localhost:8180/admin/realms \
  -H "Authorization: Bearer $(curl -s -X POST http://localhost:8180/realms/master/protocol/openid-connect/token \
    -d 'grant_type=password&client_id=admin-cli&username=admin&password=admin' | jq -r .access_token)" \
  -H "Content-Type: application/json" \
  -d '{
    "realm": "sovd",
    "enabled": true,
    "sslRequired": "none"
  }'
```

Create client `opensovd-native`:
- **Client Protocol:** openid-connect
- **Access Type:** confidential
- **Valid Redirect URIs:** `http://localhost:3000/*` (or `*` for testing)
- **Roles:** Create realm roles `admin`, `operator`, `reader`
- **Mappers:** Add a "realm roles" mapper to include roles in the `roles` claim
- **Custom claim:** Add `tenant_id` as a user attribute mapper (for multi-tenant tests)

### 3. Create Test Users

| Username | Password | Roles | tenant_id |
|----------|----------|-------|-----------|
| `admin-user` | `test` | `admin` | `tenant-a` |
| `operator-user` | `test` | `operator` | `tenant-a` |
| `reader-user` | `test` | `reader` | `tenant-b` |

### 4. Configure OpenSOVD Server

In `config/opensovd-native-server.toml`:

```toml
[auth]
enabled = true
oidc_issuer_url = "http://localhost:8180/realms/sovd"
# jwt_issuer is auto-discovered from OIDC, but can be overridden:
# jwt_issuer = "http://localhost:8180/realms/sovd"
```

### 5. Obtain Token and Test

```bash
# Get access token
TOKEN=$(curl -s -X POST \
  http://localhost:8180/realms/sovd/protocol/openid-connect/token \
  -d "grant_type=password&client_id=opensovd-native&client_secret=<SECRET>&username=admin-user&password=test" \
  | jq -r .access_token)

# Call protected endpoint
curl -s http://localhost:3000/sovd/v1/components \
  -H "Authorization: Bearer $TOKEN" | jq .
```

### 6. Validation Checklist

| Test | Expected |
|------|----------|
| Valid admin token → `GET /components` | 200 OK |
| Valid reader token → `PUT /modes/{id}` | 403 Forbidden (RBAC) |
| Expired token | 401 Unauthorized |
| Token from wrong issuer | 401 Unauthorized |
| No token on protected path | 401 Unauthorized |
| Public path without token | 200 OK |
| `tenant_id` claim → scoped storage | Tenant isolation works |
| JWKS rotation (restart Keycloak) | New keys fetched after cache TTL (5 min) |

## What Already Works

The OIDC implementation in `native-sovd/src/auth.rs` is **fully functional**:

- OIDC discovery (`/.well-known/openid-configuration`)
- JWKS fetch with 5-minute TTL cache
- RS256 token validation with `kid` matching
- Issuer verification (auto-discovered or overridden)
- Role/scope extraction into `Claims`
- Tenant context injection from `tenant_id` claim
- Audit logging for auth success/failure

**What's missing:** An automated integration test that starts Keycloak via
Testcontainers and runs the full flow. This requires the `testcontainers` crate
and Docker on the CI runner.

## Estimated Effort

**S (1–2 days)** — mostly Keycloak setup and test scripting, no code changes needed.

## Wave 3 — Cloud Bridge, Multi-Tenant, Variant-Aware, Zero-Trust

### Architecture Decisions
- **A3.1** Cloud bridge topology — feature-gated same-binary approach
- **A3.2** Multi-tenant data isolation — policy + namespace isolation
- **A3.3** `TenantContext` middleware — JWT `tenant_id` claim extraction, `TenantId` extractor in routes
- **A3.4** `BridgeTransport` trait — async cloud-to-vehicle communication abstraction
- **A3.5** API versioning contract — URL prefix versioning, deprecation policy

### Features
- **W3.1 Cloud bridge mode** — `InMemoryBridgeTransport`, REST API at `/sovd/v1/x-bridge/` (sessions, forward, heartbeat, disconnect, status)
- **W3.2 Multi-tenant fleet model** — `TenantId` extractor, tenant-scoped audit logging, `MultiTenantConfig` in server config
- **W3.3 Variant-aware discovery** — `SovdComponent` extended with `softwareVersion`, `hardwareVariant`, `installationVariant`. Query: `?variant=premium&softwareVersion=2.1.0`
- **W3.4 Zero-trust hardening** — Signed audit export (`GET /audit/export`) with hash chain integrity proof

### Enterprise Readiness
- **E3.1 Client SDK generation** — `scripts/generate-sdk.sh` (Python, TypeScript, Rust, Java, Go, C#)
- **E3.2 Compliance evidence export** — `GET /compliance-evidence` (ISO 17978-3, UNECE R155, ISO 27001)
- **E3.3 Canary/blue-green routing** — `X-Deployment-Target` / `X-Served-By` header-based routing middleware

### Quality
- **295 tests** (up from 269), all passing
- **Clippy clean**
- 7 crates bumped to 0.9.0

### New Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/sovd/v1/audit/export` | GET | Signed audit trail with hash chain integrity |
| `/sovd/v1/compliance-evidence` | GET | Regulatory compliance evidence package |
| `/sovd/v1/x-bridge/status` | GET | Bridge mode status |
| `/sovd/v1/x-bridge/sessions` | GET | List active bridge sessions |
| `/sovd/v1/x-bridge/sessions/:id` | GET/DELETE | Get or disconnect a session |
| `/sovd/v1/x-bridge/sessions/:id/forward` | POST | Forward SOVD request to vehicle |
| `/sovd/v1/x-bridge/sessions/:id/heartbeat` | POST | Session keepalive |

**Full Changelog**: https://github.com/rettde/OpenSOVD-native-server/compare/v0.8.1-beta...v0.9.0-beta

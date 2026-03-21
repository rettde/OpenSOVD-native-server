# Roadmap — OpenSOVD-native-server v0.19.0-rc

---

## Implemented Features

### Wave 1 — Security & Entity Model

| Area | What shipped | Tests |
|------|-------------|-------|
| Fine-grained AuthZ | Per-resource, per-entity authorization via `OemProfile` | ✔ |
| Audit trail | SHA-256 hash-chained `SovdAuditEntry`, `/audit` endpoint | 6 |
| Apps / Funcs entities | Full CRUD + nested resources (`EntityBackend` trait) | 14 |
| Software packages | Upload, activate, rollback lifecycle | 6 |
| Graceful shutdown | `SIGTERM` handler, connection draining, audit flush | ✔ |
| Health probes | `/healthz` (liveness), `/readyz` (readiness) | ✔ |
| Body size limit | 2 MiB default, per-endpoint timeout | ✔ |
| Config validation | Fail-fast at startup for invalid config | ✔ |
| Error catalog | Documented `SOVD-ERR-*` codes with HTTP status mapping | ✔ |
| RED metrics | Rate, error rate, duration histogram per route | ✔ |
| JSON logging | Structured JSON with W3C `traceparent` correlation | ✔ |

### Wave 2 — HPC Diagnostics & History

| Area | What shipped | Tests |
|------|-------------|-------|
| StorageBackend trait | Pluggable persistence; `InMemoryStorage` default | 11 |
| Secrets abstraction | `SecretProvider` trait; env, static, Vault implementations | 8 |
| Per-client rate limiting | Token-bucket keyed by JWT `sub` / API key | 5 |
| KPI / system-info | `/components/{id}/kpis`, `/system-info` | 1 |
| Historical storage | Time-range queries on faults + audit; background compaction | 13 |
| Fault debouncing | `FaultGovernor` wrapping `FaultBridge` | 8 |
| Mode/session model | UDS session semantics mapped to SOVD modes | ✔ |
| TLS hot-reload | Certificate file polling (30 s), graceful reload, mTLS | 7 |
| Backup/restore | Full state snapshot via admin API, tamper-evident | 2 |
| Feature flags | Lock-free atomic toggles, `/x-admin/features` REST API | 7 |
| OTLP export | OpenTelemetry span export (`otlp` feature) | ✔ |
| Deployment packaging | Dockerfile (distroless), systemd unit, Helm chart | ✔ |
| Load tests | k6 + Criterion benchmark harness | ✔ |
| Fault injection tests | Backend failure, disk full, corruption scenarios | 17 |

### Wave 3 — Enterprise & Fleet

| Area | What shipped | Tests |
|------|-------------|-------|
| Cloud bridge | `BridgeTransport` trait, in-memory + WebSocket transport | 8 |
| Multi-tenant isolation | JWT `tenant_id`, namespace-scoped storage | 15 |
| Variant-aware discovery | Filter by `installationVariant` / `softwareVersion` | ✔ |
| Signed audit export | `GET /audit/export` with hash-chain integrity proof | ✔ |
| Compliance evidence | `GET /compliance-evidence` (ISO 17978-3, UNECE R155) | ✔ |
| Canary routing | `X-Deployment-Target` header-based routing | ✔ |
| Client SDK generation | `scripts/generate-sdk.sh` (Python + TypeScript from OpenAPI) | ✔ |
| API versioning | `/sovd/v1/` stable; deprecation policy documented | ✔ |

### Wave 4 — Data Catalog & Batch Export

| Area | What shipped | Tests |
|------|-------------|-------|
| Semantic data catalog | COVESA VSS ontology, `DataCatalogProvider` trait | 6 |
| Batch snapshot export | `GET /components/{id}/snapshot` (NDJSON) | 3 |
| Fault export | `GET /export/faults` with severity + component filters | ✔ |
| Fault ontology | `affectedSubsystem`, `correlatedSignals`, `classificationTags` | 2 |
| Schema introspection | `GET /schema/data-catalog` | 1 |
| SSE data-change stream | Real-time data + fault change events | 2 |
| Data contract versioning | `schemaVersion` in all exports | ✔ |
| Reproducibility metadata | `_meta` preamble with provenance fields | ✔ |
| Schema stability tests | Regression tests for JSON shape changes | 4 |
| OpenAPI contract test | CI validates spec against CDF rules (Redocly + DSA) | 20 |

### Production Enhancements (Feature-Gated)

| Feature flag | What shipped | Tests |
|-------------|-------------|-------|
| `persist` | Sled embedded DB for faults + audit + history | 13 |
| `vault` | HashiCorp Vault KV v2 secret provider with cache + TTL | 10 |
| `ws-bridge` | WebSocket cloud↔vehicle tunnel (`tokio-tungstenite`) | 8 |
| `otlp` | OpenTelemetry OTLP trace export | ✔ |
| — | CycloneDX SBOM generation in CI | ✔ |
| — | Prometheus `/metrics` endpoint (config-gated) | ✔ |

### Compliance Fixes (C1–C3)

| Area | What shipped | Tests |
|------|-------------|-------|
| `/areas` entity type | `SovdArea`, `EntityBackend`, `DiscoveryPolicy` gating (MBDS §2.2 → 404) | 3 |
| Mode collection semantics | `PUT /modes/{modeId}` (ISO 17978-3 §5.5.4) | ✔ |
| DTC setting → modes | `dtc-on`/`dtc-off` mapped in `activate_mode` handler | ✔ |

### Enterprise Features (F10–F11)

| Area | What shipped | Tests |
|------|-------------|-------|
| RBAC (F10) | `RbacPolicy` with admin/operator/reader roles, `RbacConfig` for custom roles | 10 |
| Audit forwarding (F11) | `AuditSink` trait, `SyslogAuditSink` (RFC 5424 UDP), `CallbackAuditSink`, `AuditLog.add_sink()` | 2 |

### Persistent Storage (F9)

| Area | What shipped | Tests |
|------|-------------|-------|
| Snapshot/Rollback trait (F9) | `create_snapshot`, `list_snapshots`, `restore_snapshot`, `delete_snapshot` on `StorageBackend` — follows AUTOSAR `ara::per` / Eclipse S-CORE `KvsBackend` pattern | 13 |
| InMemoryStorage snapshots | Up to 16 in-memory snapshots with automatic eviction, `SnapshotInfo` metadata | 7 |
| SledStorage snapshots | Persistent snapshots via sled trees (`snap:<id>`), survives restart, up to 32 on disk | 6 |

| FirmwareVerifier (F12) | `FirmwareVerifier` trait with `Ed25519Verifier` + `NoopVerifier`, SHA-256 digest, signature gate in `activate_software_package` | 11 |
| mTLS backend (F14) | `client_cert_path` / `client_key_path` / `ca_cert_path` / `danger_accept_invalid_certs` on `SovdHttpBackendConfig` | 4 |

**Total: 433 tests · Clippy pedantic clean · ISO 17978-3 conformant (51/51)**

### v0.15.0 — Phase 3 Future Work (RXSWIN, TARA, UDS Security, UCM, Deployment)

| Area | What shipped | Tests |
|------|-------------|-------|
| RXSWIN Tracking (F15) | `RxswinEntry`, `RxswinReport`, `UpdateProvenanceEntry` — UNECE R156 RXSWIN per component, vehicle report, update provenance log | 5 |
| TARA (F16) | `TaraAsset`, `TaraThreatEntry`, `TaraExport`, `TaraThreatStatus` — ISO/SAE 21434 asset inventory + threat analysis + export document | 5 |
| UDS Security Access (F17) | `UdsSecurityLevel`, `UdsSecurityAccessRequest/Response` — ISO 14229 §9 seed/key protocol, 3-level security model (Workshop/Engineering/OEM) | 7 |
| UCM Campaigns (F18) | `UcmCampaign`, `UcmCampaignStatus` (9 states), `UcmTransferState`, `UcmTransferPhase` (9 phases) — AUTOSAR R24-11 campaign lifecycle with rollback | 8 |

| New endpoints | Path | Method |
|---------------|------|--------|
| RXSWIN list | `/sovd/v1/rxswin` | GET |
| RXSWIN report | `/sovd/v1/rxswin/report` | GET |
| RXSWIN by component | `/sovd/v1/rxswin/{component_id}` | GET |
| Update provenance | `/sovd/v1/update-provenance` | GET |
| TARA assets | `/sovd/v1/tara/assets` | GET |
| TARA threats | `/sovd/v1/tara/threats` | GET |
| TARA export | `/sovd/v1/tara/export` | GET |
| UCM campaigns | `/sovd/v1/ucm/campaigns` | GET, POST |
| UCM campaign detail | `/sovd/v1/ucm/campaigns/{id}` | GET |
| UCM execute | `/sovd/v1/ucm/campaigns/{id}/execute` | POST |
| UCM rollback | `/sovd/v1/ucm/campaigns/{id}/rollback` | POST |
| UDS security levels | `/sovd/v1/x-uds/components/{id}/security-levels` | GET |
| UDS security access | `/sovd/v1/x-uds/components/{id}/security-access` | POST |

**Total: 469 tests · Clippy pedantic clean · ISO 17978-3 conformant (51/51)**

### v0.5.0 — OpenSOVD Core Integration (RFC-0001)

| Phase | Scope | Effort |
|-------|-------|--------|
| 1. Structure alignment | Rename `native-*` → `opensovd-*`, SPDX headers, inline health crate | 1 day |
| 2. Core trait integration | `DataProvider` adapter, `Topology` migration, `FaultProvider`/`OperationProvider`/`ModeProvider`/`LockProvider` traits | 3 days |
| 3. ServerBuilder adoption | Replace manual axum router with `Server::builder()` pattern | 2 days |
| 4. Auth generalization | `OidcAuthenticator` + `RbacAuthorizer` implementing opensovd-core traits | 1 day |
| 5. Feature crate extraction | `opensovd-faults`, `opensovd-extensions`, `opensovd-someip` | 4 days |
| 6. Cleanup | Remove `openapi.rs`, `ComponentRouter`, adapt bridge to gateway binary | 1 day |

See [RFC-0001](rfc/RFC-0001-opensovd-core-integration.md) for full design and rationale.

---

## Planned (not yet implemented)

| ID | Area | Description | Priority | Effort |
|----|------|-------------|----------|--------|
| F5 | **E2E test suite** | **Out of Scope** — requires CDA binary + Docker infra · [HowTo](howto-f5-e2e-test-suite.md) | — | — |
| F8 | **SOME/IP real transport** | **Out of Scope** — requires Linux + vsomeip + SOME/IP peer · [HowTo](howto-f8-someip-real-transport.md) | — | — |
| F9 | ~~SQL storage backend~~ | **Implemented** as Snapshot/Rollback (AUTOSAR `ara::per` / S-CORE pattern) — see above | ✅ | — |
| F12 | ~~SW package signature verification~~ | **Implemented** — `FirmwareVerifier` trait (Ed25519 / Noop), signature gate in `activate_software_package`, `FirmwareConfig` in server TOML | ✅ | — |
| F13 | ~~Horizontal scaling~~ | **Removed** — no In-Vehicle use case; AUTOSAR HPC is always single-instance | — | — |
| F14 | ~~mTLS backend connections~~ | **Implemented** — `client_cert_path` / `client_key_path` / `ca_cert_path` on `SovdHttpBackendConfig`, reqwest identity + pinned CA | ✅ | — |
| F15 | ~~RXSWIN Tracking~~ | **Implemented** — `RxswinEntry`, `RxswinReport`, `UpdateProvenanceEntry`, 4 endpoints | ✅ | — |
| F16 | ~~TARA~~ | **Implemented** — `TaraAsset`, `TaraThreatEntry`, `TaraExport`, 3 endpoints | ✅ | — |
| F17 | ~~UDS Security Access~~ | **Implemented** — `UdsSecurityLevel`, seed/key protocol, 2 endpoints | ✅ | — |
| F18 | ~~UCM Campaigns~~ | **Implemented** — `UcmCampaign`, 9-state lifecycle, rollback orchestration, 5 endpoints | ✅ | — |
| F19 | **OIDC E2E validation** | **Out of Scope** — requires Keycloak instance · [HowTo](howto-f15-oidc-e2e-validation.md) | — | — |

**Effort key:** S = 1–2 days · M = 3–5 days · L = 1–2 weeks

---

## Architecture Decision Records

All ADRs are in [`docs/adr/`](adr/README.md).

| Wave | ADRs |
|------|------|
| 1 | Graceful shutdown, health probes, body size limit, config validation, AppState sub-groups, error catalog |
| 2 | StorageBackend trait, ComponentBackend diet, secrets abstraction, OTLP export, rate limiting |
| 3 | Cloud bridge topology, multi-tenant isolation, tenant context middleware, BridgeTransport trait, API versioning |
| 4 | Ontology reference (COVESA VSS), DataCatalogProvider trait, batch export format (NDJSON) |

# Roadmap — OpenSOVD-native-server v0.12.0

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

**Total: 398+ tests · Clippy pedantic clean · ISO 17978-3 conformant (51/51)**

---

## Planned (not yet implemented)

| ID | Area | Description | Priority |
|----|------|-------------|----------|
| F5 | **E2E test suite** | Testcontainers with CDA + demo-ecu for full gateway round-trip | Medium |
| F8 | **SOME/IP real transport** | Validate `native-comm-someip` FFI against real COVESA/vsomeip | Low |

---

## Architecture Decision Records

All ADRs are in [`docs/adr/`](adr/README.md).

| Wave | ADRs |
|------|------|
| 1 | Graceful shutdown, health probes, body size limit, config validation, AppState sub-groups, error catalog |
| 2 | StorageBackend trait, ComponentBackend diet, secrets abstraction, OTLP export, rate limiting |
| 3 | Cloud bridge topology, multi-tenant isolation, tenant context middleware, BridgeTransport trait, API versioning |
| 4 | Ontology reference (COVESA VSS), DataCatalogProvider trait, batch export format (NDJSON) |

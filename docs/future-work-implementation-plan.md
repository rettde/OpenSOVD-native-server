# Future Work

Remaining items not yet implemented. All are optional — the server is fully ISO 17978-3 conformant without them.

---

## F5 — E2E Test Suite

**Goal:** Full gateway round-trip tests with real backend processes.

**Approach:**
- Spawn `demo-ecu` + SOVD server as child processes (random ports)
- Scenarios: component discovery, fault lifecycle, auth enforcement, TLS handshake, bridge tunnel
- CI job with `--test-threads=1`, 30 s timeout per test

**Effort:** 1–2 weeks

---

## F8 — SOME/IP Real Transport

**Goal:** Validate `native-comm-someip` FFI against real COVESA/vsomeip.

**Approach:**
- Docker image with `libvsomeip3` built from COVESA/vsomeip 3.5.x
- `cargo build -p native-comm-someip --features vsomeip-ffi`
- Loopback integration test: service discovery → request/response → event subscription

**Effort:** 1–2 weeks (depends on vsomeip build environment)

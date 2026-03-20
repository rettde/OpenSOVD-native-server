# HowTo: F5 — E2E Test Suite with Testcontainers

**Status:** Out of Scope (requires CDA binary and Docker infrastructure)

---

## Goal

End-to-end integration tests that validate the full diagnostic chain:
**Tester → OpenSOVD Gateway → CDA → demo-ecu (simulated UDS)**

This verifies that the Gateway mode (`ComponentRouter`) correctly proxies
SOVD requests to a real CDA instance, which in turn communicates with ECUs
via UDS-over-DoIP.

## Prerequisites

| Requirement | Details |
|-------------|---------|
| **Docker** | Docker Engine 24+ or Podman, for Testcontainers |
| **CDA binary** | Eclipse OpenSOVD Classic Diagnostic Adapter, built from [eclipse-opensovd/classic-diagnostic-adapter](https://github.com/eclipse-opensovd/classic-diagnostic-adapter) |
| **MDD file** | Diagnostic description file for the demo ECU (from CDA toolchain) |
| **demo-ecu** | Our `examples/demo-ecu` binary (simulates a UDS ECU) |
| **Rust crate** | `testcontainers = "0.15"` + `testcontainers-modules` |

## Architecture

```
┌──────────────┐     HTTP/REST     ┌───────────────┐    HTTP     ┌──────────┐
│  Test Runner │ ───────────────→  │  OpenSOVD     │ ─────────→  │  CDA     │
│  (Rust)      │                   │  Gateway      │             │ (Docker) │
└──────────────┘                   │  (localhost)  │             └────┬─────┘
                                   └───────────────┘                 │ DoIP
                                                                     │
                                                                ┌────┴─────┐
                                                                │ demo-ecu │
                                                                │ (Docker) │
                                                                └──────────┘
```

## Step-by-Step

### 1. Build Docker Images

**demo-ecu:**
```dockerfile
# Dockerfile.demo-ecu
FROM rust:1.82-slim AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p demo-ecu

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /build/target/release/demo-ecu /demo-ecu
EXPOSE 13400
ENTRYPOINT ["/demo-ecu"]
```

**CDA:**
```bash
# Clone and build CDA
git clone https://github.com/eclipse-opensovd/classic-diagnostic-adapter.git
cd classic-diagnostic-adapter
cargo build --release
# Package as Docker image (see CDA README for details)
```

### 2. Create Testcontainers Setup

```rust
// integration-tests/src/e2e.rs (sketch — not production code)
use testcontainers::{clients::Cli, GenericImage};

async fn setup_e2e() -> (String, String) {
    let docker = Cli::default();

    // 1. Start demo-ecu (UDS simulator on DoIP port 13400)
    let ecu = docker.run(
        GenericImage::new("opensovd/demo-ecu", "latest")
            .with_exposed_port(13400)
    );
    let ecu_host = format!("{}:{}", ecu.get_host(), ecu.get_host_port(13400));

    // 2. Start CDA pointing to demo-ecu
    let cda = docker.run(
        GenericImage::new("opensovd/cda", "latest")
            .with_exposed_port(8080)
            .with_env_var("CDA_ECU_HOST", &ecu_host)
            .with_env_var("CDA_MDD_PATH", "/data/demo.mdd")
    );
    let cda_url = format!("http://{}:{}", cda.get_host(), cda.get_host_port(8080));

    // 3. Start OpenSOVD Gateway pointing to CDA
    // (use our binary or in-process via library)
    let gateway_url = start_gateway_with_backend(&cda_url).await;

    (gateway_url, cda_url)
}
```

### 3. Test Scenarios

| Test | Request | Expected |
|------|---------|----------|
| **Discovery** | `GET /sovd/v1/components` | Lists demo-ecu component |
| **Read data** | `GET /sovd/v1/components/{id}/data/{dataId}` | Returns UDS 0x22 response |
| **Read faults** | `GET /sovd/v1/components/{id}/faults` | Returns DTC list |
| **Clear faults** | `DELETE /sovd/v1/components/{id}/faults` | UDS 0x14 success |
| **Mode switch** | `PUT /sovd/v1/components/{id}/modes/{modeId}` | UDS session change |
| **Operation** | `POST /sovd/v1/components/{id}/operations/{opId}` | Routine control |
| **Health** | `GET /healthz` | 200 with CDA backend healthy |
| **Latency** | All above | < 500 ms round-trip |

### 4. CI Integration

```yaml
# .github/workflows/e2e.yml
name: E2E Tests
on: [push]
jobs:
  e2e:
    runs-on: ubuntu-latest
    services:
      docker:
        image: docker:dind
    steps:
      - uses: actions/checkout@v4
      - name: Build images
        run: |
          docker build -f Dockerfile.demo-ecu -t opensovd/demo-ecu .
          # CDA image from registry or build
      - name: Run E2E
        run: cargo test --test e2e -- --nocapture
```

## Blockers

| Blocker | Why |
|---------|-----|
| **CDA binary availability** | CDA must be built from source; no prebuilt Docker image published yet |
| **MDD file** | Requires ODX→MDD conversion toolchain (proprietary, OEM-specific) |
| **DoIP networking** | Docker networking for DoIP (TCP 13400) between containers |
| **CI Docker-in-Docker** | GitHub Actions needs Docker service for Testcontainers |

## What Already Works

- `examples/demo-ecu` — UDS ECU simulator (responds to ReadDataByIdentifier, etc.)
- `native-core/src/router.rs` — `ComponentRouter` proxies SOVD requests to backends
- `native-core/src/http_backend.rs` — HTTP backend connecting to CDA instances
- Unit/integration tests for all route handlers (190 tests in `native-sovd`)

**What's missing:** Docker images, MDD test fixture, Testcontainers wiring, CI pipeline.

## Estimated Effort

**L (1–2 weeks)** — significant infrastructure work (Docker images, MDD fixture, CI).

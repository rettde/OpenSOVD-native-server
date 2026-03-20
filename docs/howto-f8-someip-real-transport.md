# HowTo: F8 — SOME/IP Real Transport Validation

**Status:** Out of Scope (requires vsomeip library and SOME/IP-capable ECU or simulator)

---

## Goal

Validate the `native-comm-someip` FFI bindings against a real COVESA/vsomeip stack,
confirming that SOME/IP service discovery, request/response, and event subscription
work correctly in an actual Adaptive AUTOSAR environment.

## Prerequisites

| Requirement | Details |
|-------------|---------|
| **Linux host** | vsomeip is Linux-only (uses POSIX sockets, multicast) |
| **vsomeip3** | COVESA vsomeip v3.4+, built from [COVESA/vsomeip](https://github.com/COVESA/vsomeip) |
| **Boost** | Boost 1.74+ (vsomeip dependency) |
| **CMake** | For building vsomeip |
| **SOME/IP peer** | Either a vsomeip sample app, CDA with DoIP+SOME/IP, or a real ECU |
| **Multicast** | Host must support IP multicast (for SOME/IP-SD) |

## Architecture

```
┌──────────────────┐   SOME/IP (UDP/TCP)   ┌──────────────────┐
│  OpenSOVD Server │ ────────────────────→  │  vsomeip Service │
│  native-comm-    │                        │  (sample app or  │
│  someip (FFI)    │ ←──────────────────    │   CDA backend)   │
│                  │   SOME/IP-SD (mcast)   │                  │
└──────────────────┘                        └──────────────────┘
        │                                           │
        └───────────── 224.224.224.1:30490 ─────────┘
                       (SD multicast)
```

## Step-by-Step

### 1. Build vsomeip from Source

```bash
# Dependencies (Ubuntu/Debian)
sudo apt-get install -y libboost-all-dev cmake g++ git

# Clone and build
git clone https://github.com/COVESA/vsomeip.git
cd vsomeip
mkdir build && cd build
cmake .. -DCMAKE_INSTALL_PREFIX=/usr/local
make -j$(nproc)
sudo make install
sudo ldconfig
```

### 2. Enable the `vsomeip-ffi` Feature

```bash
# In the workspace root:
cargo build -p native-comm-someip --features vsomeip-ffi
```

This activates the FFI module in `native-comm-someip/src/ffi.rs` and
`native-comm-someip/src/runtime.rs` which link against `libvsomeip3`.

### 3. Configure vsomeip

Create `/etc/vsomeip/vsomeip-local.json`:

```json
{
    "unicast": "192.168.1.100",
    "netmask": "255.255.255.0",
    "logging": {
        "level": "info",
        "console": "true"
    },
    "applications": [
        {
            "name": "opensovd",
            "id": "0x1234"
        }
    ],
    "services": [
        {
            "service": "0x1000",
            "instance": "0x0001",
            "unreliable": "30509"
        }
    ],
    "service-discovery": {
        "enable": "true",
        "multicast": "224.224.224.1",
        "port": "30490",
        "protocol": "udp"
    }
}
```

Set the environment:
```bash
export VSOMEIP_CONFIGURATION=/etc/vsomeip/vsomeip-local.json
export VSOMEIP_APPLICATION_NAME=opensovd
```

### 4. Start a SOME/IP Service (Test Peer)

**Option A — vsomeip sample:**
```bash
# From vsomeip build directory:
./examples/response-sample
```

**Option B — CDA with SOME/IP backend:**
```bash
# Start CDA configured for SOME/IP transport
./opensovd-cda --config opensovd-cda.toml
```

**Option C — commonapi-someip sample:**
```bash
# Use CommonAPI/vsomeip HelloWorld service
# See: https://github.com/COVESA/capicxx-someip-tools
```

### 5. Test Scenarios

| Test | What to verify |
|------|---------------|
| **Service Discovery** | `SomeIpServiceProxy` discovers the remote service via multicast SD |
| **Request/Response** | Send a SOME/IP request (method call), receive response |
| **Event subscription** | Subscribe to a SOME/IP eventgroup, receive notifications |
| **Serialization** | Payload correctly maps to UDS service bytes (ReadDataByIdentifier, etc.) |
| **Reconnection** | Kill peer, restart → service re-discovered automatically |
| **Timeout** | Request to unavailable service returns error within configured timeout |

### 6. Validation Commands

```bash
# Check vsomeip is linked correctly
ldd target/debug/libsomeip_comm.so | grep vsomeip

# Run with verbose vsomeip logging
VSOMEIP_CONFIGURATION=/etc/vsomeip/vsomeip-local.json \
VSOMEIP_APPLICATION_NAME=opensovd \
RUST_LOG=debug \
cargo run -- --config config/opensovd-native-server.toml

# Watch SOME/IP-SD multicast traffic
sudo tcpdump -i eth0 -n udp port 30490
```

## Blockers

| Blocker | Why |
|---------|-----|
| **Linux only** | vsomeip does not build on macOS or Windows |
| **C++ library** | Requires vsomeip3 + Boost installed on build host |
| **Multicast network** | CI runners often don't support IP multicast |
| **Real ECU or simulator** | Need a SOME/IP peer to test against |
| **FFI maintenance** | vsomeip API changes require FFI binding updates |

## What Already Works

- `native-comm-someip/src/config.rs` — `SomeIpConfig` for service/instance/method IDs
- `native-comm-someip/src/service.rs` — `SomeIpRuntime` + `SomeIpServiceProxy` (stub mode)
- `native-comm-someip/src/ffi.rs` — C FFI declarations for vsomeip3 (behind `vsomeip-ffi` feature)
- `native-comm-someip/src/runtime.rs` — Real vsomeip runtime (behind `vsomeip-ffi` feature)
- Stub mode compiles and passes tests on all platforms (7 tests)

**What's missing:** End-to-end validation against a real vsomeip peer on a Linux host.

## Estimated Effort

**L (1–2 weeks)** — vsomeip build pipeline, Linux CI runner, test peer setup.

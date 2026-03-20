# HowTo: Deploy on NVIDIA DRIVE AGX (Orin / Thor)

> Target: `aarch64-unknown-linux-gnu` or `aarch64-unknown-linux-musl`
> Platform: DRIVE OS 6.x (Linux), L4T (Linux for Tegra)

---

## Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Rust toolchain | ≥ 1.88 | MSRV of this project |
| `aarch64-unknown-linux-gnu` target | via rustup | Cross-compilation target |
| `aarch64-linux-gnu-gcc` | ≥ 11 | Cross-linker (from Ubuntu `gcc-aarch64-linux-gnu`) |
| NVIDIA DRIVE SDK | 6.0.8+ | Sysroot, libraries, deployment tools |
| Docker (optional) | ≥ 24 | Container-based deployment on DRIVE AGX |

---

## 1. Install Cross-Compilation Toolchain

```bash
# Add Rust target
rustup target add aarch64-unknown-linux-gnu

# Install cross-linker (Ubuntu/Debian)
sudo apt install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu

# macOS (via Homebrew)
brew install aarch64-unknown-linux-gnu
```

## 2. Configure `.cargo/config.toml`

The project ships with pre-configured target sections. Uncomment the linker:

```toml
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
rustflags = ["-C", "target-cpu=cortex-a78ae"]   # Orin SoC cores
```

The `target-cpu=cortex-a78ae` matches the Arm Cortex-A78AE cores in DRIVE Orin.
For DRIVE Thor (next-gen), use `cortex-a720` when Rust LLVM supports it.

## 3. Build

```bash
# Debug build (fast iteration)
cargo build --target aarch64-unknown-linux-gnu -p opensovd-native-server

# Release build (production — LTO + strip + panic=abort)
cargo build --release --target aarch64-unknown-linux-gnu -p opensovd-native-server

# Verify binary
file target/aarch64-unknown-linux-gnu/release/opensovd-native-server
# → ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked ...
```

### Static linking (musl — no glibc dependency)

```bash
rustup target add aarch64-unknown-linux-musl

# Uncomment linker in .cargo/config.toml:
# [target.aarch64-unknown-linux-musl]
# linker = "aarch64-linux-musl-gcc"

cargo build --release --target aarch64-unknown-linux-musl -p opensovd-native-server

# Result: fully static binary, no runtime dependencies
file target/aarch64-unknown-linux-musl/release/opensovd-native-server
# → ELF 64-bit LSB executable, ARM aarch64, statically linked ...
```

## 4. Deploy to DRIVE AGX

### Option A: Direct copy (development)

```bash
DRIVE_IP=192.168.1.100
DRIVE_USER=nvidia

scp target/aarch64-unknown-linux-gnu/release/opensovd-native-server \
    config/opensovd-native-server.toml \
    ${DRIVE_USER}@${DRIVE_IP}:/opt/opensovd/

ssh ${DRIVE_USER}@${DRIVE_IP} \
    "/opt/opensovd/opensovd-native-server --config /opt/opensovd/opensovd-native-server.toml"
```

### Option B: Docker container (production)

```dockerfile
# Dockerfile.drive-agx
FROM nvcr.io/drive/driveos-sdk/linux-aarch64:6.0.8 AS runtime
# Or for minimal image:
# FROM arm64v8/alpine:3.20 AS runtime   (with musl binary)

COPY target/aarch64-unknown-linux-gnu/release/opensovd-native-server /usr/local/bin/
COPY config/opensovd-native-server.toml /etc/opensovd/config.toml

EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s \
    CMD curl -sf http://localhost:8080/sovd/v1/health || exit 1

ENTRYPOINT ["opensovd-native-server", "--config", "/etc/opensovd/config.toml"]
```

```bash
# Build container (on dev host)
docker buildx build --platform linux/arm64 -t opensovd-native:drive -f Dockerfile.drive-agx .

# Deploy via NVIDIA Container Runtime on DRIVE AGX
docker save opensovd-native:drive | ssh ${DRIVE_USER}@${DRIVE_IP} docker load
ssh ${DRIVE_USER}@${DRIVE_IP} docker run -d --name sovd -p 8080:8080 opensovd-native:drive
```

### Option C: systemd service

```ini
# /etc/systemd/system/opensovd-native.service
[Unit]
Description=OpenSOVD Native Diagnostic Server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/opensovd/opensovd-native-server --config /opt/opensovd/config.toml
Restart=on-failure
RestartSec=5
# Resource limits for ADAS HPC
LimitNOFILE=65536
MemoryMax=256M
CPUQuota=50%
# Watchdog (process must stay alive or systemd restarts it)
WatchdogSec=30
# Graceful shutdown (SIGTERM → 10s grace → SIGKILL)
TimeoutStopSec=15

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now opensovd-native
sudo systemctl status opensovd-native
journalctl -u opensovd-native -f
```

## 5. Configuration for In-Vehicle Use

Minimal `opensovd-native-server.toml` for DRIVE AGX:

```toml
[server]
bind_address = "0.0.0.0:8080"

[auth]
mode = "jwt"
jwt_secret = "CHANGE_ME_IN_PRODUCTION"
# For production: use HashiCorp Vault or HSM-backed secret
# jwt_secret_provider = "vault"

[rate_limit]
enabled = true
max_requests = 100
window_secs = 10

[firmware]
verify = true
# Ed25519 public key for OTA package signature verification
# public_key_hex = "..."

# Backend: connect to ECU CDAs via SOVD REST
[[backends]]
name = "brake-ecu"
base_url = "http://10.0.0.10:8081"
component_ids = ["brake-ecu-front-left", "brake-ecu-front-right"]

[[backends]]
name = "camera-ecu"
base_url = "http://10.0.0.11:8081"
component_ids = ["front-camera", "rear-camera"]
```

## 6. Verification

```bash
# From dev machine or on-board
curl http://${DRIVE_IP}:8080/sovd/v1/health
# → {"status":"healthy","uptime_secs":42,...}

curl http://${DRIVE_IP}:8080/sovd/v1/components
# → {"@odata.count":4,"value":[...]}

# RXSWIN report (UNECE R156 compliance)
curl http://${DRIVE_IP}:8080/sovd/v1/rxswin/report
```

## 7. Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `Illegal instruction` on start | Built with wrong `target-cpu` | Verify `.cargo/config.toml` uses `cortex-a78ae`, not `native` |
| `GLIBC_2.34 not found` | Host glibc newer than target | Use `musl` target for static binary |
| Connection refused on port 8080 | Firewall / network namespace | Check `iptables` / container networking |
| `Failed to bind` | Port already in use | Check `ss -tlnp | grep 8080` |
| High latency on first request | TLS handshake + JIT warm-up | Use HTTP (not HTTPS) for in-vehicle; TLS only for external |

---

## Platform Reference

| SoC | Cores | `target-cpu` | DRIVE OS |
|-----|-------|-------------|----------|
| Orin (2022) | 12× Cortex-A78AE | `cortex-a78ae` | 6.0.x |
| Thor (2025) | Cortex-A720 + Cortex-A520 | `cortex-a720` (when LLVM supports) | 7.0.x |
| Xavier (legacy) | 8× Carmel (ARMv8.2) | `cortex-a75` | 5.2.x |

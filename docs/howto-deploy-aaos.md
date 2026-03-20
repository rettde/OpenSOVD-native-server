# HowTo: Deploy on Android Automotive OS (AAOS IVI)

> Target: `aarch64-linux-android`
> Platform: AAOS 14/15 (API 34/35), Qualcomm SA8295P / SA8775P, MediaTek MT8678

---

## Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Rust toolchain | ≥ 1.88 | MSRV of this project |
| `aarch64-linux-android` target | via rustup | Cross-compilation target |
| Android NDK | r27+ | Clang cross-compiler + sysroot |
| `cargo-ndk` (optional) | ≥ 3.5 | Simplifies NDK-based builds |
| ADB | Platform-tools 35+ | Deploy & debug on target |

---

## 1. Install Cross-Compilation Toolchain

```bash
# Add Rust target
rustup target add aarch64-linux-android

# Install Android NDK (standalone or via Android Studio)
# Option A: Android Studio → SDK Manager → NDK (Side by side) → r27
# Option B: Command-line
sdkmanager "ndk;27.2.12479018"

# Set ANDROID_NDK_HOME
export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/27.2.12479018
# or on macOS:
# export ANDROID_NDK_HOME=$HOME/Library/Android/sdk/ndk/27.2.12479018

# Install cargo-ndk (optional but recommended)
cargo install cargo-ndk
```

## 2. Configure `.cargo/config.toml`

The project ships with a pre-configured target section. Uncomment the linker
and adjust the NDK API level to match your AAOS version:

```toml
[target.aarch64-linux-android]
linker = "aarch64-linux-android35-clang"   # NDK toolchain (API 35 = Android 15)
rustflags = ["-C", "target-cpu=cortex-a76"]
```

Set the linker search path:

```bash
export PATH="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH"
# macOS:
# export PATH="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin:$PATH"
```

### API Level Mapping

| AAOS Version | Android | API Level | NDK Linker |
|-------------|---------|-----------|------------|
| AAOS 13 | 13 (Tiramisu) | 33 | `aarch64-linux-android33-clang` |
| AAOS 14 | 14 (Upside Down Cake) | 34 | `aarch64-linux-android34-clang` |
| AAOS 15 | 15 (Vanilla Ice Cream) | 35 | `aarch64-linux-android35-clang` |

## 3. Build

### Option A: Direct cargo build

```bash
# Ensure NDK toolchain is on PATH
export PATH="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH"

# Debug build
cargo build --target aarch64-linux-android -p opensovd-native-server

# Release build (LTO + strip)
cargo build --release --target aarch64-linux-android -p opensovd-native-server

# Verify
file target/aarch64-linux-android/release/opensovd-native-server
# → ELF 64-bit LSB pie executable, ARM aarch64, dynamically linked (uses shared libs) ...
```

### Option B: cargo-ndk (handles toolchain paths automatically)

```bash
# Build for API 35 (Android 15)
cargo ndk -t arm64-v8a -p 35 build --release -p opensovd-native-server

# Output at: target/aarch64-linux-android/release/opensovd-native-server
```

### Known Build Issues

| Issue | Cause | Fix |
|-------|-------|-----|
| `cannot find -llog` | Missing Android sysroot | Ensure `ANDROID_NDK_HOME` is set correctly |
| `undefined reference to __android_log_print` | Linking against wrong API level | Match API level to target AAOS version |
| Ring / OpenSSL build failure | C dependencies need NDK env | Set `CC_aarch64_linux_android` and `AR_aarch64_linux_android` |

For crates with C dependencies (e.g., `ring` for TLS):

```bash
export CC_aarch64_linux_android="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android35-clang"
export AR_aarch64_linux_android="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar"
```

## 4. Deploy to AAOS Device

### Option A: ADB push (development / testing)

```bash
# Push binary and config
adb root
adb push target/aarch64-linux-android/release/opensovd-native-server /data/local/tmp/
adb push config/opensovd-native-server.toml /data/local/tmp/

# Run
adb shell /data/local/tmp/opensovd-native-server \
    --config /data/local/tmp/opensovd-native-server.toml

# Forward port for local testing
adb forward tcp:8080 tcp:8080
curl http://localhost:8080/sovd/v1/health
```

### Option B: Android system service (production)

For production AAOS deployment, the binary runs as a native system service
managed by Android's `init` system (not systemd).

**1. Place binary in system image:**

```
# In your AAOS BSP / device tree
PRODUCT_COPY_FILES += \
    vendor/opensovd/opensovd-native-server:$(TARGET_COPY_OUT_VENDOR)/bin/opensovd-native-server \
    vendor/opensovd/opensovd-native-server.toml:$(TARGET_COPY_OUT_VENDOR)/etc/opensovd/config.toml
```

**2. Init `.rc` service definition:**

```rc
# vendor/opensovd/opensovd-native.rc
service opensovd /vendor/bin/opensovd-native-server --config /vendor/etc/opensovd/config.toml
    class hal
    user system
    group system inet
    capabilities NET_BIND_SERVICE
    # Restart on crash with 5s backoff
    oneshot
    disabled

on property:sys.boot_completed=1
    start opensovd
```

**3. SELinux policy (required for AAOS):**

```
# vendor/opensovd/sepolicy/opensovd.te
type opensovd_exec, exec_type, vendor_file_type, file_type;
type opensovd, domain;

init_daemon_domain(opensovd)
net_domain(opensovd)

# Allow binding to TCP port
allow opensovd self:tcp_socket { create_socket_perms };
allow opensovd port:tcp_socket { name_bind };

# Allow reading config
allow opensovd vendor_configs_file:file r_file_perms;
allow opensovd vendor_configs_file:dir search;
```

### Option C: AAOS Vendor APEX (Android 14+)

For modular updates without full OTA, package as a Vendor APEX:

```
// vendor/opensovd/Android.bp
cc_binary {
    name: "opensovd-native-server",
    vendor: true,
    srcs: [],  // prebuilt
    arch: {
        arm64: {
            srcs: ["prebuilt/arm64/opensovd-native-server"],
        },
    },
    apex_available: ["com.vendor.opensovd"],
}

apex {
    name: "com.vendor.opensovd",
    vendor: true,
    binaries: ["opensovd-native-server"],
    prebuilts: ["opensovd-config"],
}
```

## 5. Configuration for AAOS IVI

Minimal `opensovd-native-server.toml`:

```toml
[server]
bind_address = "0.0.0.0:8080"

[auth]
mode = "jwt"
jwt_secret = "CHANGE_ME"

[rate_limit]
enabled = true
max_requests = 50
window_secs = 10

[firmware]
verify = true

# IVI-local ECUs (body, infotainment, climate)
[[backends]]
name = "body-control"
base_url = "http://10.0.1.10:8081"
component_ids = ["bcm", "window-ctrl", "door-lock"]

[[backends]]
name = "climate"
base_url = "http://10.0.1.11:8081"
component_ids = ["hvac-front", "hvac-rear"]
```

## 6. Differences from Linux HPC Deployment

| Aspect | NVIDIA DRIVE (Linux) | AAOS IVI (Android) |
|--------|---------------------|-------------------|
| Init system | systemd | Android init (`*.rc`) |
| Service supervision | `systemctl` | `start`/`stop` properties |
| Logging | journald / stdout | `logcat` (Android logging) |
| Security | Linux DAC/MAC | SELinux (mandatory) |
| Networking | Standard sockets | Android `NetPolicy` constraints |
| Updates | Docker / apt / OTA | Vendor APEX / full OTA |
| File paths | `/opt/opensovd/` | `/vendor/bin/`, `/vendor/etc/` |
| User | `root` or service user | `system` user (UID 1000) |

## 7. Logging on AAOS

By default, the server logs to stdout/stderr via `tracing`. On Android,
redirect to `logcat`:

```toml
[logging]
format = "json"  # structured logs for logcat parsing
```

```bash
# View logs
adb logcat -s opensovd

# Or filter by PID
adb shell pidof opensovd-native-server
adb logcat --pid=<PID>
```

> **Note:** For production AAOS, consider integrating `android_logger` crate
> to route `tracing` output directly to Android's logging subsystem.
> This is not yet implemented — see roadmap item F20.

## 8. Verification

```bash
# From host via ADB port forward
adb forward tcp:8080 tcp:8080

curl http://localhost:8080/sovd/v1/health
# → {"status":"healthy","uptime_secs":12,...}

curl http://localhost:8080/sovd/v1/components
# → {"@odata.count":5,"value":[...]}

# TARA export (ISO/SAE 21434 compliance)
curl http://localhost:8080/sovd/v1/tara/export
```

---

## Platform Reference

| SoC | Vendor | Cores | `target-cpu` | Typical Use |
|-----|--------|-------|-------------|-------------|
| SA8295P | Qualcomm | 8× Kryo 780 (Cortex-A710/A510) | `cortex-a76` | Premium IVI |
| SA8775P | Qualcomm | 8× Kryo (Cortex-A720/A520) | `cortex-a76` | Next-gen cockpit |
| MT8678 | MediaTek | 8× Cortex-A73/A53 | `cortex-a73` | Mid-range IVI |
| Exynos Auto V920 | Samsung | Cortex-A78AE | `cortex-a78ae` | Digital cockpit |

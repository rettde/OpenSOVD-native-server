# Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
# SPDX-License-Identifier: Apache-2.0
#
# Multi-stage build for OpenSOVD-native-server
# Produces a minimal distroless image (~30 MB) suitable for production.
#
# Usage:
#   docker build -t opensovd-native:latest -f deploy/Dockerfile .
#   docker run -p 8080:8080 opensovd-native:latest
#
# With custom config:
#   docker run -v /path/to/config:/config -p 8080:8080 opensovd-native:latest

# ── Stage 1: Build ──────────────────────────────────────────────────────────
FROM rust:1.85-bookworm AS builder

WORKDIR /build
COPY . .

# Create OEM stub for open-source builds (proprietary profiles are .gitignored)
RUN if [ ! -f native-sovd/src/oem_mbds.rs ]; then \
      echo '// Docker build stub — proprietary OEM profile not present' \
        > native-sovd/src/oem_mbds.rs; \
    fi

# Ensure a config file exists (empty defaults are fine — all values have serde defaults)
RUN test -f opensovd-native-server.toml || echo '# Default config — override via env or volume mount' > opensovd-native-server.toml

RUN cargo build --release --workspace \
    && strip target/release/opensovd-native-server

# ── Stage 2: Runtime (distroless) ───────────────────────────────────────────
FROM gcr.io/distroless/cc-debian12:nonroot

LABEL org.opencontainers.image.title="OpenSOVD Native Server" \
      org.opencontainers.image.description="ISO 17978-3 SOVD diagnostic server" \
      org.opencontainers.image.source="https://github.com/rettde/OpenSOVD-native-server" \
      org.opencontainers.image.licenses="Apache-2.0"

COPY --from=builder /build/target/release/opensovd-native-server /opensovd-native-server
COPY --from=builder /build/opensovd-native-server.toml /config/opensovd-native-server.toml

# Default SOVD API port (plain) + TLS port
EXPOSE 8080 8443

# Health check via /healthz (liveness probe)
# Note: distroless has no shell — use HEALTHCHECK with the binary or
# rely on Kubernetes liveness/readiness probes instead.

ENV SOVD_SERVER__HOST=0.0.0.0 \
    SOVD_SERVER__PORT=8080 \
    SOVD_LOGGING__LEVEL=info \
    SOVD_LOGGING__FORMAT=json

ENTRYPOINT ["/opensovd-native-server"]

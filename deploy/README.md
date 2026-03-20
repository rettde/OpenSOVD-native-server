# Deployment Guide — OpenSOVD Native Server

Production-ready deployment artifacts for the OpenSOVD-native-server.

---

## Docker

### Build

```bash
docker build -t opensovd-native:latest -f deploy/Dockerfile .
```

### Run

```bash
# Plain HTTP (development)
docker run -p 8080:8080 opensovd-native:latest

# With custom config
docker run \
  -v /path/to/config:/config \
  -p 8080:8080 \
  opensovd-native:latest

# With TLS
docker run \
  -v /path/to/certs:/tls:ro \
  -e SOVD_SERVER__CERT_PATH=/tls/server.crt \
  -e SOVD_SERVER__KEY_PATH=/tls/server.key \
  -p 8443:8443 \
  opensovd-native:latest

# With authentication
docker run \
  -e SOVD_AUTH__ENABLED=true \
  -e SOVD_AUTH__API_KEY=your-secret-key \
  -p 8080:8080 \
  opensovd-native:latest
```

### Image details

- **Base:** `gcr.io/distroless/cc-debian12:nonroot` (~30 MB)
- **User:** nonroot (uid 65534)
- **Ports:** 8080 (HTTP), 8443 (HTTPS)
- **Config:** `/config/opensovd-native-server.toml`
- **Logging:** JSON to stdout (12-factor)

---

## systemd

### Install

```bash
# Copy binary
sudo cp target/release/opensovd-native-server /usr/local/bin/

# Create user
sudo useradd -r -s /usr/sbin/nologin opensovd
sudo mkdir -p /etc/opensovd /var/lib/opensovd
sudo cp opensovd-native-server.toml /etc/opensovd/
sudo chown -R opensovd:opensovd /etc/opensovd /var/lib/opensovd

# Install service
sudo cp deploy/opensovd-native-server.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now opensovd-native-server
```

### Manage

```bash
sudo systemctl status opensovd-native-server
sudo systemctl restart opensovd-native-server
journalctl -u opensovd-native-server -f
```

### Security hardening

The systemd unit includes:
- `ProtectSystem=strict` — read-only filesystem (except `/var/lib/opensovd`)
- `NoNewPrivileges=true` — no privilege escalation
- `MemoryDenyWriteExecute=true` — W^X enforcement
- `SystemCallFilter=@system-service` — restricted syscalls
- `PrivateDevices=true` — no device access
- Runs as dedicated `opensovd` user

---

## Kubernetes (Helm)

### Install

```bash
helm install sovd deploy/helm/opensovd \
  --namespace sovd-system \
  --create-namespace
```

### Configure backends

```bash
helm install sovd deploy/helm/opensovd \
  --set backends[0].name=cda-primary \
  --set backends[0].baseUrl=http://cda-service:8080 \
  --set "backends[0].componentIds={ecu-hpc,ecu-bms}"
```

### Enable TLS + auth

```bash
# Create TLS secret
kubectl create secret tls sovd-tls \
  --cert=server.crt --key=server.key \
  -n sovd-system

# Create auth secret
kubectl create secret generic sovd-auth \
  --from-literal=api-key=your-secret-key \
  -n sovd-system

helm upgrade sovd deploy/helm/opensovd \
  --set tls.enabled=true \
  --set tls.secretName=sovd-tls \
  --set auth.enabled=true \
  --set auth.apiKeySecretName=sovd-auth
```

### Production values example

```yaml
# values-production.yaml
replicaCount: 3

autoscaling:
  enabled: true
  minReplicas: 3
  maxReplicas: 10
  targetCPUUtilizationPercentage: 70

auth:
  enabled: true
  apiKeySecretName: sovd-auth

tls:
  enabled: true
  secretName: sovd-tls

logging:
  level: "info"
  format: "json"
  otlpEndpoint: "http://otel-collector:4317"

rateLimit:
  enabled: true
  maxRequests: 200
  windowSecs: 60

resources:
  requests:
    cpu: 250m
    memory: 256Mi
  limits:
    cpu: "1"
    memory: 512Mi
```

```bash
helm install sovd deploy/helm/opensovd -f values-production.yaml
```

### Health probes

| Probe | Endpoint | Purpose |
|-------|----------|---------|
| Liveness | `GET /healthz` | Restart if unresponsive |
| Readiness | `GET /readyz` | Remove from LB until ready |
| Startup | — | Not configured (fast startup) |

---

## Environment Variables

All config values can be overridden via environment variables with the `SOVD_` prefix
and `__` as separator for nested keys:

| Variable | Default | Description |
|----------|---------|-------------|
| `SOVD_SERVER__HOST` | `0.0.0.0` | Bind address |
| `SOVD_SERVER__PORT` | `8080` | HTTP port |
| `SOVD_SERVER__CERT_PATH` | — | TLS certificate path |
| `SOVD_SERVER__KEY_PATH` | — | TLS private key path |
| `SOVD_SERVER__CLIENT_CA_PATH` | — | mTLS client CA path |
| `SOVD_AUTH__ENABLED` | `false` | Enable authentication |
| `SOVD_AUTH__API_KEY` | — | Static API key |
| `SOVD_AUTH__JWT_SECRET` | — | JWT signing secret |
| `SOVD_AUTH__OIDC_ISSUER_URL` | — | OIDC discovery URL |
| `SOVD_LOGGING__LEVEL` | `info` | Log level |
| `SOVD_LOGGING__FORMAT` | `text` | `text` or `json` |
| `SOVD_LOGGING__OTLP_ENDPOINT` | — | OTLP collector URL |
| `SOVD_RATE_LIMIT__ENABLED` | `false` | Enable rate limiting |
| `SOVD_RATE_LIMIT__MAX_REQUESTS` | `100` | Requests per window |
| `SOVD_RATE_LIMIT__WINDOW_SECS` | `60` | Rate limit window |
| `SOVD_DEPLOYMENT_LABEL` | — | Canary routing label |
| `SOVD_METRICS__ENABLED` | `true` | Enable `/metrics` Prometheus endpoint |
| `SOVD_BRIDGE__ENABLED` | `false` | Enable cloud bridge mode |
| `SOVD_BRIDGE__LISTEN_ADDR` | — | WebSocket listener address (requires `ws-bridge`) |
| `SOVD_STORAGE__BACKEND` | `memory` | `memory` or `sled` (requires `persist`) |
| `SOVD_STORAGE__SLED_PATH` | `./data/sovd.sled` | Sled database path |
| `SOVD_SECRETS__PROVIDER` | `env` | `env`, `vault`, or `static` (Vault requires `vault`) |
| `SOVD_SECRETS__VAULT_ADDR` | — | Vault server address |
| `VAULT_TOKEN` | — | Vault auth token (env fallback) |

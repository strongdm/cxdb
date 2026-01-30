# Deployment Guide

This guide covers deploying CXDB in production environments.

## Quick Start: Docker

The simplest production deployment uses the pre-built Docker image:

```bash
docker run -d \
  --name cxdb \
  --restart unless-stopped \
  -p 9009:9009 \
  -p 9010:9010 \
  -v /var/lib/cxdb:/data \
  -e CXDB_DATA_DIR=/data \
  -e CXDB_LOG_LEVEL=info \
  cxdb/cxdb:latest
```

## Architecture

Production deployments typically use three components:

```
Internet
   ↓
[Load Balancer / Ingress]
   ↓
[Go Gateway :8080]
   ↓ (HTTP)
[Rust Server :9009 binary, :9010 HTTP]
   ↓
[Persistent Volume /data]
```

## Configuration

### Environment Variables

**Server (Rust):**

| Variable | Default | Description |
|----------|---------|-------------|
| `CXDB_DATA_DIR` | `./data` | Storage directory |
| `CXDB_BIND` | `127.0.0.1:9009` | Binary protocol bind address |
| `CXDB_HTTP_BIND` | `127.0.0.1:9010` | HTTP gateway bind address |
| `CXDB_LOG_LEVEL` | `info` | Log level: debug, info, warn, error |
| `CXDB_LOG_FORMAT` | `json` | Log format: json, text |
| `CXDB_ENABLE_METRICS` | `false` | Enable Prometheus metrics on :9011 |
| `CXDB_MAX_BLOB_SIZE` | `10485760` | Max blob size (10MB) |
| `CXDB_COMPRESSION_LEVEL` | `3` | Zstd compression level (1-22) |

**Gateway (Go):**

| Variable | Required | Description |
|----------|----------|-------------|
| `PORT` | No | HTTP port (default: 8080) |
| `CXDB_BACKEND_URL` | Yes | Rust server HTTP URL |
| `PUBLIC_BASE_URL` | Yes | Public URL for OAuth redirect |
| `GOOGLE_CLIENT_ID` | Yes | OAuth client ID |
| `GOOGLE_CLIENT_SECRET` | Yes | OAuth client secret |
| `GOOGLE_ALLOWED_EMAILS` | No | Comma-separated email allowlist |
| `GOOGLE_ALLOWED_DOMAIN` | No | Allowed email domain (e.g., example.com) |
| `SESSION_SECRET` | Yes | 64-char hex string for cookie signing |
| `DATABASE_PATH` | No | Session DB path (default: ./data/sessions.db) |
| `ALLOWED_RENDERER_ORIGINS` | No | CSP script-src origins (comma-separated) |
| `DEV_MODE` | No | Disable OAuth (development only) |

### Generating Secrets

**Session secret:**

```bash
openssl rand -hex 32
```

## Docker Compose

**docker-compose.yml:**

```yaml
version: '3.8'

services:
  cxdb:
    image: cxdb/cxdb:latest
    restart: unless-stopped
    ports:
      - "9009:9009"
      - "9010:9010"
    volumes:
      - cxdb-data:/data
    environment:
      CXDB_DATA_DIR: /data
      CXDB_BIND: 0.0.0.0:9009
      CXDB_HTTP_BIND: 0.0.0.0:9010
      CXDB_LOG_LEVEL: info
      CXDB_ENABLE_METRICS: "true"
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:9010/health"]
      interval: 30s
      timeout: 3s
      retries: 3

  gateway:
    image: cxdb/gateway:latest
    restart: unless-stopped
    ports:
      - "8080:8080"
    depends_on:
      - cxdb
    environment:
      PORT: 8080
      CXDB_BACKEND_URL: http://cxdb:9010
      PUBLIC_BASE_URL: https://cxdb.example.com
      GOOGLE_CLIENT_ID: ${GOOGLE_CLIENT_ID}
      GOOGLE_CLIENT_SECRET: ${GOOGLE_CLIENT_SECRET}
      GOOGLE_ALLOWED_DOMAIN: example.com
      SESSION_SECRET: ${SESSION_SECRET}
      DATABASE_PATH: /data/sessions.db
      ALLOWED_RENDERER_ORIGINS: https://cdn.strongdm.ai,https://esm.sh
    volumes:
      - gateway-data:/data
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8080/health"]
      interval: 30s
      timeout: 3s
      retries: 3

volumes:
  cxdb-data:
  gateway-data:
```

**.env:**

```bash
GOOGLE_CLIENT_ID=your-client-id.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=your-client-secret
SESSION_SECRET=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
```

**Deploy:**

```bash
docker-compose up -d
docker-compose logs -f
```

## Kubernetes

### Namespace

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: cxdb
```

### ConfigMap

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: cxdb-config
  namespace: cxdb
data:
  CXDB_LOG_LEVEL: "info"
  CXDB_ENABLE_METRICS: "true"
  CXDB_BIND: "0.0.0.0:9009"
  CXDB_HTTP_BIND: "0.0.0.0:9010"
```

### Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: cxdb-secrets
  namespace: cxdb
type: Opaque
stringData:
  GOOGLE_CLIENT_ID: "your-client-id"
  GOOGLE_CLIENT_SECRET: "your-client-secret"
  SESSION_SECRET: "0123456789abcdef..."
```

### PersistentVolumeClaim

```yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: cxdb-storage
  namespace: cxdb
spec:
  accessModes:
    - ReadWriteOnce
  resources:
    requests:
      storage: 50Gi
  storageClassName: fast-ssd
```

### StatefulSet (CXDB Server)

```yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: cxdb
  namespace: cxdb
spec:
  serviceName: cxdb
  replicas: 1  # Single replica for v1
  selector:
    matchLabels:
      app: cxdb
  template:
    metadata:
      labels:
        app: cxdb
    spec:
      containers:
      - name: cxdb
        image: cxdb/cxdb:latest
        ports:
        - name: binary
          containerPort: 9009
        - name: http
          containerPort: 9010
        - name: metrics
          containerPort: 9011
        envFrom:
        - configMapRef:
            name: cxdb-config
        env:
        - name: CXDB_DATA_DIR
          value: /data
        volumeMounts:
        - name: data
          mountPath: /data
        resources:
          requests:
            cpu: 500m
            memory: 512Mi
          limits:
            cpu: 2000m
            memory: 2Gi
        livenessProbe:
          httpGet:
            path: /health
            port: 9010
          initialDelaySeconds: 10
          periodSeconds: 30
        readinessProbe:
          httpGet:
            path: /health
            port: 9010
          initialDelaySeconds: 5
          periodSeconds: 10
  volumeClaimTemplates:
  - metadata:
      name: data
    spec:
      accessModes: ["ReadWriteOnce"]
      resources:
        requests:
          storage: 50Gi
      storageClassName: fast-ssd
```

### Deployment (Gateway)

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: cxdb-gateway
  namespace: cxdb
spec:
  replicas: 2
  selector:
    matchLabels:
      app: cxdb-gateway
  template:
    metadata:
      labels:
        app: cxdb-gateway
    spec:
      containers:
      - name: gateway
        image: cxdb/gateway:latest
        ports:
        - name: http
          containerPort: 8080
        env:
        - name: PORT
          value: "8080"
        - name: CXDB_BACKEND_URL
          value: "http://cxdb:9010"
        - name: PUBLIC_BASE_URL
          value: "https://cxdb.example.com"
        - name: GOOGLE_ALLOWED_DOMAIN
          value: "example.com"
        envFrom:
        - secretRef:
            name: cxdb-secrets
        resources:
          requests:
            cpu: 100m
            memory: 128Mi
          limits:
            cpu: 500m
            memory: 256Mi
        livenessProbe:
          httpGet:
            path: /health
            port: 8080
          initialDelaySeconds: 5
          periodSeconds: 15
```

### Service

```yaml
apiVersion: v1
kind: Service
metadata:
  name: cxdb
  namespace: cxdb
spec:
  type: ClusterIP
  selector:
    app: cxdb
  ports:
  - name: binary
    port: 9009
    targetPort: 9009
  - name: http
    port: 9010
    targetPort: 9010
---
apiVersion: v1
kind: Service
metadata:
  name: cxdb-gateway
  namespace: cxdb
spec:
  type: ClusterIP
  selector:
    app: cxdb-gateway
  ports:
  - name: http
    port: 80
    targetPort: 8080
```

### Ingress

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: cxdb
  namespace: cxdb
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
    nginx.ingress.kubernetes.io/ssl-redirect: "true"
spec:
  ingressClassName: nginx
  tls:
  - hosts:
    - cxdb.example.com
    secretName: cxdb-tls
  rules:
  - host: cxdb.example.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: cxdb-gateway
            port:
              number: 80
```

**Deploy:**

```bash
kubectl apply -f namespace.yaml
kubectl apply -f configmap.yaml
kubectl apply -f secret.yaml
kubectl apply -f statefulset.yaml
kubectl apply -f service.yaml
kubectl apply -f ingress.yaml

# Check status
kubectl get pods -n cxdb
kubectl logs -n cxdb -l app=cxdb -f
```

## TLS Configuration

### Binary Protocol (for writer clients)

Use TLS for production binary protocol connections:

**Server-side:** Use a reverse proxy like nginx or Envoy:

```nginx
# nginx.conf
stream {
  upstream cxdb_binary {
    server 127.0.0.1:9009;
  }

  server {
    listen 9009 ssl;
    ssl_certificate /etc/ssl/certs/cxdb.crt;
    ssl_certificate_key /etc/ssl/private/cxdb.key;
    proxy_pass cxdb_binary;
  }
}
```

**Client-side:**

```go
import "crypto/tls"

tlsConfig := &tls.Config{
    ServerName: "cxdb.example.com",
}
conn, err := tls.Dial("tcp", "cxdb.example.com:9009", tlsConfig)
```

### HTTP Gateway

Use a reverse proxy or ingress controller for HTTPS:

**nginx:**

```nginx
server {
  listen 443 ssl http2;
  server_name cxdb.example.com;

  ssl_certificate /etc/ssl/certs/cxdb.crt;
  ssl_certificate_key /etc/ssl/private/cxdb.key;

  location / {
    proxy_pass http://localhost:8080;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
  }
}
```

## OAuth Setup (Google)

### Create OAuth Credentials

1. Go to [Google Cloud Console](https://console.cloud.google.com)
2. Create or select a project
3. Enable "Google+ API"
4. Navigate to **APIs & Services** → **Credentials**
5. Click **Create Credentials** → **OAuth 2.0 Client ID**
6. Application type: **Web application**
7. Authorized redirect URIs: `https://cxdb.example.com/auth/callback`
8. Save **Client ID** and **Client Secret**

### Configure Gateway

```bash
GOOGLE_CLIENT_ID=123456789-abcdefg.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=GOCSPX-abc123def456ghi789
PUBLIC_BASE_URL=https://cxdb.example.com
GOOGLE_ALLOWED_DOMAIN=example.com  # Or use GOOGLE_ALLOWED_EMAILS
```

## Backup and Restore

### Backup

CXDB's storage is append-only, making backups straightforward:

```bash
# Stop writes (optional but recommended)
# For Docker:
docker exec cxdb kill -USR1 1  # Graceful shutdown of binary protocol

# Backup data directory
tar -czf cxdb-backup-$(date +%Y%m%d).tar.gz /var/lib/cxdb/

# Or rsync for incremental backups
rsync -av --delete /var/lib/cxdb/ /backup/cxdb/
```

### Automated Backups

**Cron job:**

```bash
# /etc/cron.d/cxdb-backup
0 2 * * * root /usr/local/bin/backup-cxdb.sh

# /usr/local/bin/backup-cxdb.sh
#!/bin/bash
set -e

BACKUP_DIR="/backup/cxdb"
DATA_DIR="/var/lib/cxdb"
DATE=$(date +%Y%m%d-%H%M%S)

mkdir -p "$BACKUP_DIR"

# Sync to backup location
rsync -av --delete "$DATA_DIR/" "$BACKUP_DIR/latest/"

# Create timestamped snapshot
cp -al "$BACKUP_DIR/latest" "$BACKUP_DIR/$DATE"

# Keep last 7 days
find "$BACKUP_DIR" -maxdepth 1 -type d -name "202*" -mtime +7 -exec rm -rf {} \;
```

### Restore

```bash
# Stop CXDB
docker stop cxdb  # or systemctl stop cxdb

# Restore data
rm -rf /var/lib/cxdb/*
tar -xzf cxdb-backup-20250130.tar.gz -C /

# Or rsync
rsync -av /backup/cxdb/20250130/ /var/lib/cxdb/

# Start CXDB
docker start cxdb  # or systemctl start cxdb
```

## Monitoring

### Prometheus Metrics

Enable metrics:

```bash
CXDB_ENABLE_METRICS=true
```

Metrics endpoint: `http://localhost:9011/metrics`

**Key metrics:**

- `cxdb_turns_total` - Total turns appended
- `cxdb_blobs_total` - Total blobs stored
- `cxdb_blob_dedup_hits_total` - Deduplication hits
- `cxdb_append_duration_seconds` - Append latency histogram
- `cxdb_storage_bytes` - Storage used by component

**Prometheus config:**

```yaml
scrape_configs:
  - job_name: 'cxdb'
    static_configs:
      - targets: ['cxdb:9011']
```

### Grafana Dashboard

Import the CXDB dashboard:

```bash
curl -o cxdb-dashboard.json https://raw.githubusercontent.com/strongdm/cxdb/main/deploy/grafana-dashboard.json
```

Or create custom panels:

**Append Rate:**
```promql
rate(cxdb_turns_total[5m])
```

**Dedup Hit Rate:**
```promql
rate(cxdb_blob_dedup_hits_total[5m]) / rate(cxdb_blobs_total[5m])
```

**Storage Growth:**
```promql
cxdb_storage_bytes
```

### Logs

CXDB logs to stdout in JSON format (configurable):

```bash
# Follow logs (Docker)
docker logs -f cxdb

# Follow logs (Kubernetes)
kubectl logs -n cxdb -l app=cxdb -f

# Query with jq
docker logs cxdb 2>&1 | jq 'select(.level == "error")'
```

### Alerts

**Prometheus alert rules:**

```yaml
groups:
  - name: cxdb
    rules:
      - alert: CXDBDown
        expr: up{job="cxdb"} == 0
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: "CXDB is down"

      - alert: CXDBHighLatency
        expr: histogram_quantile(0.99, rate(cxdb_append_duration_seconds_bucket[5m])) > 0.1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "CXDB append latency p99 > 100ms"

      - alert: CXDBStorageFull
        expr: cxdb_storage_bytes / (50 * 1024 * 1024 * 1024) > 0.9
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "CXDB storage >90% full"
```

## Performance Tuning

### Storage

**Use fast storage:**
- NVMe SSD recommended
- 10,000+ IOPS for production workloads
- Avoid network-attached storage (high latency)

**Filesystem:**
- ext4 or xfs recommended
- Mount options: `noatime,nodiratime`

**Example:**

```bash
# /etc/fstab
/dev/nvme0n1 /var/lib/cxdb ext4 noatime,nodiratime 0 2
```

### Kernel Tuning

**For high-throughput binary protocol:**

```bash
# /etc/sysctl.d/99-cxdb.conf
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.ipv4.tcp_rmem = 4096 87380 16777216
net.ipv4.tcp_wmem = 4096 65536 16777216
net.core.netdev_max_backlog = 5000
net.ipv4.tcp_max_syn_backlog = 8192
```

Apply:

```bash
sysctl -p /etc/sysctl.d/99-cxdb.conf
```

### Resource Limits

**Docker:**

```yaml
services:
  cxdb:
    deploy:
      resources:
        limits:
          cpus: '4'
          memory: 4G
        reservations:
          cpus: '2'
          memory: 2G
```

**Kubernetes:**

```yaml
resources:
  requests:
    cpu: 2000m
    memory: 2Gi
  limits:
    cpu: 4000m
    memory: 4Gi
```

## Scaling

### Vertical Scaling (v1)

CXDB v1 is single-process. Scale vertically:

- More CPU: 4-8 cores recommended
- More RAM: 4-8GB for hot cache
- Faster storage: NVMe SSD

### Read Replicas (Future)

Not supported in v1. See roadmap for v2 plans.

### Sharding (Future)

Not supported in v1. See roadmap for v2 plans.

## Security

### Network

**Firewall rules:**

```bash
# Allow binary protocol from trusted networks only
iptables -A INPUT -p tcp --dport 9009 -s 10.0.0.0/8 -j ACCEPT
iptables -A INPUT -p tcp --dport 9009 -j DROP

# Allow HTTP gateway from load balancer
iptables -A INPUT -p tcp --dport 9010 -s 10.0.1.0/24 -j ACCEPT
iptables -A INPUT -p tcp --dport 9010 -j DROP
```

### Access Control

**OAuth (Gateway):**
- Use `GOOGLE_ALLOWED_DOMAIN` to restrict by email domain
- Or `GOOGLE_ALLOWED_EMAILS` for explicit allowlist

**Binary Protocol:**
- No built-in authentication in v1
- Use TLS + mutual TLS (mTLS) for client auth
- Or deploy behind VPN/private network

### Data Encryption

**At rest:**
- Use encrypted storage (LUKS, cloud provider encryption)

**In transit:**
- TLS for binary protocol
- HTTPS for gateway

## Troubleshooting

See [troubleshooting.md](troubleshooting.md) for common deployment issues.

## See Also

- [Architecture](architecture.md) - System design
- [HTTP API](http-api.md) - Gateway API reference
- [Protocol](protocol.md) - Binary protocol details
- [Troubleshooting](troubleshooting.md) - Debugging production issues

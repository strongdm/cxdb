# Troubleshooting

This guide covers common issues and debugging techniques for CXDB.

## Connection Issues

### Cannot connect to binary protocol (:9009)

**Symptoms:**
```
Error: dial tcp 127.0.0.1:9009: connect: connection refused
```

**Solutions:**

1. **Check if server is running:**
   ```bash
   # Docker
   docker ps | grep cxdb
   docker logs cxdb

   # Systemd
   systemctl status cxdb

   # Process check
   ps aux | grep ai-cxdb-store
   netstat -an | grep 9009
   ```

2. **Check bind address:**
   ```bash
   # Server must bind to 0.0.0.0 or the correct interface
   CXDB_BIND=0.0.0.0:9009 ./ai-cxdb-store
   ```

3. **Check firewall:**
   ```bash
   # Linux
   iptables -L -n | grep 9009
   ufw status

   # macOS
   sudo pfctl -s rules | grep 9009
   ```

4. **Check Docker port mapping:**
   ```bash
   docker port cxdb
   # Should show: 9009/tcp -> 0.0.0.0:9009
   ```

### Cannot connect to HTTP gateway (:9010)

**Symptoms:**
```
curl: (7) Failed to connect to localhost port 9010: Connection refused
```

**Solutions:**

1. **Check HTTP bind address:**
   ```bash
   CXDB_HTTP_BIND=0.0.0.0:9010 ./ai-cxdb-store
   ```

2. **Check server logs:**
   ```bash
   docker logs cxdb 2>&1 | grep "HTTP gateway listening"
   ```

3. **Test from inside container:**
   ```bash
   docker exec cxdb curl http://localhost:9010/health
   ```

### TLS handshake failures

**Symptoms:**
```
tls: bad certificate
tls: first record does not look like a TLS handshake
```

**Solutions:**

1. **Verify server certificate:**
   ```bash
   openssl s_client -connect cxdb.example.com:9009 -showcerts
   ```

2. **Check certificate validity:**
   ```bash
   openssl x509 -in /etc/ssl/certs/cxdb.crt -text -noout
   ```

3. **Use correct server name:**
   ```go
   tlsConfig := &tls.Config{
       ServerName: "cxdb.example.com",  // Must match cert CN/SAN
   }
   ```

## OAuth and Authentication

### Gateway redirects to login in loop

**Symptoms:**
- Browser redirects to `/login` repeatedly
- Cookie not being set

**Solutions:**

1. **Check PUBLIC_BASE_URL:**
   ```bash
   # Must match the URL you're accessing
   PUBLIC_BASE_URL=https://cxdb.example.com  # Not http:// or localhost
   ```

2. **Check cookie domain:**
   - If accessing via IP (http://10.0.0.1), OAuth won't work
   - Use a proper domain name

3. **Check session secret:**
   ```bash
   # Must be 64 hex characters
   echo $SESSION_SECRET | wc -c  # Should be 64
   ```

4. **Clear cookies:**
   - In browser, clear cookies for the domain
   - Try in incognito mode

### "Unauthorized" errors after login

**Symptoms:**
```json
{"error": {"code": "UNAUTHORIZED", "message": "Invalid session"}}
```

**Solutions:**

1. **Check session database:**
   ```bash
   sqlite3 /path/to/sessions.db "SELECT * FROM sessions;"
   ```

2. **Check gateway logs:**
   ```bash
   docker logs cxdb-gateway | grep -i auth
   ```

3. **Verify OAuth credentials:**
   ```bash
   echo $GOOGLE_CLIENT_ID
   echo $GOOGLE_CLIENT_SECRET
   ```

4. **Check allowed domain:**
   ```bash
   # If set, user's email domain must match
   GOOGLE_ALLOWED_DOMAIN=example.com
   ```

### OAuth callback fails

**Symptoms:**
```
Error: redirect_uri_mismatch
```

**Solutions:**

1. **Add redirect URI in Google Console:**
   - Go to [Google Cloud Console](https://console.cloud.google.com)
   - APIs & Services → Credentials
   - Edit OAuth 2.0 Client
   - Add: `https://cxdb.example.com/auth/callback`

2. **Ensure PUBLIC_BASE_URL matches:**
   ```bash
   PUBLIC_BASE_URL=https://cxdb.example.com  # Must match redirect URI
   ```

## Storage Issues

### "No space left on device"

**Symptoms:**
```
Error: failed to write blob: no space left on device
```

**Solutions:**

1. **Check disk space:**
   ```bash
   df -h /var/lib/cxdb
   ```

2. **Increase volume size (Docker):**
   ```bash
   docker volume inspect cxdb-data
   # May need to resize or create new volume
   ```

3. **Increase PVC size (Kubernetes):**
   ```bash
   kubectl edit pvc cxdb-storage -n cxdb
   # Change spec.resources.requests.storage
   kubectl get pvc -n cxdb  # Wait for resize
   ```

4. **Clean up old data (if safe):**
   ```bash
   # DANGER: This deletes data!
   # Only if you have backups
   rm -rf /var/lib/cxdb/blobs/blobs.pack
   ```

### Corrupted storage / CRC failures

**Symptoms:**
```
ERROR: CRC mismatch in turn record at offset 1234567
ERROR: Truncating turns.log to last valid record
```

**Solutions:**

1. **Let server recover automatically:**
   - CXDB truncates to last valid record on startup
   - Restart the server:
     ```bash
     docker restart cxdb
     ```

2. **Check logs for recovery:**
   ```bash
   docker logs cxdb 2>&1 | grep -i "recovery\|truncate"
   ```

3. **Restore from backup:**
   ```bash
   docker stop cxdb
   rsync -av /backup/cxdb/ /var/lib/cxdb/
   docker start cxdb
   ```

4. **If blobs are corrupted:**
   ```bash
   # Check blob integrity
   docker exec cxdb /app/cxdb --verify-blobs
   ```

### "Blob not found" errors

**Symptoms:**
```json
{"error": {"code": "NOT_FOUND", "message": "Blob abc123... not found"}}
```

**Solutions:**

1. **Check blob store:**
   ```bash
   ls -lh /var/lib/cxdb/blobs/
   ```

2. **Rebuild blob index:**
   ```bash
   docker exec cxdb /app/cxdb --rebuild-blob-index
   ```

3. **Check for disk errors:**
   ```bash
   dmesg | grep -i error
   smartctl -a /dev/sda
   ```

## Performance Issues

### Slow append operations

**Symptoms:**
- Append latency > 100ms
- High p99 latency

**Solutions:**

1. **Check storage latency:**
   ```bash
   # Test write speed
   dd if=/dev/zero of=/var/lib/cxdb/test bs=1M count=100
   ```

2. **Enable metrics and check:**
   ```bash
   curl http://localhost:9011/metrics | grep append_duration
   ```

3. **Check for storage contention:**
   ```bash
   iostat -x 1
   # Look for high %util or await
   ```

4. **Increase compression level (if CPU available):**
   ```bash
   # Trade CPU for less I/O
   CXDB_COMPRESSION_LEVEL=6  # Default is 3
   ```

5. **Use faster storage:**
   - NVMe SSD recommended
   - Avoid network-attached storage

### High memory usage

**Symptoms:**
- Server using >4GB RAM
- OOM kills

**Solutions:**

1. **Check blob cache size:**
   ```bash
   docker stats cxdb
   ```

2. **Limit container memory (Docker):**
   ```yaml
   services:
     cxdb:
       deploy:
         resources:
           limits:
             memory: 2G
   ```

3. **Reduce cache size (future config option):**
   ```bash
   # Not yet configurable in v1
   # Use resource limits as workaround
   ```

4. **Check for memory leaks:**
   ```bash
   docker logs cxdb 2>&1 | grep -i "memory\|oom"
   ```

### Slow turn retrieval

**Symptoms:**
- GET /v1/contexts/:id/turns takes >1s
- High CPU during reads

**Solutions:**

1. **Limit turn count:**
   ```bash
   curl "http://localhost:9010/v1/contexts/1/turns?limit=10"
   # Default is 64, lower if needed
   ```

2. **Use `view=raw` to skip projection:**
   ```bash
   curl "http://localhost:9010/v1/contexts/1/turns?view=raw"
   ```

3. **Check turn store size:**
   ```bash
   ls -lh /var/lib/cxdb/turns/
   ```

4. **Enable caching (future):**
   ```bash
   # Not yet configurable in v1
   ```

## Type Registry Issues

### "Type not found" errors

**Symptoms:**
```json
{"error": {"code": "TYPE_NOT_FOUND", "message": "com.example.Message v1 not in registry"}}
```

**Solutions:**

1. **Publish registry bundle:**
   ```bash
   curl -X PUT http://localhost:9010/v1/registry/bundles/latest \
     -H "Content-Type: application/json" \
     -d @bundle.json
   ```

2. **Check registry directory:**
   ```bash
   ls -lh /var/lib/cxdb/registry/bundles/
   cat /var/lib/cxdb/registry/index.json
   ```

3. **Verify type_id and version:**
   ```bash
   curl http://localhost:9010/v1/registry/types/com.example.Message/versions/1
   ```

### "Tag reuse detected" errors

**Symptoms:**
```json
{"error": {"code": "INVALID_EVOLUTION", "message": "Tag 3 reused in version 2"}}
```

**Solutions:**

1. **Never reuse tags:**
   - If you remove a field, don't reuse its tag
   - Assign new tags for new fields

2. **Check bundle for conflicts:**
   ```bash
   jq '.types."com.example.Message".versions' bundle.json
   ```

3. **Increment version number:**
   - If you removed tag 3 in v2, don't add a new field with tag 3
   - Use tag 4 instead

### Projection failures

**Symptoms:**
```json
{"error": {"code": "PROJECTION_ERROR", "message": "Failed to decode msgpack"}}
```

**Solutions:**

1. **Check msgpack encoding:**
   ```bash
   # View raw bytes
   curl "http://localhost:9010/v1/contexts/1/turns?view=raw" | jq '.turns[0].bytes_b64' -r | base64 -d | xxd
   ```

2. **Verify field tags match:**
   - Go struct tags must match registry tags
   - Registry tags must be integers

3. **Check for type mismatches:**
   ```json
   // Registry says tag 1 is string
   {"1": 42}  // But payload has int - ERROR
   ```

4. **Use `include_unknown=1` to debug:**
   ```bash
   curl "http://localhost:9010/v1/contexts/1/turns?include_unknown=1"
   ```

## Renderer Issues

### Renderer not loading

**Symptoms:**
- Turn shows "Renderer not found"
- Default JSON view displayed

**Solutions:**

1. **Check type_id matches:**
   ```javascript
   // renderers.ts
   type_id: 'com.example.Chart'  // Must match turn's declared_type.type_id exactly
   ```

2. **Verify renderer URL:**
   ```bash
   curl -I https://cdn.example.com/renderers/chart@1.0.0.js
   # Should return 200 OK
   ```

3. **Check browser console:**
   ```
   Press F12 → Console
   Look for: Failed to load renderer, CORS errors, etc.
   ```

4. **Test renderer URL directly:**
   ```
   Open https://cdn.example.com/renderers/chart@1.0.0.js in browser
   Should download or display JS code
   ```

### CSP blocks renderer

**Symptoms:**
```
Refused to load the script 'https://custom-cdn.com/renderer.js' because it violates the following Content Security Policy directive: "script-src 'self' https://cdn.strongdm.ai"
```

**Solutions:**

1. **Add CDN to allowed origins:**
   ```bash
   # gateway/.env
   ALLOWED_RENDERER_ORIGINS=https://cdn.strongdm.ai,https://custom-cdn.com
   ```

2. **Restart gateway:**
   ```bash
   docker restart cxdb-gateway
   ```

3. **Verify CSP header:**
   ```bash
   curl -I http://localhost:8080/ | grep -i content-security-policy
   ```

### Renderer crashes

**Symptoms:**
```
Renderer error: Cannot read property 'map' of undefined
```

**Solutions:**

1. **Check data shape:**
   ```bash
   curl "http://localhost:9010/v1/contexts/1/turns?limit=1" | jq '.turns[0].data'
   ```

2. **Add validation in renderer:**
   ```javascript
   export default function MyRenderer({ data }) {
     if (!data || !data.points) {
       return <div>Invalid data</div>;
     }
     // ... render
   }
   ```

3. **Check browser console for errors:**
   ```
   Press F12 → Console
   ```

4. **Test renderer locally:**
   ```html
   <!-- test.html -->
   <script type="module">
     import Renderer from './renderer.js';
     // Test with sample data
   </script>
   ```

## Binary Protocol Issues

### "Hash mismatch" errors

**Symptoms:**
```
ERROR: Content hash verification failed
Expected: a3f5b8c2...
Actual: b4e6c9d3...
```

**Solutions:**

1. **Compute hash correctly:**
   ```go
   import "github.com/zeebo/blake3"

   uncompressed := msgpack.Marshal(payload)
   hash := blake3.Sum256(uncompressed)  // Hash BEFORE compression
   ```

2. **Don't hash compressed data:**
   ```go
   // WRONG
   compressed := zstd.Encode(uncompressed)
   hash := blake3.Sum256(compressed)  // NO!

   // CORRECT
   hash := blake3.Sum256(uncompressed)  // YES
   compressed := zstd.Encode(uncompressed)
   ```

### "Invalid parent_turn_id" errors

**Symptoms:**
```json
{"error": {"code": "CONFLICT", "message": "Parent turn 999 not found"}}
```

**Solutions:**

1. **Use 0 for current head:**
   ```go
   req := &AppendRequest{
     ParentTurnID: 0,  // Use current context head
     // ...
   }
   ```

2. **Verify parent exists:**
   ```bash
   curl http://localhost:9010/v1/contexts/1/turns | jq '.turns[].turn_id'
   ```

3. **Check context head:**
   ```bash
   curl http://localhost:9010/v1/contexts/1 | jq .head_turn_id
   ```

### Frame parsing errors

**Symptoms:**
```
ERROR: Invalid frame header
ERROR: Unexpected EOF
```

**Solutions:**

1. **Ensure little-endian:**
   ```go
   binary.LittleEndian.PutUint32(buf, len)  // Not BigEndian
   ```

2. **Write complete frames:**
   ```go
   // Write header
   binary.Write(conn, binary.LittleEndian, header)
   // Write full payload
   conn.Write(payload)
   // Flush
   conn.(*net.TCPConn).SetWriteDeadline(time.Now().Add(5*time.Second))
   ```

3. **Check network issues:**
   ```bash
   tcpdump -i lo0 -w cxdb.pcap port 9009
   wireshark cxdb.pcap
   ```

## Debugging Techniques

### Enable debug logging

```bash
CXDB_LOG_LEVEL=debug ./ai-cxdb-store
```

Or for Docker:

```bash
docker run -e CXDB_LOG_LEVEL=debug cxdb/cxdb:latest
```

### Trace protocol frames

```bash
CXDB_LOG_LEVEL=debug CXDB_TRACE_PROTOCOL=1 ./ai-cxdb-store
```

Output:
```
DEBUG: → APPEND_TURN req_id=1 len=1234
DEBUG: ← APPEND_TURN_ACK req_id=1 turn_id=42
```

### Inspect storage files

```bash
# Check file sizes
du -sh /var/lib/cxdb/*

# Check turn count
wc -l /var/lib/cxdb/turns/turns.log

# Check blob count
ls /var/lib/cxdb/blobs/ | wc -l

# Verify file integrity
md5sum /var/lib/cxdb/turns/turns.log
```

### Use health endpoint

```bash
curl http://localhost:9010/health | jq .
```

Output:
```json
{
  "status": "ok",
  "version": "1.0.0",
  "uptime_seconds": 3600,
  "storage": {
    "turns": 1000,
    "blobs": 500,
    "contexts": 10
  }
}
```

### Check metrics

```bash
curl http://localhost:9011/metrics | grep cxdb_
```

### Verify configuration

```bash
# Print env vars
docker exec cxdb env | grep CXDB_

# Check config file (if using one)
docker exec cxdb cat /etc/cxdb/config.toml
```

## Getting Help

If you're still stuck:

1. **Check GitHub Issues:** https://github.com/strongdm/cxdb/issues
2. **Search Discussions:** https://github.com/strongdm/cxdb/discussions
3. **Join Slack:** https://strongdm-community.slack.com #cxdb
4. **File a bug report:** Include:
   - CXDB version
   - Deployment environment (Docker/K8s/bare metal)
   - Relevant logs (use `CXDB_LOG_LEVEL=debug`)
   - Steps to reproduce

## See Also

- [Deployment](deployment.md) - Production setup
- [Architecture](architecture.md) - System design for debugging
- [Protocol](protocol.md) - Binary protocol details
- [HTTP API](http-api.md) - REST API reference

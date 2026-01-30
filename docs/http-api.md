# HTTP API Reference

The CXDB HTTP gateway provides a JSON API for reading turns, managing contexts, and publishing type registry bundles. It's designed for UI clients and tooling that need typed projections.

**Base URL:** `http://localhost:9010` (development) or `https://your-domain.com` (production with gateway)

## Authentication

**Development:** No authentication required when connecting directly to the Rust server

**Production:** The Go gateway provides Google OAuth authentication:
- Unauthenticated requests to `/v1/*` return `302 Found` redirect to `/login`
- After OAuth, requests include session cookie
- Session expires after 24 hours of inactivity

## Contexts

### List Contexts

```http
GET /v1/contexts
```

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `limit` | int | 100 | Max contexts to return |
| `offset` | int | 0 | Pagination offset |

**Response:**

```json
{
  "contexts": [
    {
      "context_id": "1",
      "head_turn_id": "42",
      "head_depth": 42,
      "created_at": "2025-01-30T10:00:00Z"
    }
  ],
  "total": 1
}
```

### Get Context Details

```http
GET /v1/contexts/:context_id
```

**Response:**

```json
{
  "context_id": "1",
  "head_turn_id": "42",
  "head_depth": 42,
  "created_at": "2025-01-30T10:00:00Z"
}
```

**Error Responses:**

- `404 Not Found` - Context doesn't exist

### Create Context

```http
POST /v1/contexts/create
```

**Request Body:**

```json
{
  "base_turn_id": "0"
}
```

- `base_turn_id`: `"0"` for empty context, or turn ID to start from

**Response:**

```json
{
  "context_id": "1",
  "head_turn_id": "0",
  "head_depth": 0
}
```

### Fork Context

```http
POST /v1/contexts/fork
```

**Request Body:**

```json
{
  "base_turn_id": "42"
}
```

**Response:**

```json
{
  "context_id": "2",
  "head_turn_id": "42",
  "head_depth": 42
}
```

Creates a new context whose head is the specified turn. The new context shares history up to that turn but can diverge with new appends.

## Turns

### Get Turns from Context

```http
GET /v1/contexts/:context_id/turns
```

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `limit` | int | 64 | Max turns to return |
| `before_turn_id` | string | - | For paging: return turns older than this |
| `view` | string | `typed` | Response format: `typed`, `raw`, `both` |
| `type_hint_mode` | string | `inherit` | Type resolution: `inherit`, `latest`, `explicit` |
| `as_type_id` | string | - | Override type (requires `explicit` mode) |
| `as_type_version` | int | - | Override version (requires `explicit` mode) |
| `include_unknown` | bool | false | Include unknown fields in response |
| `bytes_render` | string | `base64` | Binary encoding: `base64`, `hex`, `len_only` |
| `u64_format` | string | `string` | Large int format: `string`, `number` |
| `enum_render` | string | `label` | Enum display: `label`, `number`, `both` |
| `time_render` | string | `iso` | Timestamp format: `iso`, `unix_ms` |

**Response (`view=typed`):**

```json
{
  "meta": {
    "context_id": "1",
    "head_turn_id": "3",
    "head_depth": 3,
    "registry_bundle_id": "2025-01-30T10:00:00Z#abc123"
  },
  "turns": [
    {
      "turn_id": "1",
      "parent_turn_id": "0",
      "depth": 1,
      "declared_type": {
        "type_id": "com.example.Message",
        "type_version": 1
      },
      "decoded_as": {
        "type_id": "com.example.Message",
        "type_version": 1
      },
      "data": {
        "role": "user",
        "text": "What is 2+2?"
      }
    },
    {
      "turn_id": "2",
      "parent_turn_id": "1",
      "depth": 2,
      "declared_type": {
        "type_id": "com.example.Message",
        "type_version": 1
      },
      "decoded_as": {
        "type_id": "com.example.Message",
        "type_version": 1
      },
      "data": {
        "role": "assistant",
        "text": "2+2 equals 4."
      }
    }
  ],
  "next_before_turn_id": "1"
}
```

**Response (`view=raw`):**

```json
{
  "meta": { ... },
  "turns": [
    {
      "turn_id": "1",
      "parent_turn_id": "0",
      "depth": 1,
      "declared_type": {
        "type_id": "com.example.Message",
        "type_version": 1
      },
      "content_hash_b3": "a3f5b8c2...",
      "encoding": 1,
      "compression": 0,
      "uncompressed_len": 42,
      "bytes_b64": "gaJyb2xlo3VzZXK..."
    }
  ]
}
```

**Response (`view=both`):**

Combines both `data` and raw fields in each turn.

**Paging:**

To fetch older turns:

```http
GET /v1/contexts/1/turns?limit=10&before_turn_id=100
```

Use `next_before_turn_id` from the previous response to continue paging.

### Append Turn

```http
POST /v1/contexts/:context_id/append
```

**Request Body:**

```json
{
  "type_id": "com.example.Message",
  "type_version": 1,
  "data": {
    "role": "user",
    "text": "Hello!"
  },
  "parent_turn_id": "0",
  "idempotency_key": "client-123-1706615000-001"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type_id` | string | Yes | Type identifier |
| `type_version` | int | Yes | Type version |
| `data` | object | Yes | Turn payload (will be encoded as msgpack) |
| `parent_turn_id` | string | No | Parent turn (default: current head) |
| `idempotency_key` | string | No | For safe retries |

**Response:**

```json
{
  "context_id": "1",
  "turn_id": "1",
  "depth": 1,
  "content_hash": "a3f5b8c2..."
}
```

**Error Responses:**

- `404 Not Found` - Context doesn't exist
- `409 Conflict` - Invalid parent_turn_id
- `422 Unprocessable Entity` - Invalid data or missing type

**Note:** The HTTP API accepts JSON `data` and converts it to msgpack internally. Numeric field tags are derived from the type registry. For maximum control over msgpack encoding, use the binary protocol.

## Registry

### Publish Type Bundle

```http
PUT /v1/registry/bundles/:bundle_id
```

**Request Body:** (JSON)

```json
{
  "registry_version": 1,
  "bundle_id": "2025-01-30T10:00:00Z#abc123",
  "types": {
    "com.example.Message": {
      "versions": {
        "1": {
          "fields": {
            "1": { "name": "role", "type": "string" },
            "2": { "name": "text", "type": "string", "optional": true },
            "3": { "name": "timestamp", "type": "u64", "semantic": "unix_ms" }
          }
        },
        "2": {
          "fields": {
            "1": { "name": "role", "type": "string" },
            "2": { "name": "text", "type": "string", "optional": true },
            "3": { "name": "timestamp", "type": "u64", "semantic": "unix_ms" },
            "4": { "name": "attachments", "type": "array", "items": "bytes" }
          }
        }
      }
    }
  },
  "enums": {
    "com.example.Role": {
      "1": "system",
      "2": "user",
      "3": "assistant",
      "4": "tool"
    }
  }
}
```

**Response:**

- `201 Created` - New bundle stored
- `204 No Content` - Identical bundle already exists
- `409 Conflict` - Invalid evolution (tag reuse, version regression)
- `422 Unprocessable Entity` - Malformed bundle

**Bundle ID Format:**

Use timestamp + hash: `2025-01-30T10:00:00Z#abc123`

### Get Type Bundle

```http
GET /v1/registry/bundles/:bundle_id
```

**Response:**

```json
{
  "registry_version": 1,
  "bundle_id": "...",
  "types": { ... }
}
```

**Headers:**

- `ETag: "abc123"` - For caching
- `Cache-Control: public, max-age=31536000` - Bundles are immutable

**Conditional Requests:**

```http
GET /v1/registry/bundles/:bundle_id
If-None-Match: "abc123"
```

Returns `304 Not Modified` if ETag matches.

### Get Type Version Descriptor

```http
GET /v1/registry/types/:type_id/versions/:type_version
```

**Example:**

```http
GET /v1/registry/types/com.example.Message/versions/1
```

**Response:**

```json
{
  "type_id": "com.example.Message",
  "type_version": 1,
  "fields": {
    "1": { "name": "role", "type": "string" },
    "2": { "name": "text", "type": "string", "optional": true }
  }
}
```

**Error Responses:**

- `404 Not Found` - Type or version doesn't exist

### List Latest Type Versions

```http
GET /v1/registry/types
```

**Response:**

```json
{
  "types": [
    {
      "type_id": "com.example.Message",
      "latest_version": 2,
      "bundle_id": "2025-01-30T10:00:00Z#abc123"
    }
  ]
}
```

## Blobs

### Get Blob by Hash

```http
GET /v1/blobs/:content_hash
```

**Example:**

```http
GET /v1/blobs/a3f5b8c2d1e4f6a9b2c5d8e1f4a7b0c3d6e9f2a5b8c1d4e7f0a3b6c9d2e5f8a1
```

**Response:**

- Content-Type: `application/octet-stream`
- Body: Raw uncompressed bytes

**Error Responses:**

- `404 Not Found` - Blob doesn't exist

## Health and Status

### Health Check

```http
GET /health
```

**Response:**

```json
{
  "status": "ok",
  "version": "1.0.0",
  "uptime_seconds": 3600
}
```

### Storage Stats

```http
GET /v1/stats
```

**Response:**

```json
{
  "contexts": 100,
  "turns": 10000,
  "blobs": 5000,
  "storage_bytes": 52428800,
  "dedup_hit_rate": 0.35
}
```

## Error Responses

All errors return JSON with this format:

```json
{
  "error": {
    "code": "NOT_FOUND",
    "message": "Context 999 not found",
    "details": {
      "context_id": "999"
    }
  }
}
```

**Common Error Codes:**

| HTTP Status | Code | Description |
|-------------|------|-------------|
| 400 | `BAD_REQUEST` | Malformed request |
| 401 | `UNAUTHORIZED` | Missing/invalid auth (gateway only) |
| 404 | `NOT_FOUND` | Resource doesn't exist |
| 409 | `CONFLICT` | Invalid operation (e.g., bad parent) |
| 412 | `PRECONDITION_FAILED` | Missing type registry |
| 422 | `UNPROCESSABLE_ENTITY` | Invalid data |
| 424 | `FAILED_DEPENDENCY` | Missing type descriptor |
| 500 | `INTERNAL_ERROR` | Server error |

## Rate Limiting

**Development:** No rate limits

**Production (with gateway):**
- 1000 requests/minute per user
- `429 Too Many Requests` when exceeded
- `Retry-After: 60` header indicates retry time

## CORS

**Development:** All origins allowed (`Access-Control-Allow-Origin: *`)

**Production (with gateway):** Configured via `ALLOWED_ORIGINS` environment variable

## Examples

### Complete Flow: Create Context and Add Turns

```bash
# Create context
curl -X POST http://localhost:9010/v1/contexts/create \
  -H "Content-Type: application/json" \
  -d '{"base_turn_id": "0"}' \
  | jq .

# Output: {"context_id": "1", "head_turn_id": "0", "head_depth": 0}

# Append user message
curl -X POST http://localhost:9010/v1/contexts/1/append \
  -H "Content-Type: application/json" \
  -d '{
    "type_id": "com.example.Message",
    "type_version": 1,
    "data": {
      "role": "user",
      "text": "What is the weather?"
    }
  }' | jq .

# Output: {"context_id": "1", "turn_id": "1", "depth": 1, ...}

# Append assistant response
curl -X POST http://localhost:9010/v1/contexts/1/append \
  -H "Content-Type: application/json" \
  -d '{
    "type_id": "com.example.Message",
    "type_version": 1,
    "data": {
      "role": "assistant",
      "text": "I need your location to check the weather."
    }
  }' | jq .

# Get conversation
curl http://localhost:9010/v1/contexts/1/turns?limit=10 | jq .
```

### Branching Example

```bash
# Fork from turn 1
curl -X POST http://localhost:9010/v1/contexts/fork \
  -H "Content-Type: application/json" \
  -d '{"base_turn_id": "1"}' \
  | jq .

# Output: {"context_id": "2", "head_turn_id": "1", "head_depth": 1}

# Append alternate response to new context
curl -X POST http://localhost:9010/v1/contexts/2/append \
  -H "Content-Type: application/json" \
  -d '{
    "type_id": "com.example.Message",
    "type_version": 1,
    "data": {
      "role": "assistant",
      "text": "The weather is sunny and 72°F."
    }
  }' | jq .
```

### Publish Type Registry

```bash
curl -X PUT http://localhost:9010/v1/registry/bundles/2025-01-30T10:00:00Z \
  -H "Content-Type: application/json" \
  -d '{
    "registry_version": 1,
    "bundle_id": "2025-01-30T10:00:00Z",
    "types": {
      "com.example.Message": {
        "versions": {
          "1": {
            "fields": {
              "1": {"name": "role", "type": "string"},
              "2": {"name": "text", "type": "string"}
            }
          }
        }
      }
    }
  }'
```

## Client Libraries

### JavaScript/TypeScript

```typescript
import { CxdbClient } from '@strongdm/cxdb';

const client = new CxdbClient('http://localhost:9010');

// Create context
const ctx = await client.createContext();

// Append turn
const turn = await client.appendTurn(ctx.context_id, {
  type_id: 'com.example.Message',
  type_version: 1,
  data: {
    role: 'user',
    text: 'Hello!'
  }
});

// Get turns
const turns = await client.getTurns(ctx.context_id, { limit: 10 });
```

### Python

```python
from cxdb import Client

client = Client("http://localhost:9010")

# Create context
ctx = client.create_context()

# Append turn
turn = client.append_turn(ctx.context_id, {
    "type_id": "com.example.Message",
    "type_version": 1,
    "data": {
        "role": "user",
        "text": "Hello!"
    }
})

# Get turns
turns = client.get_turns(ctx.context_id, limit=10)
```

## See Also

- [Binary Protocol](protocol.md) - For high-throughput writers
- [Type Registry](type-registry.md) - Defining custom types
- [Renderers](renderers.md) - Custom UI visualizations
- [Troubleshooting](troubleshooting.md) - Debugging API issues

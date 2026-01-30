# HTTP Module

JSON HTTP gateway for UI and tooling.

## Overview

The HTTP module provides a REST-ish JSON API for reading turns, managing contexts, and publishing type registry bundles. It's designed for browser clients and tools that need typed projections.

## Endpoints

### Contexts

- `GET /v1/contexts` - List contexts
- `GET /v1/contexts/:id` - Get context details
- `POST /v1/contexts/create` - Create context
- `POST /v1/contexts/fork` - Fork from turn

### Turns

- `GET /v1/contexts/:id/turns` - Get turns with optional projection
- `POST /v1/contexts/:id/append` - Append turn

### Registry

- `PUT /v1/registry/bundles/:id` - Publish bundle
- `GET /v1/registry/bundles/:id` - Fetch bundle
- `GET /v1/registry/types/:type_id/versions/:version` - Get descriptor

### Blobs

- `GET /v1/blobs/:hash` - Fetch blob by hash

### Health

- `GET /health` - Health check
- `GET /v1/stats` - Storage stats

## Implementation

### Server Setup

```rust
use axum::{Router, routing::get};

pub fn create_router(store: Arc<Store>) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/v1/contexts", get(list_contexts).post(create_context))
        .route("/v1/contexts/:id", get(get_context))
        .route("/v1/contexts/:id/turns", get(get_turns).post(append_turn))
        .route("/v1/registry/bundles/:id", get(get_bundle).put(put_bundle))
        .layer(Extension(store))
}
```

### Handlers

**GET /v1/contexts/:id/turns:**

```rust
async fn get_turns(
    Path(context_id): Path<u64>,
    Query(params): Query<TurnsQuery>,
    Extension(store): Extension<Arc<Store>>,
) -> Result<Json<TurnsResponse>, AppError> {
    let turns = store.get_last(context_id, params.limit.unwrap_or(64))?;

    let projected = if params.view == Some("typed") {
        project_turns(&turns, &store.registry, &params)?
    } else {
        raw_turns(&turns)
    };

    Ok(Json(TurnsResponse {
        meta: ContextMeta {
            context_id,
            head_turn_id: /* ... */,
            head_depth: /* ... */,
        },
        turns: projected,
    }))
}
```

**POST /v1/contexts/:id/append:**

```rust
async fn append_turn(
    Path(context_id): Path<u64>,
    Json(req): Json<AppendRequest>,
    Extension(store): Extension<Arc<Store>>,
) -> Result<Json<AppendResponse>, AppError> {
    // Encode data as msgpack
    let payload = rmp_serde::to_vec(&req.data)?;

    // Compute hash
    let hash = blake3::hash(&payload);

    // Store blob
    store.blob_store.put(&hash, &payload)?;

    // Append turn
    let turn = store.turn_store.append_turn(
        context_id,
        req.parent_turn_id.unwrap_or(0),
        hash,
        &req.type_id,
        req.type_version,
    )?;

    Ok(Json(AppendResponse {
        context_id,
        turn_id: turn.turn_id,
        depth: turn.depth,
        content_hash: hex::encode(hash),
    }))
}
```

## Query Parameters

### TurnsQuery

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `limit` | int | 64 | Max turns to return |
| `before_turn_id` | string | - | Pagination cursor |
| `view` | enum | `typed` | `typed`, `raw`, `both` |
| `type_hint_mode` | enum | `inherit` | Type resolution mode |
| `include_unknown` | bool | false | Include unknown fields |
| `bytes_render` | enum | `base64` | Binary encoding |
| `u64_format` | enum | `string` | Large int format |

## Error Handling

Errors return JSON:

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

**HTTP status codes:**

| Status | Code | Use Case |
|--------|------|----------|
| 200 | OK | Success |
| 201 | CREATED | Resource created |
| 400 | BAD_REQUEST | Invalid request |
| 404 | NOT_FOUND | Resource missing |
| 409 | CONFLICT | Invalid state |
| 422 | UNPROCESSABLE_ENTITY | Validation error |
| 500 | INTERNAL_ERROR | Server error |

## CORS

Default configuration allows all origins:

```rust
use tower_http::cors::{CorsLayer, Any};

let cors = CorsLayer::new()
    .allow_origin(Any)
    .allow_methods(Any)
    .allow_headers(Any);

Router::new()
    // ... routes
    .layer(cors)
```

Production should restrict origins.

## Middleware

### Logging

```rust
use tower_http::trace::TraceLayer;

Router::new()
    // ... routes
    .layer(TraceLayer::new_for_http())
```

### Compression

```rust
use tower_http::compression::CompressionLayer;

Router::new()
    // ... routes
    .layer(CompressionLayer::new())
```

## Testing

```bash
# Run HTTP tests
cargo test --package ai-cxdb-store --lib http

# Integration tests
cargo test --test http_integration
```

## Performance

**Typical latencies:**
- GET /v1/contexts/:id/turns (10 turns, typed): ~5ms
- POST /v1/contexts/:id/append: ~2ms
- GET /v1/blobs/:hash: ~1ms

## See Also

- [HTTP API Spec](../../docs/http-api.md) - Complete API reference
- [Projection Module](../projection/README.md) - Typed JSON conversion
- [Registry Module](../registry/README.md) - Type descriptors

# Projection Module

Msgpack → typed JSON conversion using registry descriptors.

## Overview

The projection module takes raw msgpack bytes with numeric field tags and converts them to typed JSON with named fields, applying type coercions (u64→string, bytes→base64, enums→labels) based on registry descriptors.

## Pipeline

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  Msgpack     │────►│  Normalize   │────►│  Project     │
│  (numeric    │     │  (tags → u64)│     │  (apply      │
│   keys)      │     │              │     │   descriptor)│
└──────────────┘     └──────────────┘     └──────────────┘
                                                  │
                                                  ▼
                                          ┌──────────────┐
                                          │  Apply       │
                                          │  Rendering   │
                                          │  Options     │
                                          └──────────────┘
                                                  │
                                                  ▼
                                          ┌──────────────┐
                                          │  Typed JSON  │
                                          └──────────────┘
```

## API

### Project a Turn

```rust
use projection::{project_turn, ProjectionOptions};

let options = ProjectionOptions {
    include_unknown: false,
    bytes_render: BytesRender::Base64,
    u64_format: U64Format::String,
    enum_render: EnumRender::Label,
    time_render: TimeRender::Iso,
};

let result = project_turn(
    &msgpack_bytes,
    &descriptor,
    &options,
)?;

println!("{}", serde_json::to_string_pretty(&result.data)?);
```

### ProjectionResult

```rust
pub struct ProjectionResult {
    pub data: serde_json::Value,        // Typed fields
    pub unknown: Option<serde_json::Value>,  // Unknown tags (if include_unknown)
}
```

## Rendering Options

### BytesRender

```rust
pub enum BytesRender {
    Base64,   // "iVBORw..."
    Hex,      // "89504e47..."
    LenOnly,  // "<42 bytes>"
}
```

### U64Format

```rust
pub enum U64Format {
    String,   // "9007199254740992" (safe for JS)
    Number,   // 9007199254740992 (may lose precision)
}
```

### EnumRender

```rust
pub enum EnumRender {
    Label,     // "user"
    Number,    // 2
    Both,      // {"value": 2, "label": "user"}
}
```

### TimeRender

```rust
pub enum TimeRender {
    Iso,       // "2025-01-30T10:00:00.000Z"
    UnixMs,    // 1706615000000
}
```

## Examples

### Basic Projection

**Msgpack:** `{1: "user", 2: "Hello"}`

**Descriptor:**

```json
{
  "fields": {
    "1": {"name": "role", "type": "string"},
    "2": {"name": "text", "type": "string"}
  }
}
```

**Projected JSON:**

```json
{
  "role": "user",
  "text": "Hello"
}
```

### With Unknown Fields

**Msgpack:** `{1: "user", 2: "Hello", 99: 42}`

**Descriptor:** (only tags 1 and 2)

**Projected JSON** (`include_unknown=true`):

```json
{
  "role": "user",
  "text": "Hello",
  "unknown": {
    "99": 42
  }
}
```

### Timestamp Rendering

**Msgpack:** `{1: "user", 3: 1706615000000}`

**Descriptor:**

```json
{
  "fields": {
    "1": {"name": "role", "type": "string"},
    "3": {"name": "timestamp", "type": "u64", "semantic": "unix_ms"}
  }
}
```

**Projected JSON** (`time_render=iso`):

```json
{
  "role": "user",
  "timestamp": "2025-01-30T10:10:00.000Z"
}
```

### Enum Rendering

**Msgpack:** `{1: 2}`

**Descriptor:**

```json
{
  "fields": {
    "1": {"name": "role", "type": "u8", "enum": "com.example.Role"}
  }
}
```

**Enum definition:**

```json
{
  "com.example.Role": {
    "1": "system",
    "2": "user",
    "3": "assistant"
  }
}
```

**Projected JSON** (`enum_render=label`):

```json
{
  "role": "user"
}
```

## Nested Types

**Msgpack:**

```
{
  1: "user",
  2: "Call weather",
  3: {
    1: "get_weather",
    2: {"location": "Paris"}
  }
}
```

**Descriptor:**

```json
{
  "fields": {
    "1": {"name": "role", "type": "string"},
    "2": {"name": "text", "type": "string"},
    "3": {"name": "tool_call", "type": "nested", "nested": "com.example.ToolCall"}
  }
}
```

**Projected JSON:**

```json
{
  "role": "user",
  "text": "Call weather",
  "tool_call": {
    "name": "get_weather",
    "arguments": {"location": "Paris"}
  }
}
```

## Performance

**Typical latencies** (10KB msgpack payload):
- Decode msgpack: ~0.1ms
- Normalize keys: ~0.05ms
- Project fields: ~0.1ms
- Render options: ~0.05ms
- **Total: ~0.3ms**

## Error Handling

```rust
pub enum ProjectionError {
    MsgpackDecode(rmp_serde::decode::Error),
    DescriptorNotFound { type_id: String, version: u32 },
    TypeMismatch { field: String, expected: String, actual: String },
    InvalidEnum { value: u64, enum_id: String },
}
```

## Testing

```bash
# Run projection tests
cargo test --package ai-cxdb-store --lib projection

# Test type coercions
cargo test test_u64_to_string

# Test enum rendering
cargo test test_enum_projection
```

## See Also

- [Type Registry](../registry/README.md) - Descriptor management
- [HTTP Module](../http/README.md) - API integration
- [Type Registry Spec](../../docs/type-registry.md) - Schema design

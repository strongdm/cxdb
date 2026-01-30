# Registry Module

Type registry for forward-compatible schema evolution and typed projections.

## Overview

The registry module manages type descriptors that map msgpack numeric tags to typed JSON fields. It validates schema evolution rules and provides descriptors to the projection module.

## Data Model

### Registry Bundle

```rust
pub struct RegistryBundle {
    pub registry_version: u32,
    pub bundle_id: String,
    pub types: HashMap<TypeId, TypeDefinition>,
    pub enums: HashMap<EnumId, EnumDefinition>,
}

pub struct TypeDefinition {
    pub versions: HashMap<TypeVersion, VersionDescriptor>,
}

pub struct VersionDescriptor {
    pub fields: HashMap<FieldTag, FieldDescriptor>,
}

pub struct FieldDescriptor {
    pub name: String,
    pub field_type: FieldType,
    pub optional: bool,
    pub semantic: Option<String>,
    pub enum_ref: Option<EnumId>,
}
```

### Field Types

```rust
pub enum FieldType {
    Bool,
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    F32, F64,
    String,
    Bytes,
    Array { items: Box<FieldType> },
    Map { key: Box<FieldType>, value: Box<FieldType> },
    Nested { type_id: TypeId },
}
```

## API

### Publishing a Bundle

```rust
use registry::Registry;

let mut registry = Registry::new(Path::new("./data/registry"))?;

let bundle = RegistryBundle {
    registry_version: 1,
    bundle_id: "2025-01-30T10:00:00Z".to_string(),
    types: /* ... */,
    enums: /* ... */,
};

registry.put_bundle(bundle)?;
```

### Loading a Descriptor

```rust
let descriptor = registry.get_descriptor("com.example.Message", 1)?;

for (tag, field) in &descriptor.fields {
    println!("Tag {}: {} ({})", tag, field.name, field.field_type);
}
```

### Listing Types

```rust
let types = registry.list_types()?;

for (type_id, latest_version) in types {
    println!("{} v{}", type_id, latest_version);
}
```

## Evolution Rules

The registry enforces these rules:

1. **Never reuse tags:** Once a tag is assigned, it cannot be reassigned
2. **Monotonic versions:** Version numbers must increase
3. **Additive changes:** Can add new fields, cannot change existing field types
4. **Optional new fields:** New fields should be marked optional

### Validation

```rust
impl Registry {
    fn validate_evolution(&self, bundle: &RegistryBundle) -> Result<()> {
        for (type_id, type_def) in &bundle.types {
            for (version, descriptor) in &type_def.versions {
                // Check for tag reuse
                self.check_tag_uniqueness(type_id, version, descriptor)?;

                // Check version monotonicity
                self.check_version_order(type_id, version)?;

                // Check backward compatibility
                self.check_compatibility(type_id, version, descriptor)?;
            }
        }
        Ok(())
    }
}
```

## Storage

### Bundle Files

```
registry/
├── bundles/
│   ├── 2025-01-30T10:00:00Z.json
│   └── 2025-01-30T11:00:00Z.json
└── index.json
```

**index.json:**

```json
{
  "types": {
    "com.example.Message": {
      "latest_version": 2,
      "bundle_id": "2025-01-30T11:00:00Z"
    }
  }
}
```

## Caching

Descriptors are cached in memory:

```rust
struct Registry {
    cache: Arc<RwLock<HashMap<(TypeId, TypeVersion), VersionDescriptor>>>,
    // ...
}

impl Registry {
    fn get_descriptor(&self, type_id: &str, version: u32) -> Result<VersionDescriptor> {
        // Check cache
        {
            let cache = self.cache.read().unwrap();
            if let Some(desc) = cache.get(&(type_id.to_string(), version)) {
                return Ok(desc.clone());
            }
        }

        // Load from disk
        let desc = self.load_from_disk(type_id, version)?;

        // Update cache
        {
            let mut cache = self.cache.write().unwrap();
            cache.insert((type_id.to_string(), version), desc.clone());
        }

        Ok(desc)
    }
}
```

## Testing

```bash
# Run registry tests
cargo test --package ai-cxdb-store --lib registry

# Test evolution validation
cargo test test_registry_evolution

# Test tag reuse detection
cargo test test_tag_reuse_rejected
```

## See Also

- [Type Registry Spec](../../docs/type-registry.md) - Schema evolution guide
- [Projection Module](../projection/README.md) - Using descriptors for projection
- [CLIENT_SPEC.md](../../../CLIENT_SPEC.md) - Client-side type management

// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use cxdb_server::projection::{project_msgpack, project_turn_page, serialize_turn_page};
use cxdb_server::projection::{BytesRender, EnumRender, RenderOptions, TimeRender, U64Format};
use cxdb_server::registry::Registry;
use cxdb_server::store::TurnWithMeta;
use cxdb_server::turn_store::{TurnMeta, TurnRecord};
use rmpv::Value;
use tempfile::tempdir;

fn default_options() -> RenderOptions {
    RenderOptions {
        bytes_render: BytesRender::Base64,
        u64_format: U64Format::String,
        enum_render: EnumRender::Label,
        time_render: TimeRender::Iso,
        include_unknown: true,
        string_limit: None,
    }
}

#[test]
fn registry_ingest_and_project() {
    let dir = tempdir().expect("tempdir");
    let mut registry = Registry::open(dir.path()).expect("open registry");

    let bundle = r#"
    {
      "registry_version": 1,
      "bundle_id": "2025-12-19T00:00:00Z#test",
      "types": {
        "com.example.Message": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "role", "type": "u8", "enum": "com.example.Role" },
                "2": { "name": "text", "type": "string" }
              }
            }
          }
        }
      },
      "enums": {
        "com.example.Role": { "1": "system", "2": "user" }
      }
    }
    "#;

    registry
        .put_bundle("2025-12-19T00:00:00Z#test", bundle.as_bytes())
        .expect("put bundle");

    let desc = registry
        .get_type_version("com.example.Message", 1)
        .expect("descriptor");

    let map = vec![
        (Value::Integer(1.into()), Value::Integer(2.into())),
        (Value::Integer(2.into()), Value::String("hello".into())),
        (Value::Integer(9.into()), Value::Integer(42.into())),
    ];
    let value = Value::Map(map);

    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &value).expect("encode msgpack");

    let options = RenderOptions {
        bytes_render: BytesRender::Base64,
        u64_format: U64Format::String,
        enum_render: EnumRender::Label,
        time_render: TimeRender::Iso,
        include_unknown: true,
        string_limit: None,
    };

    let projection = project_msgpack(&buf, desc, &registry, &options).expect("project");
    let data = projection.data.as_object().expect("data object");
    assert_eq!(data.get("role").unwrap().as_str().unwrap(), "user");
    assert_eq!(data.get("text").unwrap().as_str().unwrap(), "hello");

    let unknown = projection.unknown.expect("unknown");
    let unknown_obj = unknown.as_object().expect("unknown object");
    assert!(unknown_obj.contains_key("9"));
}

#[test]
fn nested_type_references() {
    let dir = tempdir().expect("tempdir");
    let mut registry = Registry::open(dir.path()).expect("open registry");

    // Bundle with nested type references
    let bundle = r#"
    {
      "registry_version": 1,
      "bundle_id": "nested-test",
      "types": {
        "test:Item": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "item_type", "type": "string" },
                "2": { "name": "nested", "type": "ref", "ref": "test:Nested" },
                "3": { "name": "items", "type": "array", "items": { "type": "ref", "ref": "test:ArrayItem" } }
              }
            }
          }
        },
        "test:Nested": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "name", "type": "string" },
                "2": { "name": "value", "type": "int64" }
              }
            }
          }
        },
        "test:ArrayItem": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "id", "type": "string" },
                "2": { "name": "count", "type": "int32" }
              }
            }
          }
        }
      },
      "enums": {}
    }
    "#;

    registry
        .put_bundle("nested-test", bundle.as_bytes())
        .expect("put bundle");
    let desc = registry
        .get_type_version("test:Item", 1)
        .expect("descriptor");

    // Build msgpack with nested structures using numeric tags
    // Item { item_type: "foo", nested: { name: "bar", value: 42 }, items: [{ id: "x", count: 1 }] }
    let nested_map = vec![
        (Value::Integer(1.into()), Value::String("bar".into())),
        (Value::Integer(2.into()), Value::Integer(42.into())),
    ];
    let array_item = vec![
        (Value::Integer(1.into()), Value::String("x".into())),
        (Value::Integer(2.into()), Value::Integer(1.into())),
    ];
    let root_map = vec![
        (Value::Integer(1.into()), Value::String("foo".into())),
        (Value::Integer(2.into()), Value::Map(nested_map)),
        (
            Value::Integer(3.into()),
            Value::Array(vec![Value::Map(array_item)]),
        ),
    ];
    let value = Value::Map(root_map);

    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &value).expect("encode msgpack");

    let projection = project_msgpack(&buf, desc, &registry, &default_options()).expect("project");
    let data = projection.data.as_object().expect("data object");

    // Check top-level field
    assert_eq!(data.get("item_type").unwrap().as_str().unwrap(), "foo");

    // Check nested type was projected correctly (not raw numeric keys)
    let nested = data
        .get("nested")
        .unwrap()
        .as_object()
        .expect("nested object");
    assert_eq!(nested.get("name").unwrap().as_str().unwrap(), "bar");
    assert_eq!(nested.get("value").unwrap().as_str().unwrap(), "42"); // u64 formatted as string

    // Check array items were projected correctly
    let items = data.get("items").unwrap().as_array().expect("items array");
    assert_eq!(items.len(), 1);
    let first_item = items[0].as_object().expect("first item");
    assert_eq!(first_item.get("id").unwrap().as_str().unwrap(), "x");
    assert_eq!(first_item.get("count").unwrap().as_i64().unwrap(), 1);
}

#[test]
fn bundle_with_renderer_parses() {
    let dir = tempdir().expect("tempdir");
    let mut registry = Registry::open(dir.path()).expect("open registry");

    // Bundle with renderer specification
    let bundle = r#"
    {
      "registry_version": 1,
      "bundle_id": "renderer-test",
      "types": {
        "test:Message": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "text", "type": "string" }
              },
              "renderer": {
                "esm_url": "builtin:MessageRenderer",
                "component": "MessageRendererWrapper",
                "integrity": "sha384-abc123"
              }
            }
          }
        }
      },
      "enums": {}
    }
    "#;

    registry
        .put_bundle("renderer-test", bundle.as_bytes())
        .expect("put bundle");

    // Verify the renderer was parsed and preserved
    let spec = registry
        .get_type_version("test:Message", 1)
        .expect("type version");

    let renderer = spec.renderer.as_ref().expect("renderer should exist");
    assert_eq!(renderer.esm_url, "builtin:MessageRenderer");
    assert_eq!(
        renderer.component.as_ref().unwrap(),
        "MessageRendererWrapper"
    );
    assert_eq!(renderer.integrity.as_ref().unwrap(), "sha384-abc123");
}

#[test]
fn bundle_without_renderer_backward_compat() {
    let dir = tempdir().expect("tempdir");
    let mut registry = Registry::open(dir.path()).expect("open registry");

    // Bundle without renderer (old format)
    let bundle = r#"
    {
      "registry_version": 1,
      "bundle_id": "no-renderer-test",
      "types": {
        "test:OldType": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "value", "type": "int32" }
              }
            }
          }
        }
      },
      "enums": {}
    }
    "#;

    registry
        .put_bundle("no-renderer-test", bundle.as_bytes())
        .expect("put bundle");

    // Verify the type was ingested correctly without renderer
    let spec = registry
        .get_type_version("test:OldType", 1)
        .expect("type version");

    assert!(
        spec.renderer.is_none(),
        "renderer should be None for old bundles"
    );
    assert!(spec.fields.contains_key(&1));
}

#[test]
fn get_all_renderers() {
    let dir = tempdir().expect("tempdir");
    let mut registry = Registry::open(dir.path()).expect("open registry");

    // Bundle with multiple types, some with renderers
    let bundle = r#"
    {
      "registry_version": 1,
      "bundle_id": "multi-renderer-test",
      "types": {
        "test:TypeA": {
          "versions": {
            "1": {
              "fields": { "1": { "name": "a", "type": "string" } },
              "renderer": { "esm_url": "builtin:RendererA" }
            }
          }
        },
        "test:TypeB": {
          "versions": {
            "1": {
              "fields": { "1": { "name": "b", "type": "string" } }
            }
          }
        },
        "test:TypeC": {
          "versions": {
            "1": {
              "fields": { "1": { "name": "c1", "type": "string" } }
            },
            "2": {
              "fields": { "1": { "name": "c2", "type": "string" } },
              "renderer": { "esm_url": "builtin:RendererC", "component": "CWrapper" }
            }
          }
        }
      },
      "enums": {}
    }
    "#;

    registry
        .put_bundle("multi-renderer-test", bundle.as_bytes())
        .expect("put bundle");

    let renderers = registry.get_all_renderers();

    // TypeA has a renderer
    assert!(renderers.contains_key("test:TypeA"));
    assert_eq!(
        renderers.get("test:TypeA").unwrap().esm_url,
        "builtin:RendererA"
    );

    // TypeB has no renderer
    assert!(!renderers.contains_key("test:TypeB"));

    // TypeC uses latest version (v2) which has a renderer
    assert!(renderers.contains_key("test:TypeC"));
    let c_renderer = renderers.get("test:TypeC").unwrap();
    assert_eq!(c_renderer.esm_url, "builtin:RendererC");
    assert_eq!(c_renderer.component.as_ref().unwrap(), "CWrapper");
}

#[test]
fn map_with_ref_recursively_projects() {
    // Regression test: a bundle schema may use `"type": "map"` with a separate
    // `"ref"` attribute for nested types.  The projection engine must treat
    // this the same as `"type": "ref"` and recursively decode nested fields
    // to named keys.
    let dir = tempdir().expect("tempdir");
    let mut registry = Registry::open(dir.path()).expect("open registry");

    let bundle = r#"
    {
      "registry_version": 1,
      "bundle_id": "map-ref-test",
      "types": {
        "test:Outer": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "item_type", "type": "string" },
                "13": { "name": "handoff", "type": "map", "ref": "test:Inner", "optional": true }
              }
            }
          }
        },
        "test:Inner": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "from_agent", "type": "string" },
                "2": { "name": "to_agent", "type": "string" },
                "5": { "name": "reason", "type": "string" }
              }
            }
          }
        }
      },
      "enums": {}
    }
    "#;

    registry
        .put_bundle("map-ref-test", bundle.as_bytes())
        .expect("put bundle");
    let desc = registry
        .get_type_version("test:Outer", 1)
        .expect("descriptor");

    // Build msgpack: Outer { item_type: "handoff", handoff: Inner { from_agent: "root", to_agent: "explorer", reason: "delegation" } }
    let inner_map = vec![
        (Value::Integer(1.into()), Value::String("root".into())),
        (Value::Integer(2.into()), Value::String("explorer".into())),
        (Value::Integer(5.into()), Value::String("delegation".into())),
    ];
    let root_map = vec![
        (Value::Integer(1.into()), Value::String("handoff".into())),
        (Value::Integer(13.into()), Value::Map(inner_map)),
    ];
    let value = Value::Map(root_map);

    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &value).expect("encode msgpack");

    let projection = project_msgpack(&buf, desc, &registry, &default_options()).expect("project");
    let data = projection.data.as_object().expect("data object");

    // Top-level field decoded
    assert_eq!(data.get("item_type").unwrap().as_str().unwrap(), "handoff");

    // Nested map+ref field MUST have named keys, not numeric string keys
    let handoff = data
        .get("handoff")
        .expect("handoff field present")
        .as_object()
        .expect("handoff is object");
    assert_eq!(handoff.get("from_agent").unwrap().as_str().unwrap(), "root");
    assert_eq!(
        handoff.get("to_agent").unwrap().as_str().unwrap(),
        "explorer"
    );
    assert_eq!(
        handoff.get("reason").unwrap().as_str().unwrap(),
        "delegation"
    );

    // Verify numeric keys are NOT present (regression guard)
    assert!(
        handoff.get("1").is_none(),
        "numeric key '1' should not appear in typed view"
    );
    assert!(
        handoff.get("2").is_none(),
        "numeric key '2' should not appear in typed view"
    );
}

#[test]
fn array_shorthand_ref_recursively_projects() {
    // Regression test: a bundle schema may use `"items": { "ref": "T" }`
    // (without `"type": "ref"`) for array items.  The registry parser must
    // treat this shorthand the same as the long form `{"type":"ref","ref":"T"}`.
    let dir = tempdir().expect("tempdir");
    let mut registry = Registry::open(dir.path()).expect("open registry");

    let bundle = r#"
    {
      "registry_version": 1,
      "bundle_id": "shorthand-ref-test",
      "types": {
        "test:Parent": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "label", "type": "string" },
                "2": { "name": "children", "type": "array", "items": { "ref": "test:Child" }, "optional": true }
              }
            }
          }
        },
        "test:Child": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "name", "type": "string" },
                "2": { "name": "score", "type": "int32" }
              }
            }
          }
        }
      },
      "enums": {}
    }
    "#;

    registry
        .put_bundle("shorthand-ref-test", bundle.as_bytes())
        .expect("put bundle");
    let desc = registry
        .get_type_version("test:Parent", 1)
        .expect("descriptor");

    // Build msgpack: Parent { label: "grp", children: [Child { name: "a", score: 10 }] }
    let child_map = vec![
        (Value::Integer(1.into()), Value::String("a".into())),
        (Value::Integer(2.into()), Value::Integer(10.into())),
    ];
    let root_map = vec![
        (Value::Integer(1.into()), Value::String("grp".into())),
        (
            Value::Integer(2.into()),
            Value::Array(vec![Value::Map(child_map)]),
        ),
    ];
    let value = Value::Map(root_map);

    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &value).expect("encode msgpack");

    let projection = project_msgpack(&buf, desc, &registry, &default_options()).expect("project");
    let data = projection.data.as_object().expect("data object");

    assert_eq!(data.get("label").unwrap().as_str().unwrap(), "grp");

    let children = data
        .get("children")
        .unwrap()
        .as_array()
        .expect("children array");
    assert_eq!(children.len(), 1);

    let child = children[0].as_object().expect("child object");
    // Must have named keys, not numeric string keys
    assert_eq!(child.get("name").unwrap().as_str().unwrap(), "a");
    assert_eq!(child.get("score").unwrap().as_i64().unwrap(), 10);
    assert!(
        child.get("1").is_none(),
        "numeric key '1' should not appear in shorthand ref array items"
    );
}

// ---------------------------------------------------------------------------
// Tests for include_unknown propagation into nested types
// ---------------------------------------------------------------------------

/// Helper: build a registry with a parent type containing a nested ref and an
/// array of refs, so we can test unknown-tag propagation at every nesting level.
fn build_nested_registry_with_unknown() -> (tempfile::TempDir, cxdb_server::registry::Registry) {
    let dir = tempdir().expect("tempdir");
    let mut registry = Registry::open(dir.path()).expect("open registry");

    let bundle = r#"
    {
      "registry_version": 1,
      "bundle_id": "unknown-nested-test",
      "types": {
        "test:Root": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "label", "type": "string" },
                "2": { "name": "nested", "type": "ref", "ref": "test:Nested" },
                "3": { "name": "items", "type": "array", "items": { "ref": "test:ArrayItem" } }
              }
            }
          }
        },
        "test:Nested": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "name", "type": "string" }
              }
            }
          }
        },
        "test:ArrayItem": {
          "versions": {
            "1": {
              "fields": {
                "1": { "name": "id", "type": "string" }
              }
            }
          }
        }
      },
      "enums": {}
    }
    "#;

    registry
        .put_bundle("unknown-nested-test", bundle.as_bytes())
        .expect("put bundle");
    (dir, registry)
}

#[test]
fn nested_ref_includes_unknown_tags() {
    let (_dir, registry) = build_nested_registry_with_unknown();
    let desc = registry
        .get_type_version("test:Root", 1)
        .expect("descriptor");

    // Build msgpack: Root {
    //   label: "hello",
    //   nested: { 1: "known", 99: "extra_nested" },   <-- tag 99 is unknown
    //   items: []
    // }
    let nested_map = vec![
        (Value::Integer(1.into()), Value::String("known".into())),
        (
            Value::Integer(99.into()),
            Value::String("extra_nested".into()),
        ),
    ];
    let root_map = vec![
        (Value::Integer(1.into()), Value::String("hello".into())),
        (Value::Integer(2.into()), Value::Map(nested_map)),
        (Value::Integer(3.into()), Value::Array(vec![])),
    ];
    let value = Value::Map(root_map);

    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &value).expect("encode msgpack");

    // With include_unknown = true
    let options = RenderOptions {
        include_unknown: true,
        ..default_options()
    };
    let projection = project_msgpack(&buf, desc, &registry, &options).expect("project");
    let data = projection.data.as_object().expect("data object");

    // Known field is present
    let nested = data
        .get("nested")
        .unwrap()
        .as_object()
        .expect("nested object");
    assert_eq!(nested.get("name").unwrap().as_str().unwrap(), "known");

    // Unknown tag 99 must appear in _unknown
    let nested_unknown = nested
        .get("_unknown")
        .expect("nested _unknown must be present when include_unknown=true")
        .as_object()
        .expect("_unknown is object");
    assert_eq!(
        nested_unknown.get("99").unwrap().as_str().unwrap(),
        "extra_nested"
    );
}

#[test]
fn nested_ref_omits_unknown_when_disabled() {
    let (_dir, registry) = build_nested_registry_with_unknown();
    let desc = registry
        .get_type_version("test:Root", 1)
        .expect("descriptor");

    // Same payload as above — tag 99 is unknown on the nested ref
    let nested_map = vec![
        (Value::Integer(1.into()), Value::String("known".into())),
        (
            Value::Integer(99.into()),
            Value::String("extra_nested".into()),
        ),
    ];
    let root_map = vec![
        (Value::Integer(1.into()), Value::String("hello".into())),
        (Value::Integer(2.into()), Value::Map(nested_map)),
        (Value::Integer(3.into()), Value::Array(vec![])),
    ];
    let value = Value::Map(root_map);

    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &value).expect("encode msgpack");

    // With include_unknown = false (default behaviour)
    let options = RenderOptions {
        include_unknown: false,
        ..default_options()
    };
    let projection = project_msgpack(&buf, desc, &registry, &options).expect("project");
    let data = projection.data.as_object().expect("data object");

    let nested = data
        .get("nested")
        .unwrap()
        .as_object()
        .expect("nested object");

    // _unknown must NOT be present
    assert!(
        nested.get("_unknown").is_none(),
        "_unknown should not appear when include_unknown is false"
    );
}

#[test]
fn array_ref_items_include_unknown_tags() {
    let (_dir, registry) = build_nested_registry_with_unknown();
    let desc = registry
        .get_type_version("test:Root", 1)
        .expect("descriptor");

    // Build msgpack: Root {
    //   label: "hello",
    //   nested: { 1: "n" },
    //   items: [
    //     { 1: "item_a", 50: 123 },   <-- tag 50 is unknown
    //     { 1: "item_b" }              <-- no unknowns
    //   ]
    // }
    let nested_map = vec![(Value::Integer(1.into()), Value::String("n".into()))];
    let item_a = vec![
        (Value::Integer(1.into()), Value::String("item_a".into())),
        (Value::Integer(50.into()), Value::Integer(123.into())),
    ];
    let item_b = vec![(Value::Integer(1.into()), Value::String("item_b".into()))];
    let root_map = vec![
        (Value::Integer(1.into()), Value::String("hello".into())),
        (Value::Integer(2.into()), Value::Map(nested_map)),
        (
            Value::Integer(3.into()),
            Value::Array(vec![Value::Map(item_a), Value::Map(item_b)]),
        ),
    ];
    let value = Value::Map(root_map);

    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &value).expect("encode msgpack");

    let options = RenderOptions {
        include_unknown: true,
        ..default_options()
    };
    let projection = project_msgpack(&buf, desc, &registry, &options).expect("project");
    let data = projection.data.as_object().expect("data object");

    let items = data.get("items").unwrap().as_array().expect("items array");
    assert_eq!(items.len(), 2);

    // First item has unknown tag 50
    let first = items[0].as_object().expect("first item");
    assert_eq!(first.get("id").unwrap().as_str().unwrap(), "item_a");
    let first_unknown = first
        .get("_unknown")
        .expect("first item _unknown must be present")
        .as_object()
        .expect("_unknown is object");
    assert!(first_unknown.contains_key("50"));

    // Second item has NO unknown tags — _unknown should be absent
    let second = items[1].as_object().expect("second item");
    assert_eq!(second.get("id").unwrap().as_str().unwrap(), "item_b");
    assert!(
        second.get("_unknown").is_none(),
        "_unknown should not appear when there are no unknown tags"
    );
}

#[test]
fn named_keys_are_recursive_numeric_priority_and_utf8_bounded() {
    let dir = tempdir().expect("tempdir");
    let mut registry = Registry::open(dir.path()).expect("open registry");
    let bundle = r#"{
      "registry_version": 1, "bundle_id": "named#test",
      "types": {"test:Message": {"versions": {"1": {"fields": {
        "1": {"name": "text", "type": "string"},
        "2": {"name": "count", "type": "u64"}
      }}}}}
    }"#;
    registry
        .put_bundle("named#test", bundle.as_bytes())
        .expect("bundle");
    let desc = registry.get_type_version("test:Message", 1).expect("desc");
    let value = Value::Map(vec![
        (Value::String("text".into()), Value::String("named".into())),
        (Value::Integer(1.into()), Value::String("numeric".into())),
        (Value::String("1".into()), Value::String("alias".into())),
        (
            Value::String("extra".into()),
            Value::String("éclair".into()),
        ),
        (Value::String("count".into()), Value::Integer(7.into())),
    ]);
    let mut payload = Vec::new();
    rmpv::encode::write_value(&mut payload, &value).expect("encode");

    let bounded = RenderOptions {
        string_limit: Some(4),
        ..default_options()
    };
    let projected = project_msgpack(&payload, desc, &registry, &bounded).expect("project");
    assert_eq!(projected.data["text"], "nume");
    assert_eq!(projected.data["count"], "7");
    assert_eq!(projected.unknown.as_ref().unwrap()["extra"], "écl");

    let record = TurnRecord {
        turn_id: 9,
        parent_turn_id: 0,
        depth: 0,
        codec: 1,
        type_tag: 0,
        payload_hash: *blake3::hash(&payload).as_bytes(),
        flags: 0,
        created_at_unix_ms: 0,
    };
    let item = TurnWithMeta {
        record,
        meta: TurnMeta {
            declared_type_id: "test:Message".into(),
            declared_type_version: 1,
            encoding: 1,
            compression: 0,
            uncompressed_len: payload.len() as u32,
        },
        payload: Some(payload),
    };
    let numeric = RenderOptions {
        u64_format: U64Format::Number,
        ..bounded.clone()
    };
    let options = cxdb_server::projection::TurnProjectionOptions {
        view: "typed",
        type_hint_mode: "inherit",
        as_type_id: None,
        as_type_version: None,
        render: &numeric,
    };
    let normal =
        project_turn_page(std::slice::from_ref(&item), &registry, &options).expect("normal");
    let fast = serialize_turn_page(std::slice::from_ref(&item), &registry, &options).expect("fast");
    let fast_json: serde_json::Value = serde_json::from_slice(&fast[0]).expect("fast json");
    assert_eq!(fast_json, normal[0]);
    assert_eq!(fast_json["turn_id"], 9);
}

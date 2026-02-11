// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use base64::Engine;
use chrono::{DateTime, Utc};
use rmpv::Value;
use serde_json::{Map, Number, Value as JsonValue};

use crate::error::{Result, StoreError};
use crate::registry::{ItemsSpec, Registry, TypeVersionSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytesRender {
    Base64,
    Hex,
    LenOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum U64Format {
    String,
    Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumRender {
    Label,
    Number,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRender {
    Iso,
    UnixMs,
}

#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub bytes_render: BytesRender,
    pub u64_format: U64Format,
    pub enum_render: EnumRender,
    pub time_render: TimeRender,
    pub include_unknown: bool,
}

pub struct ProjectionResult {
    pub data: JsonValue,
    pub unknown: Option<JsonValue>,
}

pub fn project_msgpack(
    payload: &[u8],
    descriptor: &TypeVersionSpec,
    registry: &Registry,
    options: &RenderOptions,
) -> Result<ProjectionResult> {
    let mut cursor = std::io::Cursor::new(payload);
    let value = rmpv::decode::read_value(&mut cursor)
        .map_err(|e| StoreError::InvalidInput(format!("msgpack decode error: {e}")))?;

    let map = normalize_tags(&value)?;
    let mut data = Map::new();
    let mut unknown = Map::new();

    for (tag, field) in descriptor.fields.iter() {
        if let Some(val) = map.get(tag) {
            let rendered = render_field_value(val, field, registry, options);
            data.insert(field.name.clone(), rendered);
        }
    }

    if options.include_unknown {
        for (tag, val) in map.iter() {
            if descriptor.fields.contains_key(tag) {
                continue;
            }
            unknown.insert(tag.to_string(), render_value(val, options));
        }
    }

    Ok(ProjectionResult {
        data: JsonValue::Object(data),
        unknown: if options.include_unknown {
            Some(JsonValue::Object(unknown))
        } else {
            None
        },
    })
}

fn normalize_tags(value: &Value) -> Result<HashMap<u64, Value>> {
    let mut out = HashMap::new();
    let map = match value {
        Value::Map(map) => map,
        _ => return Err(StoreError::InvalidInput("payload is not a map".into())),
    };

    for (k, v) in map.iter() {
        if let Some(tag) = key_to_tag(k) {
            out.insert(tag, v.clone());
        }
    }

    Ok(out)
}

fn key_to_tag(key: &Value) -> Option<u64> {
    match key {
        Value::Integer(int) => int.as_u64().or_else(|| {
            int.as_i64()
                .and_then(|v| if v >= 0 { Some(v as u64) } else { None })
        }),
        Value::String(s) => s.as_str()?.parse::<u64>().ok(),
        _ => None,
    }
}

fn render_field_value(
    value: &Value,
    field: &crate::registry::FieldSpec,
    registry: &Registry,
    options: &RenderOptions,
) -> JsonValue {
    if let Some(enum_ref) = &field.enum_ref {
        if let Some(num) = value_to_u64(value) {
            if let Some(map) = registry.get_enum(enum_ref) {
                if let Some(label) = map.get(&num.to_string()) {
                    return match options.enum_render {
                        EnumRender::Label => JsonValue::String(label.clone()),
                        EnumRender::Number => JsonValue::Number(Number::from(num)),
                        EnumRender::Both => {
                            let mut obj = Map::new();
                            obj.insert("label".into(), JsonValue::String(label.clone()));
                            obj.insert("value".into(), JsonValue::Number(Number::from(num)));
                            JsonValue::Object(obj)
                        }
                    };
                }
            }
        }
    }

    // Handle type references - recursively project using the referenced type
    // Also recurse for "map" fields with a type_ref (e.g., metrics: map with ref to TurnMetrics)
    if field.type_ref.is_some() && (field.field_type == "ref" || field.field_type == "map") {
        if let Some(type_ref) = &field.type_ref {
            return render_type_ref(value, type_ref, registry, options);
        }
    }

    let field_type = field.field_type.as_str();
    match field_type {
        "u64" | "uint64" | "i64" | "int64" => render_u64(value, options),
        "u32" | "uint32" | "u8" | "uint8" | "int32" => render_int(value),
        "string" => render_string(value),
        "bool" => render_bool(value),
        "bytes" | "typed_blob" => render_bytes(value, options),
        "array" => render_array(value, field.items.as_ref(), registry, options),
        "unix_ms" | "time_ms" | "timestamp_ms" => render_time(value, options),
        _ => render_value(value, options),
    }
}

/// Recursively project a value using a referenced type's descriptor
fn render_type_ref(
    value: &Value,
    type_ref: &str,
    registry: &Registry,
    options: &RenderOptions,
) -> JsonValue {
    // Get the latest version of the referenced type
    let Some(type_spec) = registry.get_latest_type_version(type_ref) else {
        // Fall back to raw rendering if type not found
        return render_value(value, options);
    };

    // Normalize the value to a tag map
    let Ok(map) = normalize_tags(value) else {
        return render_value(value, options);
    };

    // Project using the type descriptor
    let mut data = Map::new();
    for (tag, field) in type_spec.fields.iter() {
        if let Some(val) = map.get(tag) {
            let rendered = render_field_value(val, field, registry, options);
            data.insert(field.name.clone(), rendered);
        }
    }

    JsonValue::Object(data)
}

fn render_value(value: &Value, options: &RenderOptions) -> JsonValue {
    match value {
        Value::Nil => JsonValue::Null,
        Value::Boolean(b) => JsonValue::Bool(*b),
        Value::Integer(int) => {
            if let Some(u) = int.as_u64() {
                render_u64_raw(u, options)
            } else if let Some(i) = int.as_i64() {
                JsonValue::Number(Number::from(i))
            } else {
                JsonValue::Null
            }
        }
        Value::F32(f) => JsonValue::Number(Number::from_f64(*f as f64).unwrap_or(Number::from(0))),
        Value::F64(f) => JsonValue::Number(Number::from_f64(*f).unwrap_or(Number::from(0))),
        Value::String(s) => JsonValue::String(s.as_str().unwrap_or("").to_string()),
        Value::Binary(_) => render_bytes(value, options),
        Value::Array(arr) => {
            let items = arr.iter().map(|v| render_value(v, options)).collect();
            JsonValue::Array(items)
        }
        Value::Map(map) => {
            let mut obj = Map::new();
            for (k, v) in map.iter() {
                let key = match k {
                    Value::String(s) => s.as_str().unwrap_or("").to_string(),
                    Value::Integer(int) => int
                        .as_u64()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "".into()),
                    _ => "".into(),
                };
                obj.insert(key, render_value(v, options));
            }
            JsonValue::Object(obj)
        }
        _ => JsonValue::Null,
    }
}

fn render_string(value: &Value) -> JsonValue {
    match value {
        Value::String(s) => JsonValue::String(s.as_str().unwrap_or("").to_string()),
        _ => JsonValue::Null,
    }
}

fn render_bool(value: &Value) -> JsonValue {
    match value {
        Value::Boolean(b) => JsonValue::Bool(*b),
        _ => JsonValue::Null,
    }
}

fn render_int(value: &Value) -> JsonValue {
    match value_to_i64(value) {
        Some(i) => JsonValue::Number(Number::from(i)),
        None => JsonValue::Null,
    }
}

fn render_u64(value: &Value, options: &RenderOptions) -> JsonValue {
    match value_to_u64(value) {
        Some(u) => render_u64_raw(u, options),
        None => JsonValue::Null,
    }
}

fn render_u64_raw(u: u64, options: &RenderOptions) -> JsonValue {
    match options.u64_format {
        U64Format::String => JsonValue::String(u.to_string()),
        U64Format::Number => JsonValue::Number(Number::from(u)),
    }
}

fn render_bytes(value: &Value, options: &RenderOptions) -> JsonValue {
    let bytes = match value {
        Value::Binary(b) => b,
        _ => return JsonValue::Null,
    };

    match options.bytes_render {
        BytesRender::Base64 => {
            JsonValue::String(base64::engine::general_purpose::STANDARD.encode(bytes))
        }
        BytesRender::Hex => JsonValue::String(hex::encode(bytes)),
        BytesRender::LenOnly => JsonValue::Number(Number::from(bytes.len() as u64)),
    }
}

fn render_array(
    value: &Value,
    items_spec: Option<&ItemsSpec>,
    registry: &Registry,
    options: &RenderOptions,
) -> JsonValue {
    let arr = match value {
        Value::Array(arr) => arr,
        _ => return JsonValue::Null,
    };

    let mut out = Vec::with_capacity(arr.len());
    for item in arr.iter() {
        let rendered = match items_spec {
            Some(ItemsSpec::Simple(item_type)) => {
                let dummy_field = crate::registry::FieldSpec {
                    name: "".into(),
                    field_type: item_type.clone(),
                    enum_ref: None,
                    type_ref: None,
                    optional: false,
                    items: None,
                };
                render_field_value(item, &dummy_field, registry, options)
            }
            Some(ItemsSpec::Ref(type_ref)) => {
                // Recursively project array items using the referenced type
                render_type_ref(item, type_ref, registry, options)
            }
            None => render_value(item, options),
        };
        out.push(rendered);
    }

    JsonValue::Array(out)
}

fn render_time(value: &Value, options: &RenderOptions) -> JsonValue {
    let ms = match value_to_i64(value) {
        Some(v) => v,
        None => return JsonValue::Null,
    };

    match options.time_render {
        TimeRender::UnixMs => JsonValue::Number(Number::from(ms)),
        TimeRender::Iso => {
            let dt = DateTime::<Utc>::from_timestamp_millis(ms);
            match dt {
                Some(ts) => JsonValue::String(ts.to_rfc3339()),
                None => JsonValue::Null,
            }
        }
    }
}

fn value_to_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Integer(int) => int.as_u64().or_else(|| {
            int.as_i64()
                .and_then(|v| if v >= 0 { Some(v as u64) } else { None })
        }),
        _ => None,
    }
}

fn value_to_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(int) => int.as_i64().or_else(|| {
            int.as_u64().and_then(|v| {
                if v <= i64::MAX as u64 {
                    Some(v as i64)
                } else {
                    None
                }
            })
        }),
        _ => None,
    }
}

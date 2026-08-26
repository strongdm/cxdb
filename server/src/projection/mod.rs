// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use rayon::prelude::*;
use std::borrow::Cow;
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
    pub string_limit: Option<usize>,
}

pub struct TurnProjectionOptions<'a> {
    pub view: &'a str,
    pub type_hint_mode: &'a str,
    pub as_type_id: Option<&'a str>,
    pub as_type_version: Option<u32>,
    pub render: &'a RenderOptions,
}

pub fn project_turn_page(
    turns: &[crate::store::TurnWithMeta],
    registry: &Registry,
    options: &TurnProjectionOptions<'_>,
) -> Result<Vec<JsonValue>> {
    if turns.len() < 8 {
        turns
            .iter()
            .map(|turn| project_turn(turn, registry, options))
            .collect()
    } else {
        turns
            .par_iter()
            .map(|turn| project_turn(turn, registry, options))
            .collect()
    }
}

pub fn serialize_turn_page(
    turns: &[crate::store::TurnWithMeta],
    registry: &Registry,
    options: &TurnProjectionOptions<'_>,
) -> Result<Vec<Vec<u8>>> {
    if turns.len() < 8 {
        turns
            .iter()
            .map(|turn| fast::serialize_turn(turn, registry, options))
            .collect()
    } else {
        turns
            .par_iter()
            .map(|turn| fast::serialize_turn(turn, registry, options))
            .collect()
    }
}

pub fn assemble_turn_page_json(
    meta: &JsonValue,
    next_before_turn_id: Option<JsonValue>,
    turns: &[Vec<u8>],
) -> Result<Vec<u8>> {
    let meta = serde_json::to_vec(meta)
        .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
    let next = serde_json::to_vec(&next_before_turn_id)
        .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
    let mut out = Vec::with_capacity(meta.len() + turns.iter().map(Vec::len).sum::<usize>() + 64);
    out.extend_from_slice(br#"{"meta":"#);
    out.extend_from_slice(&meta);
    out.extend_from_slice(br#","turns":["#);
    for (index, turn) in turns.iter().enumerate() {
        if index != 0 {
            out.push(b',');
        }
        out.extend_from_slice(turn);
    }
    out.extend_from_slice(br#"],"next_before_turn_id":"#);
    out.extend_from_slice(&next);
    out.push(b'}');
    Ok(out)
}

#[cfg(test)]
mod page_assembly_tests {
    use super::assemble_turn_page_json;
    use serde_json::json;

    #[test]
    fn assembles_empty_and_populated_pages_as_json() {
        let empty = assemble_turn_page_json(&json!({"context_id": 1}), None, &[])
            .expect("assemble empty page");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&empty).expect("parse empty page"),
            json!({"meta": {"context_id": 1}, "turns": [], "next_before_turn_id": null})
        );

        let turns = vec![br#"{"turn_id":1}"#.to_vec(), br#"{"turn_id":2}"#.to_vec()];
        let populated = assemble_turn_page_json(&json!({"context_id": 1}), Some(json!(2)), &turns)
            .expect("assemble populated page");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&populated).expect("parse populated page"),
            json!({
                "meta": {"context_id": 1},
                "turns": [{"turn_id": 1}, {"turn_id": 2}],
                "next_before_turn_id": 2
            })
        );
    }
}

mod fast;

pub struct ProjectionResult {
    pub data: JsonValue,
    pub unknown: Option<JsonValue>,
}

pub fn project_turn(
    item: &crate::store::TurnWithMeta,
    registry: &Registry,
    options: &TurnProjectionOptions<'_>,
) -> Result<JsonValue> {
    let declared_type_id = item.meta.declared_type_id.clone();
    let decoded_type = match options.type_hint_mode {
        "explicit" => Ok((
            options
                .as_type_id
                .ok_or_else(|| StoreError::InvalidInput("as_type_id required".into()))?
                .to_string(),
            options
                .as_type_version
                .ok_or_else(|| StoreError::InvalidInput("as_type_version required".into()))?,
        )),
        "latest" => registry
            .get_latest_type_version(&declared_type_id)
            .map(|latest| (declared_type_id.clone(), latest.version))
            .ok_or_else(|| StoreError::NotFound("type descriptor".into())),
        _ => Ok((declared_type_id.clone(), item.meta.declared_type_version)),
    };
    let mut turn = Map::new();
    turn.insert(
        "turn_id".into(),
        render_id(item.record.turn_id, options.render.u64_format),
    );
    turn.insert(
        "parent_turn_id".into(),
        render_id(item.record.parent_turn_id, options.render.u64_format),
    );
    turn.insert("depth".into(), JsonValue::Number(item.record.depth.into()));
    turn.insert("declared_type".into(), serde_json::json!({"type_id": declared_type_id, "type_version": item.meta.declared_type_version}));
    if options.view == "typed" || options.view == "both" {
        let projected = decoded_type.and_then(|(decoded_type_id, decoded_type_version)| {
            let descriptor = registry
                .get_type_version(&decoded_type_id, decoded_type_version)
                .ok_or_else(|| StoreError::NotFound("type descriptor".into()))?;
            let payload = item
                .payload
                .as_ref()
                .ok_or_else(|| StoreError::InvalidInput("payload not loaded".into()))?;
            let projected = project_msgpack(payload, descriptor, registry, options.render)?;
            Ok((decoded_type_id, decoded_type_version, projected))
        });
        match projected {
            Ok((decoded_type_id, decoded_type_version, projected)) => {
                turn.insert(
                    "decoded_as".into(),
                    serde_json::json!({"type_id": decoded_type_id, "type_version": decoded_type_version}),
                );
                turn.insert("data".into(), projected.data);
                if let Some(unknown) = projected.unknown {
                    turn.insert("unknown".into(), unknown);
                }
            }
            Err(error) => {
                turn.insert(
                    "projection_error".into(),
                    serde_json::json!({"message": error.to_string()}),
                );
            }
        }
    }
    if options.view == "raw" || options.view == "both" {
        let payload = item
            .payload
            .as_ref()
            .ok_or_else(|| StoreError::InvalidInput("payload not loaded".into()))?;
        turn.insert(
            "content_hash_b3".into(),
            JsonValue::String(hex::encode(item.record.payload_hash)),
        );
        turn.insert(
            "encoding".into(),
            JsonValue::Number(item.meta.encoding.into()),
        );
        turn.insert("compression".into(), JsonValue::Number(0u32.into()));
        turn.insert(
            "uncompressed_len".into(),
            JsonValue::Number((payload.len() as u32).into()),
        );
        match options.render.bytes_render {
            BytesRender::Base64 => {
                turn.insert(
                    "bytes_b64".into(),
                    JsonValue::String(base64::engine::general_purpose::STANDARD.encode(payload)),
                );
            }
            BytesRender::Hex => {
                turn.insert("bytes_hex".into(), JsonValue::String(hex::encode(payload)));
            }
            BytesRender::LenOnly => {
                turn.insert(
                    "bytes_len".into(),
                    JsonValue::Number((payload.len() as u64).into()),
                );
            }
        }
    }
    Ok(JsonValue::Object(turn))
}

fn render_id(value: u64, format: U64Format) -> JsonValue {
    match format {
        U64Format::String => JsonValue::String(value.to_string()),
        U64Format::Number => JsonValue::Number(value.into()),
    }
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
    if cursor.position() != payload.len() as u64 {
        return Err(StoreError::InvalidInput(
            "payload contains trailing msgpack data".into(),
        ));
    }
    if !matches!(value, Value::Map(_)) {
        return Err(StoreError::InvalidInput("payload is not a map".into()));
    }

    let map = normalize_fields(&value, descriptor);
    let mut data = Map::new();
    let mut unknown = Map::new();

    for (tag, field) in descriptor.fields.iter() {
        if let Some(val) = map.known.get(tag) {
            let rendered = render_field_value(val, field, registry, options);
            data.insert(field.name.clone(), rendered);
        }
    }

    if options.include_unknown {
        for (tag, val) in map.unknown.iter() {
            unknown.insert(tag.clone(), render_value(val, options));
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

struct NormalizedFields {
    known: HashMap<u64, Value>,
    unknown: HashMap<String, Value>,
}

fn normalize_fields(value: &Value, descriptor: &TypeVersionSpec) -> NormalizedFields {
    let mut known = HashMap::new();
    let mut unknown = HashMap::new();
    let map = match value {
        Value::Map(map) => map,
        _ => return NormalizedFields { known, unknown },
    };

    for (k, v) in map.iter() {
        let named = match k {
            Value::String(name) => name.as_str().and_then(|name| {
                descriptor
                    .fields
                    .iter()
                    .find_map(|(tag, field)| (field.name == name).then_some(*tag))
            }),
            _ => None,
        };
        if let Some(tag) = key_to_tag(k).or(named) {
            if matches!(k, Value::Integer(_)) || !known.contains_key(&tag) {
                known.insert(tag, v.clone());
            }
            if !descriptor.fields.contains_key(&tag) {
                let name = tag.to_string();
                if matches!(k, Value::Integer(_)) || !unknown.contains_key(&name) {
                    unknown.insert(name, v.clone());
                }
            }
        } else if let Value::String(name) = k {
            unknown.insert(name.as_str().unwrap_or("").to_string(), v.clone());
        }
    }
    NormalizedFields { known, unknown }
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

    // Handle type references - recursively project using the referenced type.
    // Schemas may use either `"type": "ref"` or `"type": "map"` with a separate
    // `"ref"` attribute (e.g., conversation-bundle.json).  Both forms carry a
    // `type_ref` that should trigger recursive projection.
    if field.type_ref.is_some() && (field.field_type == "ref" || field.field_type == "map") {
        if let Some(type_ref) = &field.type_ref {
            return render_type_ref(value, type_ref, registry, options);
        }
    }

    let field_type = field.field_type.as_str();
    match field_type {
        "u64" | "uint64" | "i64" | "int64" => render_u64(value, options),
        "u32" | "uint32" | "u8" | "uint8" | "int32" => render_int(value),
        "string" => render_string(value, options),
        "bool" => render_bool(value),
        "bytes" | "typed_blob" => render_bytes(value, options),
        "array" => render_array(value, field.items.as_ref(), registry, options),
        "unix_ms" | "time_ms" | "timestamp_ms" => render_time(value, options),
        _ => render_value(value, options),
    }
}

/// Recursively project a value using a referenced type's descriptor.
///
/// When `options.include_unknown` is true, any tags present in the msgpack
/// payload but absent from the type descriptor are collected into an
/// `"_unknown"` key on the returned object.  This mirrors the top-level
/// `project_msgpack` behaviour and ensures that clients reading via the HTTP
/// API can discover extension fields added by newer writers (e.g. Amplifier
/// adding `event_blobs` or `child_context_id` to a ToolCallItem).
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

    if !matches!(value, Value::Map(_)) {
        return render_value(value, options);
    }

    // Normalize the value to a tag map
    let map = normalize_fields(value, type_spec);

    // Project using the type descriptor
    let mut data = Map::new();
    for (tag, field) in type_spec.fields.iter() {
        if let Some(val) = map.known.get(tag) {
            let rendered = render_field_value(val, field, registry, options);
            data.insert(field.name.clone(), rendered);
        }
    }

    if options.include_unknown && !map.unknown.is_empty() {
        let mut unknown = Map::new();
        for (name, value) in map.unknown {
            unknown.insert(name, render_value(&value, options));
        }
        data.insert("_unknown".into(), JsonValue::Object(unknown));
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
        Value::String(s) => JsonValue::String(
            limit_string(s.as_str().unwrap_or(""), options.string_limit).into_owned(),
        ),
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

fn render_string(value: &Value, options: &RenderOptions) -> JsonValue {
    match value {
        Value::String(s) => JsonValue::String(
            limit_string(s.as_str().unwrap_or(""), options.string_limit).into_owned(),
        ),
        _ => JsonValue::Null,
    }
}

pub fn limit_string(value: &str, limit: Option<usize>) -> Cow<'_, str> {
    let Some(limit) = limit else {
        return Cow::Borrowed(value);
    };
    if value.len() <= limit {
        return Cow::Borrowed(value);
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(value[..end].to_string())
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

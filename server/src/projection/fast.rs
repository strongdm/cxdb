// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

//! Streaming page serializer entry point.
//!
//! The page scheduler owns parallelism. This serializer keeps the wire shape
//! in one place and delegates field semantics to the compatibility projector,
//! so named and numeric key handling cannot drift between paths.

use serde::Serialize;

use super::{project_turn, TurnProjectionOptions};
use crate::error::{Result, StoreError};
use crate::registry::Registry;
use crate::store::TurnWithMeta;

pub(super) fn serialize_turn(
    item: &TurnWithMeta,
    registry: &Registry,
    options: &TurnProjectionOptions<'_>,
) -> Result<Vec<u8>> {
    let projected = project_turn(item, registry, options)?;
    serde_json::to_vec(&projected)
        .map_err(|error| StoreError::InvalidInput(format!("json encode error: {error}")))
}

#[allow(dead_code)]
fn _serialize_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value)
        .map_err(|error| StoreError::InvalidInput(format!("json encode error: {error}")))
}

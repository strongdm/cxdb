// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use serde::de::{self, Deserialize, Deserializer, Visitor};
use std::fmt;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SseUint64 {
    pub value: u64,
    pub set: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SseUint32 {
    pub value: u32,
    pub set: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SseInt64 {
    pub value: i64,
    pub set: bool,
}

impl<'de> Deserialize<'de> for SseUint64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(SseUint64Visitor)
    }
}

impl<'de> Deserialize<'de> for SseUint32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(SseUint32Visitor)
    }
}

impl<'de> Deserialize<'de> for SseInt64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(SseInt64Visitor)
    }
}

struct SseUint64Visitor;

impl<'de> Visitor<'de> for SseUint64Visitor {
    type Value = SseUint64;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a uint64 number or string")
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SseUint64 { value, set: true })
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value < 0 {
            return Err(E::custom(format!("negative value {}", value)));
        }
        Ok(SseUint64 {
            value: value as u64,
            set: true,
        })
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        parse_u64_str(value).map(|value| SseUint64 { value, set: true })
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(&value)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SseUint64 {
            value: 0,
            set: true,
        })
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_unit()
    }
}

struct SseUint32Visitor;

impl<'de> Visitor<'de> for SseUint32Visitor {
    type Value = SseUint32;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a uint32 number or string")
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value > u64::from(u32::MAX) {
            return Err(E::custom(format!("value {} overflows uint32", value)));
        }
        Ok(SseUint32 {
            value: value as u32,
            set: true,
        })
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value < 0 {
            return Err(E::custom(format!("negative value {}", value)));
        }
        self.visit_u64(value as u64)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let value = parse_u64_str(value)?;
        self.visit_u64(value)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(&value)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SseUint32 {
            value: 0,
            set: true,
        })
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_unit()
    }
}

struct SseInt64Visitor;

impl<'de> Visitor<'de> for SseInt64Visitor {
    type Value = SseInt64;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an int64 number or string")
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SseInt64 { value, set: true })
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value > i64::MAX as u64 {
            return Err(E::custom(format!("value {} overflows int64", value)));
        }
        Ok(SseInt64 {
            value: value as i64,
            set: true,
        })
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        parse_i64_str(value).map(|value| SseInt64 { value, set: true })
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(&value)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SseInt64 {
            value: 0,
            set: true,
        })
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_unit()
    }
}

fn parse_u64_str<E: de::Error>(value: &str) -> Result<u64, E> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    trimmed
        .parse::<u64>()
        .map_err(|err| E::custom(format!("invalid uint64: {}", err)))
}

fn parse_i64_str<E: de::Error>(value: &str) -> Result<i64, E> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    trimmed
        .parse::<i64>()
        .map_err(|err| E::custom(format!("invalid int64: {}", err)))
}

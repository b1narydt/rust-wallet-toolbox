//! Lenient NaiveDateTime serde helpers.
//!
//! The TS wallet serializes dates as ISO 8601 strings with a trailing "Z"
//! (e.g. "2026-01-11T23:13:12.718Z"). Chrono's default NaiveDateTime
//! deserializer rejects the "Z" as "trailing input" since NaiveDateTime
//! has no timezone concept.
//!
//! This module provides a deserialize function that strips the trailing "Z"
//! before parsing, and a serialize function that outputs the standard format.

use chrono::NaiveDateTime;
use serde::{self, Deserialize, Deserializer, Serializer};

const FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.f";

/// Serialize a NaiveDateTime to ISO 8601 string.
pub fn serialize<S>(date: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let s = date.format(FORMAT).to_string();
    serializer.serialize_str(&s)
}

/// Deserialize a NaiveDateTime from an ISO 8601 string, tolerating trailing "Z".
pub fn deserialize<'de, D>(deserializer: D) -> Result<NaiveDateTime, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let trimmed = s.trim_end_matches('Z');
    NaiveDateTime::parse_from_str(trimmed, FORMAT).map_err(serde::de::Error::custom)
}

/// Optional variant for `Option<NaiveDateTime>` fields.
pub mod option {
    use chrono::NaiveDateTime;
    use serde::{self, Deserialize, Deserializer, Serializer};

    const FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.f";

    /// Serialize an optional NaiveDateTime.
    pub fn serialize<S>(date: &Option<NaiveDateTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match date {
            Some(d) => serializer.serialize_str(&d.format(FORMAT).to_string()),
            None => serializer.serialize_none(),
        }
    }

    /// Deserialize an optional NaiveDateTime, tolerating trailing "Z".
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<NaiveDateTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            Some(s) => {
                let trimmed = s.trim_end_matches('Z');
                NaiveDateTime::parse_from_str(trimmed, FORMAT)
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            }
            None => Ok(None),
        }
    }
}

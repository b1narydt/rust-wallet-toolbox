//! Lenient NaiveDateTime serde helpers.
//!
//! The TS wallet serializes dates as ISO 8601 strings with a trailing "Z"
//! (e.g. "2026-01-11T23:13:12.718Z"). Chrono's default NaiveDateTime
//! deserializer rejects the "Z" as "trailing input" since NaiveDateTime
//! has no timezone concept.
//!
//! This module provides a deserialize function that strips the trailing "Z"
//! before parsing, and a serialize function that outputs the standard format
//! with exactly 3 millisecond digits and a trailing "Z" -- matching TS
//! Date.toISOString() output byte-for-byte.

use chrono::NaiveDateTime;
use serde::{self, Deserialize, Deserializer, Serializer};

/// Tolerant parse format: accepts any fractional second precision (1-9 digits).
const PARSE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.f";

/// Strict serialize format: always produces exactly 3 millisecond digits.
const SERIALIZE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3f";

/// Serialize a NaiveDateTime to ISO 8601 string with exactly 3 ms digits and Z suffix.
pub fn serialize<S>(date: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let s = format!("{}Z", date.format(SERIALIZE_FORMAT));
    serializer.serialize_str(&s)
}

/// Deserialize a NaiveDateTime from an ISO 8601 string, tolerating trailing "Z".
pub fn deserialize<'de, D>(deserializer: D) -> Result<NaiveDateTime, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let trimmed = s.trim_end_matches('Z');
    NaiveDateTime::parse_from_str(trimmed, PARSE_FORMAT).map_err(serde::de::Error::custom)
}

/// Optional variant for `Option<NaiveDateTime>` fields.
pub mod option {
    use chrono::NaiveDateTime;
    use serde::{self, Deserialize, Deserializer, Serializer};

    /// Tolerant parse format: accepts any fractional second precision (1-9 digits).
    const PARSE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.f";

    /// Strict serialize format: always produces exactly 3 millisecond digits.
    const SERIALIZE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3f";

    /// Serialize an optional NaiveDateTime with exactly 3 ms digits and Z suffix.
    pub fn serialize<S>(date: &Option<NaiveDateTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match date {
            Some(d) => serializer.serialize_str(&format!("{}Z", d.format(SERIALIZE_FORMAT))),
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
                NaiveDateTime::parse_from_str(trimmed, PARSE_FORMAT)
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            }
            None => Ok(None),
        }
    }
}

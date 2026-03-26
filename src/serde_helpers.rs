//! Lenient serde helpers for TS server interop.
//!
//! The TS storage server sometimes returns `0`/`1` instead of `false`/`true`
//! for boolean fields. These helpers accept both representations.

use serde::{self, Deserialize, Deserializer};

/// Deserialize a bool that may arrive as `0`/`1` integer or `true`/`false`.
pub fn bool_from_int_or_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum IntOrBool {
        Bool(bool),
        Int(i64),
    }

    match IntOrBool::deserialize(deserializer)? {
        IntOrBool::Bool(b) => Ok(b),
        IntOrBool::Int(i) => Ok(i != 0),
    }
}

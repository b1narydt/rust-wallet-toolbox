//! Portable row encoding: table structs <-> BRC-38 JSON rows.
//!
//! Mirrors the TS `portableRow` / `fromPortableRow` pair and the per-kind
//! field tables (`binaryFieldsByKind`, `jsonFieldsByKind`, `dateFieldsByKind`).
//! In a portable row, byte columns are padded base64 strings, JSON-text
//! columns are decoded JSON objects, dates are ISO timestamps, and null/absent
//! columns are omitted entirely.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::{alphabet, Engine};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{WalletError, WalletResult};

/// Base64 decoder matching TS `Utils.toArray(v, 'base64')` leniency: padding
/// is required by the BRC-38 validator before decoding, but non-canonical
/// trailing bits (e.g. `"AB=="`) decode instead of erroring, as they do in TS.
static BASE64_LENIENT: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_allow_trailing_bits(true)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Byte-array columns per row kind (TS `binaryFieldsByKind`).
pub fn binary_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "commission" | "output" => &["lockingScript"],
        "provenTx" => &["merklePath", "rawTx"],
        "provenTxReq" => &["rawTx", "inputBEEF"],
        "transaction" => &["inputBEEF", "rawTx"],
        _ => &[],
    }
}

/// JSON-text columns per row kind (TS `jsonFieldsByKind`).
pub fn json_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "provenTxReq" => &["history", "notify"],
        "syncState" => &["syncMap", "errorLocal", "errorOther"],
        _ => &[],
    }
}

/// Date columns per row kind (TS `dateFieldsByKind`). Every kind carries the
/// entity timestamps; syncState adds `when`.
pub fn date_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "syncState" => &["created_at", "updated_at", "when"],
        _ => &["created_at", "updated_at"],
    }
}

/// Convert a table struct into a portable BRC-38 row (TS `portableRow`).
///
/// The struct's serde output already matches the TS column names and ISO date
/// format; this drops nulls, converts byte arrays to base64, and parses
/// JSON-text columns into objects.
pub fn portable_row<T: Serialize>(kind: &str, row: &T) -> WalletResult<Map<String, Value>> {
    let value = serde_json::to_value(row)
        .map_err(|e| WalletError::Internal(format!("portableRow serialization failed: {e}")))?;
    let map = match value {
        Value::Object(map) => map,
        _ => {
            return Err(WalletError::Internal(
                "portableRow requires a struct that serializes to an object".to_string(),
            ))
        }
    };
    let binary = binary_fields(kind);
    let json = json_fields(kind);
    let mut out = Map::new();
    for (key, value) in map {
        if value.is_null() || key == "logger" {
            continue;
        }
        if binary.contains(&key.as_str()) {
            let bytes = value_to_bytes(&value, &key)?;
            out.insert(key, Value::String(BASE64.encode(bytes)));
        } else if json.contains(&key.as_str()) {
            let parsed = match value {
                Value::String(s) => serde_json::from_str::<Value>(&s).map_err(|e| {
                    WalletError::BadRequest(format!(
                        "portableRow {kind}.{key} is not valid JSON text: {e}"
                    ))
                })?,
                other => other,
            };
            out.insert(key, parsed);
        } else {
            out.insert(key, value);
        }
    }
    Ok(out)
}

/// Convert a portable BRC-38 row back into a table struct
/// (TS `fromPortableRow`): base64 back to bytes, JSON objects back to JSON
/// text, dates parsed by the struct's serde. Unknown keys are ignored, as the
/// TS storage layer ignores them at insert time.
pub fn from_portable_row<T: DeserializeOwned>(
    kind: &str,
    row: &Map<String, Value>,
) -> WalletResult<T> {
    let binary = binary_fields(kind);
    let json = json_fields(kind);
    let mut out = Map::new();
    for (key, value) in row {
        if binary.contains(&key.as_str()) {
            let s = value.as_str().ok_or_else(|| {
                WalletError::BadRequest(format!("BRC-38 {kind}.{key} must be a base64 string"))
            })?;
            let bytes = BASE64_LENIENT.decode(s).map_err(|e| {
                WalletError::BadRequest(format!("BRC-38 {kind}.{key} is not valid base64: {e}"))
            })?;
            out.insert(
                key.clone(),
                Value::Array(bytes.into_iter().map(|b| Value::from(b as i64)).collect()),
            );
        } else if json.contains(&key.as_str()) {
            let text = serde_json::to_string(value)
                .map_err(|e| WalletError::Internal(format!("JSON field encode failed: {e}")))?;
            out.insert(key.clone(), Value::String(text));
        } else {
            out.insert(key.clone(), value.clone());
        }
    }
    serde_json::from_value(Value::Object(out)).map_err(|e| {
        WalletError::BadRequest(format!("BRC-38 {kind} row does not decode: {e}"))
    })
}

fn value_to_bytes(value: &Value, key: &str) -> WalletResult<Vec<u8>> {
    let items = value.as_array().ok_or_else(|| {
        WalletError::Internal(format!("portableRow binary field {key} is not a byte array"))
    })?;
    items
        .iter()
        .map(|item| {
            item.as_u64()
                .filter(|b| *b <= 255)
                .map(|b| b as u8)
                .ok_or_else(|| {
                    WalletError::Internal(format!(
                        "portableRow binary field {key} contains a non-byte value"
                    ))
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    use crate::status::ProvenTxReqStatus;
    use crate::tables::ProvenTxReq;

    fn req() -> ProvenTxReq {
        let at = NaiveDate::from_ymd_opt(2026, 1, 2)
            .unwrap()
            .and_hms_milli_opt(3, 4, 5, 6)
            .unwrap();
        ProvenTxReq {
            created_at: at,
            updated_at: at,
            proven_tx_req_id: 3,
            proven_tx_id: Some(2),
            status: ProvenTxReqStatus::Nosend,
            attempts: 0,
            notified: false,
            txid: "ab".repeat(32),
            batch: None,
            history: "{\"x\":1}".to_string(),
            notify: "{}".to_string(),
            raw_tx: vec![4, 5, 6],
            input_beef: Some(vec![1, 2, 3]),
        }
    }

    #[test]
    fn portable_row_round_trips_proven_tx_req() {
        let row = portable_row("provenTxReq", &req()).unwrap();
        assert_eq!(row["rawTx"], Value::String("BAUG".to_string()));
        assert_eq!(row["inputBEEF"], Value::String("AQID".to_string()));
        assert!(row["history"].is_object());
        assert!(!row.contains_key("batch"), "null fields must be omitted");
        assert_eq!(row["created_at"], Value::String("2026-01-02T03:04:05.006Z".into()));

        let back: ProvenTxReq = from_portable_row("provenTxReq", &row).unwrap();
        assert_eq!(back.raw_tx, vec![4, 5, 6]);
        assert_eq!(back.input_beef, Some(vec![1, 2, 3]));
        assert_eq!(back.history, "{\"x\":1}");
        assert_eq!(back.created_at, req().created_at);
    }
}

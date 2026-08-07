//! Canonical JSON serialization matching the TS `canonicalize` function.
//!
//! The BRC-38 payload bytes are only reproducible across implementations if
//! serialization is deterministic: object keys sorted, no whitespace, and
//! value formatting identical to what `JSON.stringify` produces in JS.

use std::cmp::Ordering;

use serde_json::Value;

use crate::error::{WalletError, WalletResult};

/// Serialize a JSON value to its canonical string form (TS `canonicalize`).
///
/// - Object keys are sorted by UTF-16 code units (the JS `<` operator on
///   strings), not by UTF-8 bytes -- these differ for keys mixing BMP
///   characters above U+DFFF with supplementary-plane characters.
/// - Numbers must be integers. The TS reference serializes any finite number
///   via `JSON.stringify`, but ECMAScript float-to-string formatting (e.g.
///   `1e+21` vs `1e21`, decimal/exponent switchover thresholds) is not
///   reproducible with Rust float formatting, so rather than risk silently
///   producing different bytes for the same document, non-integer numbers are
///   rejected loudly. Wallet table data contains only integers.
pub fn canonicalize(value: &Value) -> WalletResult<String> {
    let mut out = String::new();
    write_canonical(value, &mut out)?;
    Ok(out)
}

fn write_canonical(value: &Value, out: &mut String) -> WalletResult<()> {
    match value {
        Value::Null => Err(WalletError::BadRequest(
            "Cannot canonicalize null or undefined".to_string(),
        )),
        Value::Bool(b) => {
            out.push_str(if *b { "true" } else { "false" });
            Ok(())
        }
        Value::Number(_) => {
            out.push_str(&canonical_number(value)?);
            Ok(())
        }
        Value::String(s) => {
            out.push_str(&escape_json_string(s));
            Ok(())
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out)?;
            }
            out.push(']');
            Ok(())
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| js_string_cmp(a, b));
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&escape_json_string(key));
                out.push(':');
                write_canonical(&map[key.as_str()], out)?;
            }
            out.push('}');
            Ok(())
        }
    }
}

/// Format a JSON number the way `JSON.stringify` would, restricted to
/// integer values (see [`canonicalize`] for why floats are rejected).
fn canonical_number(value: &Value) -> WalletResult<String> {
    let n = match value {
        Value::Number(n) => n,
        _ => unreachable!("caller matched Value::Number"),
    };
    if let Some(i) = n.as_i64() {
        return Ok(i.to_string());
    }
    if let Some(u) = n.as_u64() {
        return Ok(u.to_string());
    }
    if let Some(f) = n.as_f64() {
        if !f.is_finite() {
            return Err(WalletError::BadRequest(
                "Cannot canonicalize non-finite number".to_string(),
            ));
        }
        // JSON.stringify prints integral floats within the safe-integer range
        // without a fractional part; match that.
        if f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_991.0 {
            return Ok((f as i64).to_string());
        }
    }
    Err(WalletError::BadRequest(format!(
        "Cannot canonicalize non-integer number: {n}"
    )))
}

/// Compare strings by UTF-16 code units -- the semantics of `a < b` in JS,
/// which the TS `compareCodepoints` relies on.
pub fn js_string_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Escape a string exactly as `JSON.stringify` does: short escapes for
/// `\b \t \n \f \r \" \\`, `\u00xx` for other control characters, and all
/// other characters emitted raw. `serde_json` implements this same scheme.
fn escape_json_string(s: &str) -> String {
    serde_json::to_string(s).expect("string serialization is infallible")
}

/// Format a `chrono` timestamp the way `Date.prototype.toISOString` does:
/// millisecond precision with a trailing `Z`.
pub fn iso_date(date: &chrono::NaiveDateTime) -> String {
    format!("{}Z", date.format("%Y-%m-%dT%H:%M:%S%.3f"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonicalize_sorts_keys_and_strips_whitespace() {
        let v = json!({"b": 1, "a": [true, "x"], "c": {"z": 2, "y": 3}});
        assert_eq!(
            canonicalize(&v).unwrap(),
            r#"{"a":[true,"x"],"b":1,"c":{"y":3,"z":2}}"#
        );
    }

    #[test]
    fn canonicalize_rejects_null() {
        assert!(canonicalize(&Value::Null).is_err());
        assert!(canonicalize(&json!({"a": null})).is_err());
    }

    #[test]
    fn canonicalize_rejects_non_integer_numbers() {
        assert!(canonicalize(&json!(1.5)).is_err());
        assert_eq!(canonicalize(&json!(-42)).unwrap(), "-42");
    }

    #[test]
    fn js_string_cmp_uses_utf16_order() {
        // U+FF61 (halfwidth ideographic full stop) is a single UTF-16 unit
        // 0xFF61; U+10000 encodes as surrogates starting 0xD800. JS orders
        // the surrogate pair first; UTF-8 byte order would reverse them.
        assert_eq!(js_string_cmp("\u{10000}", "\u{FF61}"), Ordering::Less);
        assert_eq!(js_string_cmp("a", "b"), Ordering::Less);
    }
}

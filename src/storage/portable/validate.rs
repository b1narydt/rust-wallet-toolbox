//! BRC-38 document validation (TS `validateBRC38` and helpers).
//!
//! Validation operates on the raw JSON document, before any decoding into
//! table structs, so that structurally valid documents with fields this port
//! does not model still validate exactly as they do in TS. Import is strict by
//! design: the full relational graph is checked before a single row is
//! written. Do not weaken these checks to make an import pass -- a backup
//! that imports garbage is worse than one that refuses.

use std::collections::HashSet;

use chrono::NaiveDateTime;
use serde_json::{Map, Value};

use crate::error::{WalletError, WalletResult};

use super::canonical::js_string_cmp;
use super::row::{binary_fields, date_fields, json_fields};

/// The BRC-38 `title` constant.
pub const BRC38_TITLE: &str = "User Wallet Data Format";

/// The thirteen table arrays of a BRC-38 document, in document order.
pub const TABLE_NAMES: [&str; 13] = [
    "provenTxs",
    "provenTxReqs",
    "outputBaskets",
    "transactions",
    "commissions",
    "outputs",
    "outputTags",
    "outputTagMaps",
    "txLabels",
    "txLabelMaps",
    "certificates",
    "certificateFields",
    "syncStates",
];

/// A validated BRC-38 document.
///
/// The document is held as raw JSON (as TS holds the parsed object): the
/// canonical bytes are the source of truth, and rows may carry fields this
/// port does not model in its table structs. Construction always validates,
/// so holding a `Brc38WalletData` implies the document passed the full
/// BRC-38 validation suite.
#[derive(Debug, Clone)]
pub struct Brc38WalletData {
    value: Value,
}

impl Brc38WalletData {
    /// The underlying JSON document.
    pub fn as_value(&self) -> &Value {
        &self.value
    }

    /// The `sourceStorage` row.
    pub fn source_storage(&self) -> &Map<String, Value> {
        object(&self.value["sourceStorage"])
    }

    /// The `user` row.
    pub fn user(&self) -> &Map<String, Value> {
        object(&self.value["user"])
    }

    /// A table's rows by BRC-38 table name.
    pub fn table(&self, name: &str) -> Vec<&Map<String, Value>> {
        self.value["tables"][name]
            .as_array()
            .expect("validated document has all table arrays")
            .iter()
            .map(object)
            .collect()
    }
}

fn object(value: &Value) -> &Map<String, Value> {
    value.as_object().expect("validated document shape")
}

/// Validate a JSON value as a BRC-38 document (TS `validateBRC38`).
pub fn validate_brc38(value: Value) -> WalletResult<Brc38WalletData> {
    reject_nulls(&value, "document")?;
    let map = value
        .as_object()
        .ok_or_else(|| bad("BRC-38 document must be an object"))?;
    if js_int(map.get("brc")) != Some(38) {
        return Err(bad("BRC-38 document brc must equal 38"));
    }
    if map.get("title").and_then(Value::as_str) != Some(BRC38_TITLE) {
        return Err(bad("BRC-38 title must equal User Wallet Data Format"));
    }
    if js_int(map.get("formatVersion")) != Some(1) {
        return Err(bad("BRC-38 formatVersion must equal 1"));
    }
    assert_iso_date(map.get("exportedAt").unwrap_or(&Value::Null), "exportedAt")?;
    let source_storage = map
        .get("sourceStorage")
        .and_then(Value::as_object)
        .ok_or_else(|| bad("BRC-38 sourceStorage must be an object"))?;
    let user = map
        .get("user")
        .and_then(Value::as_object)
        .ok_or_else(|| bad("BRC-38 user must be an object"))?;
    let tables = map
        .get("tables")
        .and_then(Value::as_object)
        .ok_or_else(|| bad("BRC-38 tables must be an object"))?;
    for name in TABLE_NAMES {
        if !tables.get(name).map(Value::is_array).unwrap_or(false) {
            return Err(bad(&format!("BRC-38 tables.{name} must be an array")));
        }
    }

    validate_portable_rows("settings", &[source_storage], "sourceStorage")?;
    validate_portable_rows("user", &[user], "user")?;
    for (kind, name) in [
        ("provenTx", "provenTxs"),
        ("provenTxReq", "provenTxReqs"),
        ("outputBasket", "outputBaskets"),
        ("transaction", "transactions"),
        ("commission", "commissions"),
        ("output", "outputs"),
        ("outputTag", "outputTags"),
        ("outputTagMap", "outputTagMaps"),
        ("txLabel", "txLabels"),
        ("txLabelMap", "txLabelMaps"),
        ("certificate", "certificates"),
        ("certificateField", "certificateFields"),
        ("syncState", "syncStates"),
    ] {
        let rows: Vec<&Map<String, Value>> = tables[name]
            .as_array()
            .expect("checked above")
            .iter()
            .enumerate()
            .map(|(i, row)| {
                row.as_object()
                    .ok_or_else(|| bad(&format!("BRC-38 {name}[{i}] must be an object")))
            })
            .collect::<WalletResult<_>>()?;
        validate_portable_rows(kind, &rows, name)?;
    }

    let data = Brc38WalletData { value };
    validate_relationships(&data)?;
    Ok(data)
}

/// Per-row field constraints (TS `validatePortableRows`): dates are strict
/// ISO timestamps, byte columns are padded base64, JSON columns are objects.
fn validate_portable_rows(
    kind: &str,
    rows: &[&Map<String, Value>],
    path: &str,
) -> WalletResult<()> {
    let dates = date_fields(kind);
    let binaries = binary_fields(kind);
    let jsons = json_fields(kind);
    for (index, row) in rows.iter().enumerate() {
        let row_path = format!("{path}[{index}]");
        for field in dates {
            if let Some(value) = row.get(*field) {
                assert_iso_date(value, &format!("{row_path}.{field}"))?;
            }
        }
        for field in binaries {
            if let Some(value) = row.get(*field) {
                assert_base64(value, &format!("{row_path}.{field}"))?;
            }
        }
        for field in jsons {
            if let Some(value) = row.get(*field) {
                if !value.is_object() {
                    return Err(bad(&format!("BRC-38 {row_path}.{field} must be an object")));
                }
            }
        }
    }
    Ok(())
}

/// Cross-table relational validation (TS `validateRelationships`): every
/// foreign key must reference a row inside the same document.
fn validate_relationships(data: &Brc38WalletData) -> WalletResult<()> {
    let idx = RelationshipIndex::build(data)?;

    for row in data.table("transactions") {
        require_user_id(row, idx.user_id, "transactions")?;
        if let Some(v) = present(row, "provenTxId") {
            require_ref(
                &idx.proven_tx_ids,
                Some(v),
                "transaction.provenTxId",
                "BRC-38 transaction.provenTxId does not reference an exported provenTx",
            )?;
        }
    }
    for (table, label) in [
        ("outputBaskets", "outputBaskets"),
        ("outputTags", "outputTags"),
        ("txLabels", "txLabels"),
        ("certificates", "certificates"),
        ("syncStates", "syncStates"),
    ] {
        for row in data.table(table) {
            require_user_id(row, idx.user_id, label)?;
        }
    }
    for row in data.table("outputs") {
        require_user_id(row, idx.user_id, "outputs")?;
        require_ref(
            &idx.tx_ids,
            row.get("transactionId"),
            "output.transactionId",
            "BRC-38 output.transactionId does not reference an exported transaction",
        )?;
        if let Some(v) = present(row, "basketId") {
            require_ref(
                &idx.basket_ids,
                Some(v),
                "output.basketId",
                "BRC-38 output.basketId does not reference an exported output basket",
            )?;
        }
        if let Some(v) = present(row, "spentBy") {
            require_ref(
                &idx.tx_ids,
                Some(v),
                "output.spentBy",
                "BRC-38 output.spentBy does not reference an exported transaction",
            )?;
        }
    }
    for row in data.table("commissions") {
        require_user_id(row, idx.user_id, "commissions")?;
        require_ref(
            &idx.tx_ids,
            row.get("transactionId"),
            "commission.transactionId",
            "BRC-38 commission.transactionId does not reference an exported transaction",
        )?;
    }
    for row in data.table("txLabelMaps") {
        require_ref(
            &idx.tx_ids,
            row.get("transactionId"),
            "txLabelMap.transactionId",
            "BRC-38 txLabelMap.transactionId does not reference an exported transaction",
        )?;
        require_ref(
            &idx.tx_label_ids,
            row.get("txLabelId"),
            "txLabelMap.txLabelId",
            "BRC-38 txLabelMap.txLabelId does not reference an exported transaction label",
        )?;
    }
    for row in data.table("outputTagMaps") {
        require_ref(
            &idx.output_ids,
            row.get("outputId"),
            "outputTagMap.outputId",
            "BRC-38 outputTagMap.outputId does not reference an exported output",
        )?;
        require_ref(
            &idx.output_tag_ids,
            row.get("outputTagId"),
            "outputTagMap.outputTagId",
            "BRC-38 outputTagMap.outputTagId does not reference an exported output tag",
        )?;
    }
    for row in data.table("certificateFields") {
        require_user_id(row, idx.user_id, "certificateFields")?;
        require_ref(
            &idx.certificate_ids,
            row.get("certificateId"),
            "certificateField.certificateId",
            "BRC-38 certificateField.certificateId does not reference an exported certificate",
        )?;
    }
    for row in data.table("provenTxReqs") {
        let txid = require_string(row.get("txid"), "provenTxReq.txid")?;
        if !idx.txid_values.contains(txid) {
            return Err(bad(
                "BRC-38 provenTxReq.txid does not match an exported transaction",
            ));
        }
        if let Some(v) = present(row, "provenTxId") {
            require_ref(
                &idx.proven_tx_ids,
                Some(v),
                "provenTxReq.provenTxId",
                "BRC-38 provenTxReq.provenTxId does not reference an exported provenTx",
            )?;
        }
    }
    Ok(())
}

struct RelationshipIndex {
    user_id: i64,
    tx_ids: HashSet<i64>,
    txid_values: HashSet<String>,
    proven_tx_ids: HashSet<i64>,
    basket_ids: HashSet<i64>,
    output_ids: HashSet<i64>,
    output_tag_ids: HashSet<i64>,
    tx_label_ids: HashSet<i64>,
    certificate_ids: HashSet<i64>,
}

impl RelationshipIndex {
    fn build(data: &Brc38WalletData) -> WalletResult<Self> {
        Ok(Self {
            user_id: require_number(data.user().get("userId"), "user.userId")?,
            tx_ids: ids(&data.table("transactions"), "transactionId", "transactions")?,
            txid_values: data
                .table("transactions")
                .iter()
                .filter_map(|row| row.get("txid").and_then(Value::as_str))
                .map(str::to_string)
                .collect(),
            proven_tx_ids: ids(&data.table("provenTxs"), "provenTxId", "provenTxs")?,
            basket_ids: ids(&data.table("outputBaskets"), "basketId", "outputBaskets")?,
            output_ids: ids(&data.table("outputs"), "outputId", "outputs")?,
            output_tag_ids: ids(&data.table("outputTags"), "outputTagId", "outputTags")?,
            tx_label_ids: ids(&data.table("txLabels"), "txLabelId", "txLabels")?,
            certificate_ids: ids(&data.table("certificates"), "certificateId", "certificates")?,
        })
    }
}

/// Collect a table's primary keys, rejecting duplicates (TS `ids`).
fn ids(rows: &[&Map<String, Value>], field: &str, label: &str) -> WalletResult<HashSet<i64>> {
    let mut set = HashSet::new();
    for row in rows {
        let id = require_number(row.get(field), &format!("{label}.{field}"))?;
        if !set.insert(id) {
            return Err(bad(&format!("BRC-38 duplicate {label}.{field}: {id}")));
        }
    }
    Ok(set)
}

fn require_ref(
    set: &HashSet<i64>,
    value: Option<&Value>,
    path: &str,
    message: &str,
) -> WalletResult<()> {
    if !set.contains(&require_number(value, path)?) {
        return Err(bad(message));
    }
    Ok(())
}

fn require_user_id(row: &Map<String, Value>, user_id: i64, label: &str) -> WalletResult<()> {
    if require_number(row.get("userId"), &format!("{label}.userId"))? != user_id {
        return Err(bad(&format!("BRC-38 {label}.userId does not match user.userId")));
    }
    Ok(())
}

/// TS `requireNumber`: the value must be an integer in the JS sense
/// (`Number.isInteger`), so integral floats are accepted.
fn require_number(value: Option<&Value>, path: &str) -> WalletResult<i64> {
    js_int(value).ok_or_else(|| bad(&format!("BRC-38 {path} must be an integer")))
}

fn require_string<'v>(value: Option<&'v Value>, path: &str) -> WalletResult<&'v str> {
    value
        .and_then(Value::as_str)
        .ok_or_else(|| bad(&format!("BRC-38 {path} must be a string")))
}

/// Extract an integer with `Number.isInteger` semantics: i64/u64 directly,
/// or a float whose value is integral and within the i64 range.
pub(super) fn js_int(value: Option<&Value>) -> Option<i64> {
    let n = value?.as_number()?;
    if let Some(i) = n.as_i64() {
        return Some(i);
    }
    let f = n.as_f64()?;
    if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
        return Some(f as i64);
    }
    None
}

/// A field present with a non-null value (TS `row.x != null`).
fn present<'v>(row: &'v Map<String, Value>, key: &str) -> Option<&'v Value> {
    match row.get(key) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v),
    }
}

/// TS `assertIsoDate`: exactly `YYYY-MM-DDTHH:MM:SS.mmmZ`, and a valid
/// calendar instant that round-trips to the same string.
pub fn assert_iso_date(value: &Value, path: &str) -> WalletResult<()> {
    let s = value
        .as_str()
        .ok_or_else(|| bad(&format!("BRC-38 {path} must be a UTC ISO timestamp")))?;
    if !iso_shape_ok(s) {
        return Err(bad(&format!("BRC-38 {path} must be a UTC ISO timestamp")));
    }
    let parsed = NaiveDateTime::parse_from_str(&s[..s.len() - 1], "%Y-%m-%dT%H:%M:%S%.3f")
        .map_err(|_| bad(&format!("BRC-38 {path} is invalid")))?;
    if format!("{}Z", parsed.format("%Y-%m-%dT%H:%M:%S%.3f")) != s {
        return Err(bad(&format!("BRC-38 {path} is invalid")));
    }
    Ok(())
}

/// The TS regex `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$`.
fn iso_shape_ok(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 24 {
        return false;
    }
    for (i, c) in b.iter().enumerate() {
        let ok = match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'.',
            23 => *c == b'Z',
            _ => c.is_ascii_digit(),
        };
        if !ok {
            return false;
        }
    }
    true
}

/// TS `assertBase64`: a padded base64 string (`len % 4 == 0`, standard
/// alphabet, up to two trailing `=`).
pub fn assert_base64(value: &Value, path: &str) -> WalletResult<()> {
    let err = || bad(&format!("BRC-38 {path} must be padded base64"));
    let s = value.as_str().ok_or_else(err)?;
    if s.len() % 4 != 0 {
        return Err(err());
    }
    let trimmed = s.trim_end_matches('=');
    if s.len() - trimmed.len() > 2 {
        return Err(err());
    }
    if !trimmed
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/')
    {
        return Err(err());
    }
    Ok(())
}

/// TS `rejectNulls`: no JSON null may appear anywhere in the document --
/// absent columns are omitted, never null.
pub fn reject_nulls(value: &Value, path: &str) -> WalletResult<()> {
    match value {
        Value::Null => Err(bad(&format!("BRC-38 {path} must omit null values"))),
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                reject_nulls(item, &format!("{path}[{i}]"))?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for (key, child) in map {
                reject_nulls(child, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Sort all table arrays into canonical order (TS `sortBRC38Tables`).
///
/// One divergence from TS: `certificateFields` ties on `certificateId` break
/// by `fieldName.localeCompare(..)` in TS, which is ICU locale collation.
/// This port approximates it (case-insensitive primary, lowercase-first
/// tiebreak, code-unit fallback), which matches ICU for plain ASCII names;
/// exotic fieldName pairs could order differently. Ordering is an export-side
/// property only -- import never requires sortedness.
pub fn sort_brc38_tables(tables: &mut Map<String, Value>) -> WalletResult<()> {
    sort_by_number(tables, "provenTxs", "provenTxId", None)?;
    sort_by_number(tables, "provenTxReqs", "provenTxReqId", None)?;
    sort_by_number(tables, "outputBaskets", "basketId", None)?;
    sort_by_number(tables, "transactions", "transactionId", None)?;
    sort_by_number(tables, "commissions", "commissionId", None)?;
    sort_by_number(tables, "outputs", "outputId", None)?;
    sort_by_number(tables, "outputTags", "outputTagId", None)?;
    sort_by_number(tables, "outputTagMaps", "outputId", Some("outputTagId"))?;
    sort_by_number(tables, "txLabels", "txLabelId", None)?;
    sort_by_number(tables, "txLabelMaps", "transactionId", Some("txLabelId"))?;
    sort_by_number(tables, "certificates", "certificateId", None)?;
    sort_certificate_fields(tables)?;
    sort_by_number(tables, "syncStates", "syncStateId", None)?;
    Ok(())
}

fn sort_by_number(
    tables: &mut Map<String, Value>,
    name: &str,
    field: &str,
    second_field: Option<&str>,
) -> WalletResult<()> {
    let rows = tables
        .get_mut(name)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| bad(&format!("BRC-38 tables.{name} must be an array")))?;
    let mut keyed: Vec<((i64, i64), Value)> = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        let map = row
            .as_object()
            .ok_or_else(|| bad(&format!("BRC-38 tables.{name} rows must be objects")))?;
        let first = require_number(map.get(field), field)?;
        let second = match second_field {
            Some(sf) => require_number(map.get(sf), sf)?,
            None => 0,
        };
        keyed.push(((first, second), row));
    }
    keyed.sort_by_key(|(key, _)| *key);
    rows.extend(keyed.into_iter().map(|(_, row)| row));
    Ok(())
}

fn sort_certificate_fields(tables: &mut Map<String, Value>) -> WalletResult<()> {
    let rows = tables
        .get_mut("certificateFields")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| bad("BRC-38 tables.certificateFields must be an array"))?;
    let mut keyed: Vec<((i64, String), Value)> = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        let map = row
            .as_object()
            .ok_or_else(|| bad("BRC-38 tables.certificateFields rows must be objects"))?;
        let id = require_number(map.get("certificateId"), "certificateId")?;
        let name = require_string(map.get("fieldName"), "fieldName")?.to_string();
        keyed.push(((id, name), row));
    }
    keyed.sort_by(|((id_a, name_a), _), ((id_b, name_b), _)| {
        id_a.cmp(id_b).then_with(|| locale_ish_cmp(name_a, name_b))
    });
    rows.extend(keyed.into_iter().map(|(_, row)| row));
    Ok(())
}

/// Approximation of `String.prototype.localeCompare` for ASCII: primary
/// case-insensitive comparison, lowercase before uppercase on a full-string
/// tie, then code-unit order.
fn locale_ish_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let primary = a
        .chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase));
    primary
        .then_with(|| js_string_cmp(b, a)) // lowercase (higher code units) first
        .then_with(|| js_string_cmp(a, b))
}

pub(super) fn bad(message: &str) -> WalletError {
    WalletError::BadRequest(message.to_string())
}

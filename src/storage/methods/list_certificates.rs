//! listCertificates query with field joining.
//!
//! Ported from wallet-toolbox/src/storage/methods/listCertificates.ts.
//! Translates bsv-sdk `ListCertificatesArgs` into storage find calls,
//! joining certificate fields to produce `ListCertificatesResult`.

use std::collections::HashMap;

use bsv::primitives::public_key::PublicKey;
use bsv::wallet::interfaces::{
    Certificate as SdkCertificate, CertificateResult, CertificateType, ListCertificatesArgs,
    ListCertificatesResult, SerialNumber,
};

use crate::error::{WalletError, WalletResult};
use crate::storage::find_args::{
    CertificateFieldPartial, CertificatePartial, FindCertificateFieldsArgs, FindCertificatesArgs,
    Paged,
};
use crate::storage::traits::reader::StorageReader;
use crate::storage::TrxToken;

/// Encode bytes to base64 string.
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn cert_type_to_string(ct: &CertificateType) -> String {
    base64_encode(&ct.0)
}

fn serial_number_to_string(sn: &SerialNumber) -> String {
    base64_encode(&sn.0)
}

/// Decode a hex string to bytes. Returns None on invalid hex.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        match u8::from_str_radix(&s[i..i + 2], 16) {
            Ok(b) => bytes.push(b),
            Err(_) => return None,
        }
    }
    Some(bytes)
}

/// Execute a listCertificates query against the given storage reader.
///
/// Translates the bsv-sdk `ListCertificatesArgs` to low-level find/count calls,
/// applying certifier/type filtering, pagination, and field joining.
pub async fn list_certificates(
    storage: &dyn StorageReader,
    _auth: &str,
    user_id: i64,
    args: &ListCertificatesArgs,
    trx: Option<&TrxToken>,
) -> WalletResult<ListCertificatesResult> {
    let limit = args.limit.unwrap_or(10) as i64;
    let offset = args.offset.map(|o| o as i64).unwrap_or(0);

    // Build the certificate partial filter, incorporating the optional partial match
    let mut cert_partial = CertificatePartial {
        user_id: Some(user_id),
        is_deleted: Some(false),
        ..Default::default()
    };
    if let Some(ref p) = args.partial {
        if let Some(ref ct) = p.cert_type {
            cert_partial.cert_type = Some(cert_type_to_string(ct));
        }
        if let Some(ref sn) = p.serial_number {
            cert_partial.serial_number = Some(serial_number_to_string(sn));
        }
        if let Some(ref c) = p.certifier {
            cert_partial.certifier = Some(c.to_der_hex());
        }
        if let Some(ref s) = p.subject {
            cert_partial.subject = Some(s.to_der_hex());
        }
    }

    let find_args = FindCertificatesArgs {
        partial: cert_partial,
        paged: Some(Paged { limit, offset }),
        ..Default::default()
    };

    let certs = storage.find_certificates(&find_args, trx).await?;

    // Filter by certifiers and types in-memory.
    // The TS version passes these to the storage query directly, but our FindCertificatesArgs
    // doesn't have multi-value certifier/type filters, so we filter post-query.
    let filtered_certs: Vec<_> = certs
        .into_iter()
        .filter(|c| {
            // Filter by certifiers if specified
            if !args.certifiers.is_empty() {
                let cert_key_hex = &c.certifier;
                let matches_certifier = args
                    .certifiers
                    .iter()
                    .any(|pk| pk.to_der_hex() == *cert_key_hex);
                if !matches_certifier {
                    return false;
                }
            }
            // Filter by types if specified
            if !args.types.is_empty() {
                let matches_type = args.types.iter().any(|ct| {
                    let parsed = CertificateType::from_string(&c.cert_type)
                        .unwrap_or(CertificateType([0u8; 32]));
                    parsed == *ct
                });
                if !matches_type {
                    return false;
                }
            }
            true
        })
        .collect();

    // Build CertificateResult with fields joined
    let mut certificates: Vec<CertificateResult> = Vec::with_capacity(filtered_certs.len());

    for cert in &filtered_certs {
        // Fetch certificate fields
        let fields = storage
            .find_certificate_fields(
                &FindCertificateFieldsArgs {
                    partial: CertificateFieldPartial {
                        certificate_id: Some(cert.certificate_id),
                        user_id: Some(user_id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                trx,
            )
            .await?;

        let field_map: HashMap<String, String> = fields
            .iter()
            .map(|f| (f.field_name.clone(), f.field_value.clone()))
            .collect();

        let keyring: HashMap<String, String> = fields
            .iter()
            .map(|f| (f.field_name.clone(), f.master_key.clone()))
            .collect();

        // Convert storage certificate to SDK types
        let subject = PublicKey::from_string(&cert.subject).map_err(|e| {
            WalletError::Internal(format!(
                "Invalid subject key in certificate {}: {}",
                cert.certificate_id, e
            ))
        })?;

        let certifier_pk = PublicKey::from_string(&cert.certifier).map_err(|e| {
            WalletError::Internal(format!(
                "Invalid certifier key in certificate {}: {}",
                cert.certificate_id, e
            ))
        })?;

        let cert_type =
            CertificateType::from_string(&cert.cert_type).unwrap_or(CertificateType([0u8; 32]));

        let serial_number =
            SerialNumber::from_string(&cert.serial_number).unwrap_or(SerialNumber([0u8; 32]));

        let signature =
            hex_decode(&cert.signature).or_else(|| Some(cert.signature.as_bytes().to_vec()));

        let sdk_cert = SdkCertificate {
            cert_type,
            serial_number,
            subject,
            certifier: certifier_pk,
            revocation_outpoint: Some(cert.revocation_outpoint.clone()),
            fields: Some(field_map),
            signature,
        };

        let verifier = cert
            .verifier
            .as_ref()
            .and_then(|v| hex_decode(v))
            .or_else(|| cert.verifier.as_ref().map(|v| v.as_bytes().to_vec()));

        certificates.push(CertificateResult {
            certificate: sdk_cert,
            keyring: Some(keyring),
            verifier,
        });
    }

    let total_certificates = if filtered_certs.len() < limit as usize {
        (offset as u32) + filtered_certs.len() as u32
    } else {
        let count_args = FindCertificatesArgs {
            partial: CertificatePartial {
                user_id: Some(user_id),
                is_deleted: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        storage.count_certificates(&count_args, trx).await? as u32
    };

    Ok(ListCertificatesResult {
        total_certificates,
        certificates,
    })
}

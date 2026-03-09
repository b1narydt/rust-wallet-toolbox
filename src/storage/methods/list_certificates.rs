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

/// Decode a hex string to bytes. Returns None on invalid hex.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
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

    let find_args = FindCertificatesArgs {
        partial: CertificatePartial {
            user_id: Some(user_id),
            is_deleted: Some(false),
            ..Default::default()
        },
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
            keyring,
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

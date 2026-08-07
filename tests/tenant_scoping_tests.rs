//! Cross-tenant refusal tests for the authenticated storage surface.
//!
//! Two users share one database. Every test drives a `WalletStorageProvider`
//! `*_auth` method with user A's `AuthId` against rows seeded for user B and
//! asserts B's rows are neither returned nor mutated. Happy-path assertions
//! exist only to prove the refusals are not vacuous.

#[cfg(feature = "sqlite")]
mod tenant_scoping {
    use chrono::NaiveDateTime;

    use bsv::primitives::public_key::PublicKey;
    use bsv::wallet::interfaces::{
        CertificateType, RelinquishCertificateArgs, RelinquishOutputArgs, SerialNumber,
    };
    use bsv_wallet_toolbox::error::{WalletError, WalletResult};
    use bsv_wallet_toolbox::status::TransactionStatus;
    use bsv_wallet_toolbox::storage::find_args::*;
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
    use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
    use bsv_wallet_toolbox::storage::traits::wallet_provider::WalletStorageProvider;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::tables::*;
    use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};
    use bsv_wallet_toolbox::wallet::types::AuthId;

    const KEY_A: &str = "02aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const KEY_B: &str = "02bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TXID_A: &str = "aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11";
    const TXID_B: &str = "bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22";
    // secp256k1 generator point: a parseable compressed public key.
    const CERTIFIER_HEX: &str =
        "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    fn now() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-01-15 10:30:00", "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn auth(identity_key: &str) -> AuthId {
        AuthId {
            identity_key: identity_key.to_string(),
            user_id: None,
            is_active: None,
        }
    }

    /// 32-byte field padded with zeros, matching how the relinquish path
    /// stringifies CertificateType/SerialNumber (trailing NULs trimmed).
    fn bytes32(s: &str) -> [u8; 32] {
        let mut buf = [0u8; 32];
        buf[..s.len()].copy_from_slice(s.as_bytes());
        buf
    }

    struct Fixture {
        storage: SqliteStorage,
        user_a: i64,
        user_b: i64,
        basket_b: i64,
        cert_b: i64,
    }

    /// One database, two tenants. Each user owns a basket named "shared", one
    /// transaction, one output (B's carries full BRC-42 derivation
    /// coordinates), and one certificate; B's certificate triple is what A's
    /// cross-tenant attempts target.
    async fn setup() -> WalletResult<Fixture> {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test).await?;
        storage.migrate_database().await?;

        let (a, _) = StorageReaderWriter::find_or_insert_user(&storage, KEY_A, None).await?;
        let (b, _) = StorageReaderWriter::find_or_insert_user(&storage, KEY_B, None).await?;

        let mut basket_ids = Vec::new();
        for (user_id, txid, sender) in [(a.user_id, TXID_A, None), (b.user_id, TXID_B, Some(KEY_A))]
        {
            let basket_id = StorageReaderWriter::insert_output_basket(
                &storage,
                &OutputBasket {
                    created_at: now(),
                    updated_at: now(),
                    basket_id: 0,
                    user_id,
                    name: "shared".to_string(),
                    number_of_desired_utxos: 6,
                    minimum_desired_utxo_value: 1000,
                    is_deleted: false,
                },
                None,
            )
            .await?;
            basket_ids.push(basket_id);
            let tx_id = StorageReaderWriter::insert_transaction(
                &storage,
                &Transaction {
                    created_at: now(),
                    updated_at: now(),
                    transaction_id: 0,
                    user_id,
                    proven_tx_id: None,
                    status: TransactionStatus::Completed,
                    reference: format!("ref-{user_id}"),
                    is_outgoing: false,
                    satoshis: 4000,
                    description: "seed".to_string(),
                    version: Some(1),
                    lock_time: Some(0),
                    txid: Some(txid.to_string()),
                    input_beef: None,
                    raw_tx: None,
                },
                None,
            )
            .await?;
            StorageReaderWriter::insert_output(
                &storage,
                &Output {
                    created_at: now(),
                    updated_at: now(),
                    output_id: 0,
                    user_id,
                    transaction_id: tx_id,
                    basket_id: Some(basket_id),
                    spendable: true,
                    change: false,
                    output_description: None,
                    vout: 0,
                    satoshis: 4000,
                    provided_by: StorageProvidedBy::Storage,
                    purpose: "seed".to_string(),
                    output_type: "P2PKH".to_string(),
                    txid: Some(txid.to_string()),
                    sender_identity_key: sender.map(str::to_string),
                    derivation_prefix: sender.map(|_| "prefixB==".to_string()),
                    derivation_suffix: sender.map(|_| "suffixB==".to_string()),
                    custom_instructions: None,
                    spent_by: None,
                    sequence_number: None,
                    spending_description: None,
                    script_length: None,
                    script_offset: None,
                    locking_script: None,
                },
                None,
            )
            .await?;
        }

        let mut cert_ids = Vec::new();
        for (user_id, serial) in [(a.user_id, "SN-A-1"), (b.user_id, "SN-B-1")] {
            let cert_id = StorageReaderWriter::insert_certificate(
                &storage,
                &Certificate {
                    created_at: now(),
                    updated_at: now(),
                    certificate_id: 0,
                    user_id,
                    cert_type: "identity".to_string(),
                    serial_number: serial.to_string(),
                    certifier: CERTIFIER_HEX.to_string(),
                    subject: "subject".to_string(),
                    verifier: None,
                    revocation_outpoint: "outpoint:0".to_string(),
                    signature: "sig".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await?;
            cert_ids.push(cert_id);
        }

        Ok(Fixture {
            storage,
            user_a: a.user_id,
            user_b: b.user_id,
            basket_b: basket_ids[1],
            cert_b: cert_ids[1],
        })
    }

    fn assert_unauthorized<T: std::fmt::Debug>(result: WalletResult<T>) {
        match result {
            Err(WalletError::Unauthorized(_)) => {}
            other => panic!("expected WERR_UNAUTHORIZED, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // find_outputs_auth
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn find_outputs_auth_scopes_to_caller() {
        let f = setup().await.unwrap();
        let outputs = WalletStorageProvider::find_outputs_auth(
            &f.storage,
            &auth(KEY_A),
            &FindOutputsArgs::default(),
        )
        .await
        .unwrap();
        assert_eq!(outputs.len(), 1, "A must see exactly A's output");
        assert_eq!(outputs[0].user_id, f.user_a);
        // B's BRC-42 derivation coordinates must never surface for A.
        assert!(outputs.iter().all(|o| o.derivation_prefix.is_none()));
        assert!(outputs.iter().all(|o| o.txid.as_deref() != Some(TXID_B)));
    }

    #[tokio::test]
    async fn find_outputs_auth_rejects_foreign_user_id_filter() {
        let f = setup().await.unwrap();
        let args = FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(f.user_b),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_unauthorized(
            WalletStorageProvider::find_outputs_auth(&f.storage, &auth(KEY_A), &args).await,
        );
    }

    #[tokio::test]
    async fn find_outputs_auth_treats_zero_user_id_as_unset() {
        let f = setup().await.unwrap();
        let args = FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(0),
                ..Default::default()
            },
            ..Default::default()
        };
        let outputs = WalletStorageProvider::find_outputs_auth(&f.storage, &auth(KEY_A), &args)
            .await
            .unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].user_id, f.user_a);
    }

    #[tokio::test]
    async fn auth_user_id_claim_must_match_identity_key() {
        let f = setup().await.unwrap();
        let mut forged = auth(KEY_A);
        forged.user_id = Some(f.user_b);
        assert_unauthorized(
            WalletStorageProvider::find_outputs_auth(
                &f.storage,
                &forged,
                &FindOutputsArgs::default(),
            )
            .await,
        );
    }

    // -------------------------------------------------------------------
    // find_output_baskets_auth
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn find_output_baskets_auth_scopes_to_caller() {
        let f = setup().await.unwrap();
        // Both users own a basket named "shared"; the name filter alone must
        // not cross the tenant line.
        let args = FindOutputBasketsArgs {
            partial: OutputBasketPartial {
                name: Some("shared".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let baskets =
            WalletStorageProvider::find_output_baskets_auth(&f.storage, &auth(KEY_A), &args)
                .await
                .unwrap();
        assert_eq!(baskets.len(), 1, "A must see exactly A's basket");
        assert_eq!(baskets[0].user_id, f.user_a);
    }

    #[tokio::test]
    async fn find_output_baskets_auth_rejects_foreign_user_id_filter() {
        let f = setup().await.unwrap();
        let args = FindOutputBasketsArgs {
            partial: OutputBasketPartial {
                user_id: Some(f.user_b),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_unauthorized(
            WalletStorageProvider::find_output_baskets_auth(&f.storage, &auth(KEY_A), &args).await,
        );
    }

    // -------------------------------------------------------------------
    // find_certificates_auth
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn find_certificates_auth_scopes_to_caller() {
        let f = setup().await.unwrap();
        // Same certifier + type on both rows; only the serial differs.
        let certs = WalletStorageProvider::find_certificates_auth(
            &f.storage,
            &auth(KEY_A),
            &FindCertificatesArgs::default(),
        )
        .await
        .unwrap();
        assert_eq!(certs.len(), 1, "A must see exactly A's certificate");
        assert_eq!(certs[0].user_id, f.user_a);
        assert_eq!(certs[0].serial_number, "SN-A-1");
    }

    #[tokio::test]
    async fn find_certificates_auth_rejects_foreign_user_id_filter() {
        let f = setup().await.unwrap();
        let args = FindCertificatesArgs {
            partial: CertificatePartial {
                user_id: Some(f.user_b),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_unauthorized(
            WalletStorageProvider::find_certificates_auth(&f.storage, &auth(KEY_A), &args).await,
        );
    }

    // -------------------------------------------------------------------
    // insert_certificate_auth
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn insert_certificate_auth_rejects_foreign_owner() {
        let f = setup().await.unwrap();
        let cert = Certificate {
            created_at: now(),
            updated_at: now(),
            certificate_id: 0,
            user_id: f.user_b,
            cert_type: "identity".to_string(),
            serial_number: "SN-FORGED".to_string(),
            certifier: CERTIFIER_HEX.to_string(),
            subject: "subject".to_string(),
            verifier: None,
            revocation_outpoint: "outpoint:0".to_string(),
            signature: "sig".to_string(),
            is_deleted: false,
        };
        assert_unauthorized(
            WalletStorageProvider::insert_certificate_auth(&f.storage, &auth(KEY_A), &cert).await,
        );
        // Nothing landed in B's rows.
        let b_certs = StorageReader::find_certificates(
            &f.storage,
            &FindCertificatesArgs {
                partial: CertificatePartial {
                    user_id: Some(f.user_b),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
        assert_eq!(b_certs.len(), 1);
    }

    #[tokio::test]
    async fn insert_certificate_auth_owns_row_as_caller() {
        let f = setup().await.unwrap();
        let cert = Certificate {
            created_at: now(),
            updated_at: now(),
            certificate_id: 0,
            user_id: 0, // unset: resolves to the caller
            cert_type: "identity".to_string(),
            serial_number: "SN-A-2".to_string(),
            certifier: CERTIFIER_HEX.to_string(),
            subject: "subject".to_string(),
            verifier: None,
            revocation_outpoint: "outpoint:0".to_string(),
            signature: "sig".to_string(),
            is_deleted: false,
        };
        let id = WalletStorageProvider::insert_certificate_auth(&f.storage, &auth(KEY_A), &cert)
            .await
            .unwrap();
        let stored = StorageReader::find_certificate_by_id(&f.storage, id, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.user_id, f.user_a);
    }

    // -------------------------------------------------------------------
    // relinquish_certificate
    // -------------------------------------------------------------------

    fn relinquish_cert_args(serial: &str) -> RelinquishCertificateArgs {
        RelinquishCertificateArgs {
            cert_type: CertificateType(bytes32("identity")),
            serial_number: SerialNumber(bytes32(serial)),
            certifier: PublicKey::from_string(CERTIFIER_HEX).unwrap(),
        }
    }

    #[tokio::test]
    async fn relinquish_certificate_cannot_touch_foreign_certificate() {
        let f = setup().await.unwrap();
        // A targets B's (certifier, serial, type) triple: must fail as
        // "not found", and B's certificate must stay live.
        let result = WalletStorageProvider::relinquish_certificate(
            &f.storage,
            &auth(KEY_A),
            &relinquish_cert_args("SN-B-1"),
        )
        .await;
        assert!(result.is_err(), "cross-tenant relinquish must not succeed");
        let b_cert = StorageReader::find_certificate_by_id(&f.storage, f.cert_b, None)
            .await
            .unwrap()
            .unwrap();
        assert!(!b_cert.is_deleted, "B's certificate must remain live");
    }

    #[tokio::test]
    async fn relinquish_certificate_soft_deletes_own_certificate() {
        let f = setup().await.unwrap();
        let n = WalletStorageProvider::relinquish_certificate(
            &f.storage,
            &auth(KEY_B),
            &relinquish_cert_args("SN-B-1"),
        )
        .await
        .unwrap();
        assert_eq!(n, 1);
        let b_cert = StorageReader::find_certificate_by_id(&f.storage, f.cert_b, None)
            .await
            .unwrap()
            .unwrap();
        assert!(b_cert.is_deleted);
    }

    // -------------------------------------------------------------------
    // relinquish_output
    // -------------------------------------------------------------------

    fn relinquish_output_args(txid: &str) -> RelinquishOutputArgs {
        RelinquishOutputArgs {
            basket: "shared".to_string(),
            output: format!("{txid}.0"),
        }
    }

    async fn output_basket_of(storage: &SqliteStorage, user_id: i64) -> Option<i64> {
        let outputs = StorageReader::find_outputs(
            storage,
            &FindOutputsArgs {
                partial: OutputPartial {
                    user_id: Some(user_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
        assert_eq!(outputs.len(), 1);
        outputs[0].basket_id
    }

    #[tokio::test]
    async fn relinquish_output_cannot_touch_foreign_output() {
        let f = setup().await.unwrap();
        // A targets B's outpoint: must fail as "not found", and B's output
        // must keep its basket.
        let result = WalletStorageProvider::relinquish_output(
            &f.storage,
            &auth(KEY_A),
            &relinquish_output_args(TXID_B),
        )
        .await;
        assert!(result.is_err(), "cross-tenant relinquish must not succeed");
        assert_eq!(
            output_basket_of(&f.storage, f.user_b).await,
            Some(f.basket_b),
            "B's output must keep its basket"
        );
    }

    #[tokio::test]
    async fn relinquish_output_debaskets_own_output() {
        let f = setup().await.unwrap();
        let n = WalletStorageProvider::relinquish_output(
            &f.storage,
            &auth(KEY_B),
            &relinquish_output_args(TXID_B),
        )
        .await
        .unwrap();
        assert_eq!(n, 1);
        assert_ne!(
            output_basket_of(&f.storage, f.user_b).await,
            Some(f.basket_b),
            "B's own relinquish must clear the basket"
        );
    }
}

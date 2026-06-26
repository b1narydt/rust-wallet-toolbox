//! Regression test for the relinquish_output FK fix.
//!
//! Asserts that after `WalletStorageProvider::relinquish_output`, the
//! output row's `basketId` is NULL (canonical TS + Go behaviour).
//!
//! Previously the storage path wrote `basket_id: Some(0)` which violated
//! `outputs.basketId -> baskets.basketId` (no basket has id 0). The fix
//! routes through a new dedicated `clear_output_basket` storage method
//! that issues `UPDATE outputs SET basketId = NULL WHERE outputId = ?`.

#[cfg(feature = "sqlite")]
mod relinquish_output_tests {
    use bsv::wallet::interfaces::RelinquishOutputArgs;

    use bsv_wallet_toolbox::error::WalletResult;
    use bsv_wallet_toolbox::status::TransactionStatus;
    use bsv_wallet_toolbox::storage::find_args::{FindOutputsArgs, OutputPartial};
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
    use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
    use bsv_wallet_toolbox::storage::traits::wallet_provider::WalletStorageProvider;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::tables::{Output, OutputBasket, Transaction, User};
    use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};
    use bsv_wallet_toolbox::wallet::types::AuthId;
    use chrono::NaiveDateTime;

    async fn setup_test_storage() -> WalletResult<SqliteStorage> {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test).await?;
        storage.migrate_database().await?;
        Ok(storage)
    }

    fn test_datetime() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2024-01-15 10:30:00", "%Y-%m-%d %H:%M:%S").unwrap()
    }

    async fn seed_output_in_basket(
        storage: &SqliteStorage,
    ) -> WalletResult<(i64, i64, String, String)> {
        let now = test_datetime();
        let identity_key = "02relinquish_test_identity".to_string();
        let user_id = StorageReaderWriter::insert_user(
            storage,
            &User {
                created_at: now,
                updated_at: now,
                user_id: 0,
                identity_key: identity_key.clone(),
                active_storage: String::new(),
            },
            None,
        )
        .await?;

        let basket_id = StorageReaderWriter::insert_output_basket(
            storage,
            &OutputBasket {
                created_at: now,
                updated_at: now,
                basket_id: 0,
                user_id,
                name: "relinquish-test-basket".to_string(),
                number_of_desired_utxos: 6,
                minimum_desired_utxo_value: 10_000,
                is_deleted: false,
            },
            None,
        )
        .await?;

        let txid = "a".repeat(64);
        let tx_id = StorageReaderWriter::insert_transaction(
            storage,
            &Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id,
                proven_tx_id: None,
                status: TransactionStatus::Completed,
                reference: "ref-relinquish".to_string(),
                is_outgoing: false,
                satoshis: 100_000,
                description: "Relinquish regression test tx".to_string(),
                version: None,
                lock_time: None,
                txid: Some(txid.clone()),
                input_beef: None,
                raw_tx: None,
            },
            None,
        )
        .await?;

        let output_id = StorageReaderWriter::insert_output(
            storage,
            &Output {
                created_at: now,
                updated_at: now,
                output_id: 0,
                user_id,
                transaction_id: tx_id,
                basket_id: Some(basket_id),
                spendable: true,
                change: false,
                output_description: None,
                vout: 0,
                satoshis: 50_000,
                provided_by: StorageProvidedBy::Storage,
                purpose: "test".to_string(),
                output_type: "P2PKH".to_string(),
                txid: Some(txid.clone()),
                sender_identity_key: None,
                derivation_prefix: None,
                derivation_suffix: None,
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

        Ok((output_id, basket_id, identity_key, txid))
    }

    #[tokio::test]
    async fn relinquish_output_clears_basket() {
        let storage = setup_test_storage().await.unwrap();
        let (output_id, basket_id, identity_key, txid) =
            seed_output_in_basket(&storage).await.unwrap();

        let pre_args = FindOutputsArgs {
            partial: OutputPartial { output_id: Some(output_id), ..Default::default() },
            ..Default::default()
        };
        let pre = StorageReader::find_outputs(&storage, &pre_args, None).await.unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0].basket_id, Some(basket_id));
        assert!(pre[0].spendable);

        let auth = AuthId { identity_key, user_id: None, is_active: None };
        let relinquish_args = RelinquishOutputArgs {
            basket: "relinquish-test-basket".to_string(),
            output: format!("{txid}.0"),
        };
        let rows = WalletStorageProvider::relinquish_output(&storage, &auth, &relinquish_args)
            .await
            .expect("relinquish_output must succeed");
        assert_eq!(rows, 1);

        let post = StorageReader::find_outputs(&storage, &pre_args, None).await.unwrap();
        assert_eq!(post.len(), 1);
        assert_eq!(
            post[0].basket_id, None,
            "after relinquish_output, basketId MUST be NULL (canonical TS + Go semantics)"
        );
    }
}

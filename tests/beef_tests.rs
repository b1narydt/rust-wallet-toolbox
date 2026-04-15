//! BEEF construction integration tests.
//!
//! Tests verify get_valid_beef_for_txid behavior including proven transaction
//! lookup, recursive input chain walking, and known_txids optimization.
//!
//! All tests use in-memory SQLite databases and are gated with `#[cfg(feature = "sqlite")]`.
//!
//! Note: bsv-sdk Beef serialization requires valid 64-char hex txids. Tests use
//! properly formatted txids to avoid serialization errors.

#[cfg(feature = "sqlite")]
mod beef_tests {
    use std::collections::HashSet;

    use chrono::NaiveDateTime;

    use bsv_wallet_toolbox::error::WalletResult;
    use bsv_wallet_toolbox::status::TransactionStatus;
    use bsv_wallet_toolbox::storage::beef::{get_valid_beef_for_txid, TrustSelf};
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::tables::*;
    use bsv_wallet_toolbox::types::Chain;

    // Valid 64-char hex txids for bsv-sdk compatibility
    const TXID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TXID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TXID_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const TXID_UNKNOWN: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    /// Helper to create a fresh in-memory SQLite storage with migrations.
    async fn setup_storage() -> WalletResult<SqliteStorage> {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test).await?;
        storage.migrate_database().await?;
        storage.make_available().await?;
        Ok(storage)
    }

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    /// Insert a user and return the local user_id.
    async fn insert_test_user(storage: &SqliteStorage, identity_key: &str) -> i64 {
        let now = dt("2024-01-15 10:00:00");
        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: identity_key.to_string(),
            active_storage: "default".to_string(),
        };
        storage.insert_user(&user, None).await.unwrap()
    }

    // -----------------------------------------------------------------------
    // Test 1: get_valid_beef_for_txid returns None for unknown txid
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_beef_unknown_txid_returns_none() {
        let storage = setup_storage().await.unwrap();
        let _user_id = insert_test_user(&storage, "02beef01").await;

        let known_txids = HashSet::new();
        let result = get_valid_beef_for_txid(&storage, TXID_UNKNOWN, TrustSelf::No, &known_txids)
            .await
            .unwrap();

        assert!(result.is_none(), "unknown txid should return None");
    }

    // -----------------------------------------------------------------------
    // Test 2: known_txids optimization -- a txid in known_txids set that is
    //         not in storage but looked up as root gets added as txid-only
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_beef_known_txid_produces_result() {
        let storage = setup_storage().await.unwrap();
        let _user_id = insert_test_user(&storage, "02beef02").await;

        // TXID_A is in known_txids but not in storage at all.
        // When collect_tx_recursive is called with TXID_A:
        //   - Not in ProvenTx -> not in Transaction -> but IS in known_txids
        //   -> added as txid-only entry -> collected is non-empty -> BEEF built
        let mut known_txids = HashSet::new();
        known_txids.insert(TXID_A.to_string());

        let result = get_valid_beef_for_txid(&storage, TXID_A, TrustSelf::No, &known_txids)
            .await
            .unwrap();

        assert!(
            result.is_some(),
            "known txid should produce BEEF bytes (txid-only entry)"
        );
        let bytes = result.unwrap();
        assert!(!bytes.is_empty(), "BEEF bytes should not be empty");
    }

    // -----------------------------------------------------------------------
    // Test 3: TrustSelf::Known makes proven transactions txid-only
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_beef_trust_self_known_returns_txid_only() {
        let storage = setup_storage().await.unwrap();
        let _user_id = insert_test_user(&storage, "02beef03").await;
        let now = dt("2024-01-15 11:00:00");

        // Insert a ProvenTx with a valid hex txid
        let proven = ProvenTx {
            created_at: now,
            updated_at: now,
            proven_tx_id: 0,
            txid: TXID_B.to_string(),
            height: 800000,
            index: 5,
            merkle_path: vec![0x01, 0x02, 0x03],
            raw_tx: vec![0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            block_hash: "00000000000000000004block".to_string(),
            merkle_root: "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
                .to_string(),
        };
        storage.insert_proven_tx(&proven, None).await.unwrap();

        let known_txids = HashSet::new();

        // With TrustSelf::Known, proven txs are included as txid-only
        // (no raw_tx parsing, no merkle_path parsing needed)
        let result =
            get_valid_beef_for_txid(&storage, TXID_B, TrustSelf::Known, &known_txids).await;

        assert!(
            result.is_ok(),
            "TrustSelf::Known should not fail: {:?}",
            result.err()
        );
        let bytes = result.unwrap();
        assert!(
            bytes.is_some(),
            "should produce BEEF bytes for trusted proven tx"
        );
        assert!(!bytes.unwrap().is_empty(), "BEEF bytes should not be empty");
    }

    // -----------------------------------------------------------------------
    // Test 4: Transaction with input_beef merges ancestor proofs into output
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_beef_input_beef_is_merged_into_output() {
        use bsv::transaction::beef::{Beef, BEEF_V2};
        use bsv::transaction::beef_tx::BeefTx;
        use bsv::transaction::transaction::Transaction as BsvTransaction;
        use std::io::Cursor;

        let storage = setup_storage().await.unwrap();
        let user_id = insert_test_user(&storage, "02beef05").await;
        let now = dt("2024-01-15 11:00:00");

        // Build a minimal valid BEEF to serve as the stored inputBEEF.
        // It contains TXID_A as a txid-only entry — simulating an ancestor proof chain.
        let ancestor_beef_bytes = {
            let mut beef = Beef::new(BEEF_V2);
            let beef_tx = BeefTx::from_txid(TXID_A.to_string());
            beef.txs.push(beef_tx);
            let mut buf = Vec::new();
            beef.to_binary(&mut buf).expect("BEEF serialization");
            buf
        };

        // Minimal valid raw_tx (version + 0 inputs + 0 outputs + locktime).
        // This parses successfully and has no inputs, so no recursion occurs.
        let minimal_raw_tx = vec![
            0x01, 0x00, 0x00, 0x00, // version 1
            0x00, // 0 inputs
            0x00, // 0 outputs
            0x00, 0x00, 0x00, 0x00, // locktime 0
        ];

        // Compute the txid of our minimal raw_tx
        let txid_c = {
            let bsv_tx = BsvTransaction::from_binary(&mut Cursor::new(&minimal_raw_tx)).unwrap();
            bsv_tx.id().expect("txid")
        };

        // Insert a Transaction with raw_tx and input_beef populated
        let tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id,
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: "ref-input-beef".to_string(),
            is_outgoing: true,
            satoshis: 1000,
            description: "tx with inputBEEF".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some(txid_c.clone()),
            input_beef: Some(ancestor_beef_bytes),
            raw_tx: Some(minimal_raw_tx),
        };
        storage.insert_transaction(&tx, None).await.unwrap();

        let known_txids = HashSet::new();
        let result = get_valid_beef_for_txid(&storage, &txid_c, TrustSelf::No, &known_txids)
            .await
            .unwrap();

        assert!(
            result.is_some(),
            "should produce BEEF for tx with inputBEEF"
        );

        // Deserialize the output BEEF and verify TXID_A from the inputBEEF was merged in
        let output_bytes = result.unwrap();
        let parsed = Beef::from_binary(&mut Cursor::new(&output_bytes))
            .expect("output BEEF should be parseable");

        let txids: Vec<&str> = parsed.txs.iter().map(|t| t.txid.as_str()).collect();
        assert!(
            txids.contains(&TXID_A),
            "output BEEF should contain TXID_A from merged inputBEEF, got: {:?}",
            txids
        );
        assert!(
            txids.contains(&txid_c.as_str()),
            "output BEEF should contain the main transaction txid"
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: Transaction without raw_tx and not in known_txids returns None
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_beef_tx_without_raw_tx_returns_none() {
        let storage = setup_storage().await.unwrap();
        let user_id = insert_test_user(&storage, "02beef04").await;
        let now = dt("2024-01-15 11:00:00");

        // Insert a Transaction without raw_tx (no ProvenTx entry either)
        let tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id,
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: "ref-no-rawtx".to_string(),
            is_outgoing: true,
            satoshis: 1000,
            description: "no raw_tx".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some(TXID_C.to_string()),
            input_beef: None,
            raw_tx: None, // no raw_tx
        };
        storage.insert_transaction(&tx, None).await.unwrap();

        let known_txids = HashSet::new();
        let result = get_valid_beef_for_txid(&storage, TXID_C, TrustSelf::No, &known_txids)
            .await
            .unwrap();

        assert!(
            result.is_none(),
            "transaction without raw_tx and not in known_txids should return None"
        );
    }
}

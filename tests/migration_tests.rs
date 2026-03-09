//! SQLite migration integration tests.
//!
//! Verifies that migrations run successfully against an in-memory SQLite database
//! and that all 16 tables are created with the expected schema.

use sqlx::Row;
use sqlx::SqlitePool;

use bsv_wallet_toolbox::migrations::run_sqlite_migrations;

#[tokio::test]
async fn sqlite_migrations_run_successfully() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    run_sqlite_migrations(&pool).await.unwrap();
}

#[tokio::test]
async fn sqlite_migrations_create_all_16_tables() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    run_sqlite_migrations(&pool).await.unwrap();

    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE '_sqlx%' AND name NOT LIKE 'sqlite_%' ORDER BY name"
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    let table_names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();

    let expected_tables = [
        "certificate_fields",
        "certificates",
        "commissions",
        "monitor_events",
        "output_baskets",
        "output_tags",
        "output_tags_map",
        "outputs",
        "proven_tx_reqs",
        "proven_txs",
        "settings",
        "sync_states",
        "transactions",
        "tx_labels",
        "tx_labels_map",
        "users",
    ];

    for expected in &expected_tables {
        assert!(
            table_names.contains(expected),
            "Table '{}' not found. Found tables: {:?}",
            expected,
            table_names
        );
    }
    assert_eq!(
        table_names.len(),
        16,
        "Expected 16 tables, found {}: {:?}",
        table_names.len(),
        table_names
    );
}

#[tokio::test]
async fn sqlite_insert_select_user() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    run_sqlite_migrations(&pool).await.unwrap();

    sqlx::query("INSERT INTO users (identityKey, activeStorage) VALUES (?, ?)")
        .bind("02abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab")
        .bind("03storagekey")
        .execute(&pool)
        .await
        .unwrap();

    let row = sqlx::query("SELECT userId, identityKey, activeStorage FROM users WHERE userId = 1")
        .fetch_one(&pool)
        .await
        .unwrap();

    let user_id: i64 = row.get("userId");
    let identity_key: String = row.get("identityKey");
    let active_storage: String = row.get("activeStorage");

    assert_eq!(user_id, 1);
    assert_eq!(
        identity_key,
        "02abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab"
    );
    assert_eq!(active_storage, "03storagekey");
}

#[tokio::test]
async fn sqlite_insert_select_transaction_with_fk() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    run_sqlite_migrations(&pool).await.unwrap();

    // Insert user first (FK dependency)
    sqlx::query("INSERT INTO users (identityKey, activeStorage) VALUES (?, ?)")
        .bind("02user1")
        .bind("storage1")
        .execute(&pool)
        .await
        .unwrap();

    // Insert transaction referencing user
    sqlx::query(
        "INSERT INTO transactions (userId, status, reference, isOutgoing, satoshis, description) VALUES (?, ?, ?, ?, ?, ?)"
    )
    .bind(1i64)
    .bind("completed")
    .bind("ref-001")
    .bind(true)
    .bind(50000i64)
    .bind("Test transaction")
    .execute(&pool)
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT transactionId, userId, status, reference, isOutgoing, satoshis, description FROM transactions WHERE transactionId = 1"
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let tx_id: i64 = row.get("transactionId");
    let user_id: i64 = row.get("userId");
    let status: String = row.get("status");
    let reference: String = row.get("reference");
    let is_outgoing: bool = row.get("isOutgoing");
    let satoshis: i64 = row.get("satoshis");
    let description: String = row.get("description");

    assert_eq!(tx_id, 1);
    assert_eq!(user_id, 1);
    assert_eq!(status, "completed");
    assert_eq!(reference, "ref-001");
    assert!(is_outgoing);
    assert_eq!(satoshis, 50000);
    assert_eq!(description, "Test transaction");
}

#[tokio::test]
async fn sqlite_proven_tx_index_column_works() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    run_sqlite_migrations(&pool).await.unwrap();

    // Insert proven_tx with the reserved word "index" column
    sqlx::query(
        r#"INSERT INTO proven_txs (txid, height, "index", merklePath, rawTx, blockHash, merkleRoot) VALUES (?, ?, ?, ?, ?, ?, ?)"#
    )
    .bind("txid123")
    .bind(800000i32)
    .bind(5i32)
    .bind(vec![0x01u8, 0x02])
    .bind(vec![0x03u8, 0x04])
    .bind("blockhash")
    .bind("merkleroot")
    .execute(&pool)
    .await
    .unwrap();

    let row = sqlx::query(r#"SELECT provenTxId, txid, height, "index", blockHash FROM proven_txs WHERE provenTxId = 1"#)
        .fetch_one(&pool)
        .await
        .unwrap();

    let index: i32 = row.get("index");
    assert_eq!(index, 5);
}

#[tokio::test]
async fn sqlite_key_columns_exist() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    run_sqlite_migrations(&pool).await.unwrap();

    // Spot-check: query pragma for outputs table columns
    let rows: Vec<(String,)> = sqlx::query_as("SELECT name FROM pragma_table_info('outputs')")
        .fetch_all(&pool)
        .await
        .unwrap();

    let col_names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();

    let expected_cols = [
        "outputId",
        "userId",
        "transactionId",
        "basketId",
        "spendable",
        "change",
        "vout",
        "satoshis",
        "providedBy",
        "purpose",
        "type",
        "outputDescription",
        "txid",
        "lockingScript",
        "spentBy",
        "scriptLength",
        "scriptOffset",
    ];

    for expected in &expected_cols {
        assert!(
            col_names.contains(expected),
            "Column '{}' not found in outputs table. Found: {:?}",
            expected,
            col_names
        );
    }
}

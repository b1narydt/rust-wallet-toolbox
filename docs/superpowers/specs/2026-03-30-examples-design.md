# Wallet Examples Design Spec

**Date:** 2026-03-30
**Location:** `examples/*.rs`
**Branch:** `feat/examples`

## Summary

Create 11 runnable Cargo examples demonstrating the full capabilities of `bsv-wallet-toolbox`: wallet setup with funding, local chaintracks header sync and SPV validation, P2PKH and BRC29 transfers, balance/action queries, PushDrop tokens, noSend batching, payment internalization, and wallet backup. Built in three phases, each producing working examples.

## Conventions

- **File pattern:** `examples/<name>.rs` — run via `cargo run --example <name>`
- **Entry point:** `#[tokio::main] async fn main() -> Result<(), Box<dyn std::error::Error>>`
- **Self-contained:** Each example includes its own setup code (no shared helper module)
- **Doc comment:** Each file starts with `//!` doc comments explaining what it does and how to run it
- **Chain selection:** Defaults to testnet. Set `BSV_CHAIN=main` env var for mainnet. Parse "main"→`Chain::Main`, "test"→`Chain::Test` (default).
- **Key management:** Private keys read from `BSV_PRIVATE_KEY` and `BSV_PRIVATE_KEY_2` env vars (hex-encoded). `setup_wallet` generates a key if none is set.
- **Two-wallet examples:** P2PKH, BRC29, and internalize examples require both key env vars. Each wallet uses a separate SQLite file (e.g., `sender.db`, `receiver.db`) to avoid contention.
- **Feature flags:** Examples require the default `sqlite` feature. `cargo run --example <name>` works out of the box.
- **Output:** Clear step-by-step stdout output with labeled values.
- **SDK imports:** Examples use types from both `bsv_wallet_toolbox` and `bsv` (the SDK). Key imports: `bsv::primitives::private_key::PrivateKey`, `bsv::wallet::interfaces::{WalletInterface, CreateActionArgs, InternalizeActionArgs, ListOutputsArgs, ListActionsArgs}`, `bsv::script::templates::p2pkh::P2PKH`, `bsv::script::templates::push_drop::PushDrop`.

## Phase 1: Setup & Chaintracks

### `setup_wallet.rs`

Entry point for new users. Generates or imports a private key, builds a wallet, displays the P2PKH address for faucet funding, and checks balance.

1. Check `BSV_PRIVATE_KEY` env var — if not set, generate a random key via `PrivateKey::from_random()` and print it with instructions to save it
2. Build wallet: `WalletBuilder::new().chain(chain).root_key(key).with_sqlite("wallet.db").with_default_services().build().await`
3. Derive and print the wallet's identity key from `setup.identity_key`
4. Derive P2PKH address: use `PublicKey::to_address()` from bsv-sdk with prefix `0x6f` for testnet or `0x00` for mainnet. Print the address.
5. Print instructions: "Send testnet BSV to this address using a faucet"
6. Query balance via `wallet.balance(None)` — this returns the total satoshi value of spendable outputs
7. If balance > 0, print "Wallet funded! Ready to run other examples."

### `chaintracks_sync.rs`

Demonstrates local block header synchronization using the chaintracks module.

1. Create `MemoryStorage::new(chain)` for the selected chain
2. Create `Chaintracks::new(ChaintracksOptions::default_testnet(), storage)`
3. Call `ct.make_available().await`
4. Fetch headers using `BulkWocIngestor`: call `ingestor.get_recent_headers().await` (public method) to get recent block headers, then feed each to chaintracks via `ct.add_header(base_header).await`
5. Call `ct.process_pending_headers().await` to process the queue
6. Print chain tip height and hash via `ct.find_chain_tip_header().await`
7. Subscribe to new headers via `ct.subscribe_headers(callback)` and print arrivals (with a 30-second timeout)

### `chaintracks_validate.rs`

Demonstrates SPV merkle root validation against locally synced headers.

1. Set up local chaintracks and sync headers (same pattern as chaintracks_sync, abbreviated)
2. After sync, pick the chain tip height
3. Look up its header via `ct.find_header_for_height(height).await` — print the header's merkle root
4. Validate using `ct.is_valid_root_for_height(&merkle_root, height).await` — print "valid: true"
5. Validate a bogus merkle root ("0000...") — print "valid: false"
6. Print explanation: "This validation is used internally by BEEF transaction verification to confirm merkle proofs against block headers."

## Phase 2: Wallet Operations

### `p2pkh_transfer.rs`

Transfer satoshis between two wallets using Pay-to-Public-Key-Hash.

1. Read `BSV_PRIVATE_KEY` and `BSV_PRIVATE_KEY_2`, build two wallets with separate SQLite files (`sender.db`, `receiver.db`) and default services
2. Print both identity keys and balances
3. Create P2PKH locking script from receiver's public key using `P2PKH` template from bsv-sdk
4. Sender calls `wallet.create_action(CreateActionArgs { outputs: vec![CreateActionOutput { locking_script, satoshis: 42, .. }], .. }, None).await`
5. Print txid and updated balances for both wallets

### `brc29_transfer.rs`

Transfer between two wallets using BRC29 key derivation protocol.

1. Same two-wallet setup with separate SQLite files
2. Generate random derivation prefix/suffix (base64-encoded random bytes)
3. Create BRC29 locking script using `ScriptTemplateBRC29::lock()` from `crate::utility::script_template_brc29`
4. Sender calls `create_action` with BRC29 output (1000 satoshis)
5. Receiver calls `wallet.internalize_action(InternalizeActionArgs { tx: beef_bytes, outputs: vec![InternalizeOutput::WalletPayment { payment: Payment { derivation_prefix, derivation_suffix, sender_identity_key } }], .. }, None).await` where `beef_bytes` is the `Vec<u8>` from sender's `CreateActionResult.tx`
6. Print txid, derivation info, and updated balances

### `list_balance.rs`

Query wallet balance and list spendable UTXOs.

1. Read `BSV_PRIVATE_KEY`, build single wallet
2. Call `wallet.list_outputs(ListOutputsArgs { basket: "default".into(), include: OutputInclude::LockingScripts, .. }, None).await`, paginate with `offset`
3. Print formatted table: satoshis, spendable status, outpoint (txid:vout)
4. Print total balance (sum of satoshis)
5. Also demonstrate `wallet.balance(None).await` convenience method

### `list_actions.rs`

List transaction history.

1. Read `BSV_PRIVATE_KEY`, build single wallet
2. Call `wallet.list_actions(ListActionsArgs { labels: vec![], .. }, None).await`
3. Print formatted table: txid (first 16 chars), status, satoshis, description, labels
4. Paginate if more than 10 results using `offset`

## Phase 3: Advanced

### `pushdrop_tokens.rs`

Mint and redeem data-bearing PushDrop tokens using `bsv::script::templates::push_drop::PushDrop`.

1. Read `BSV_PRIVATE_KEY`, build single wallet
2. Mint: create `PushDrop::new(fields, signing_key)` where fields are `vec![vec![1,2,3], vec![4,5,6]]` and signing_key is derived from wallet key. Use the PushDrop's locking script in `create_action` output with protocol ID and key ID in `customInstructions`.
3. Print minted token outpoint and data
4. Redeem: create another `create_action` that spends the token output, providing the PushDrop unlock template
5. Print redemption txid and recovered satoshis

### `nosend_batch.rs`

Create multiple transactions without immediate broadcast, then batch send.

1. Read `BSV_PRIVATE_KEY`, build single wallet
2. Create 3 PushDrop tokens in sequence, each with `CreateActionOptions { no_send: Some(true), no_send_change: previous_no_send_change, .. }`. Chain the `no_send_change: Vec<OutpointString>` from each `CreateActionResult` to the next action's options.
3. Collect all txids from results
4. Batch send via `create_action` with `CreateActionOptions { send_with: collected_txids, .. }`
5. Print all txids and final balance

### `internalize_payment.rs`

Receive and internalize an external payment.

1. Read `BSV_PRIVATE_KEY` and `BSV_PRIVATE_KEY_2`, build two wallets with separate SQLite files
2. Sender creates BRC29 payment to receiver (same flow as brc29_transfer)
3. Receiver calls `wallet.internalize_action(InternalizeActionArgs { tx: beef_bytes, outputs: vec![InternalizeOutput::WalletPayment { payment: Payment { derivation_prefix, derivation_suffix, sender_identity_key } }], description: "Received payment".into(), labels: vec!["incoming".into()] }, None).await`
4. Print internalized transaction details and receiver's updated balance

### `backup_wallet.rs`

Backup wallet data to a local SQLite file.

1. Read `BSV_PRIVATE_KEY`, build wallet manually (not via WalletBuilder) to control storage setup:
   - Create primary `SqliteStorage` at `wallet.db`
   - Create backup `SqliteStorage` at `backup_wallet.db`
   - Construct `WalletStorageManager::new(identity_key, Some(primary), vec![backup])`
   - Build wallet with both storages
2. Call `storage_manager.make_available().await` then `storage_manager.update_backups(None).await`
3. Print backup status: confirmation message, backup file path
4. Print backup file size on disk

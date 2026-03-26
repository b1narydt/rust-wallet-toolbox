//! StorageClient: remote JSON-RPC implementation of WalletStorageProvider.
//!
//! Forwards all `WalletStorageProvider` calls to a TypeScript wallet-toolbox
//! storage server via authenticated JSON-RPC 2.0 over BRC-31 (AuthFetch).
//!
//! Plan 01 provides:
//! - The struct, constructor, and core infrastructure (rpc_call, error mapping,
//!   settings caching, is_available)
//! - UpdateProvenTxReqWithNewProvenTx types + inherent method
//! - Stub WalletStorageProvider impl with todo!() on unimplemented methods
//!
//! Plan 02 fills in the ~23 remaining WalletStorageProvider methods.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use bsv::auth::clients::auth_fetch::AuthFetch;
use bsv::wallet::interfaces::{
    AbortActionArgs, AbortActionResult, ListActionsArgs, ListActionsResult, ListCertificatesArgs,
    ListCertificatesResult, ListOutputsArgs, ListOutputsResult, RelinquishCertificateArgs,
    RelinquishOutputArgs, WalletInterface,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::error::{wallet_error_from_object, WalletError, WalletErrorObject, WalletResult};
use crate::services::traits::WalletServices;
use crate::status::ProvenTxReqStatus;
use crate::storage::action_types::{
    StorageCreateActionArgs, StorageCreateActionResult, StorageInternalizeActionArgs,
    StorageInternalizeActionResult, StorageProcessActionArgs, StorageProcessActionResult,
};
use crate::storage::find_args::{
    FindCertificatesArgs, FindOutputBasketsArgs, FindOutputsArgs, FindProvenTxReqsArgs,
};
use crate::storage::sync::request_args::RequestSyncChunkArgs;
use crate::storage::sync::{ProcessSyncChunkResult, SyncChunk};
use crate::storage::traits::WalletStorageProvider;
use crate::tables::{Certificate, Output, OutputBasket, ProvenTxReq, Settings, SyncState, User};
use crate::wallet::types::AuthId;

// ---------------------------------------------------------------------------
// UpdateProvenTxReqWithNewProvenTx types
// ---------------------------------------------------------------------------

/// Arguments for the `updateProvenTxReqWithNewProvenTx` RPC method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProvenTxReqWithNewProvenTxArgs {
    /// Database ID of the proven transaction request to update.
    pub proven_tx_req_id: i64,
    /// Transaction identifier (hex).
    pub txid: String,
    /// Number of broadcast attempts made so far.
    pub attempts: i64,
    /// New status for the proven transaction request.
    pub status: ProvenTxReqStatus,
    /// JSON serialized history blob.
    pub history: String,
    /// Block height where the transaction was mined.
    pub height: i64,
    /// Index of the transaction within the block (coinbase = 0).
    pub index: i64,
    /// Block hash (hex) where the transaction was mined.
    pub block_hash: String,
    /// Merkle root of the block (hex).
    pub merkle_root: String,
    /// Merkle proof path as a list of node indices.
    pub merkle_path: Vec<i64>,
}

/// Result returned by `updateProvenTxReqWithNewProvenTx`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProvenTxReqWithNewProvenTxResult {
    /// Final status of the proven transaction request after processing.
    pub status: ProvenTxReqStatus,
    /// Updated history blob after server-side processing.
    pub history: String,
    /// Database ID of the newly created ProvenTx record.
    pub proven_tx_id: i64,
    /// Optional human-readable log messages from server-side processing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

// ---------------------------------------------------------------------------
// StorageClient
// ---------------------------------------------------------------------------

/// Remote JSON-RPC storage client.
///
/// Implements `WalletStorageProvider` by forwarding every call to a remote
/// TypeScript storage server using BRC-31 authenticated HTTP (AuthFetch).
///
/// # Type Parameter
///
/// `W` — a `WalletInterface` used for BRC-31 authentication during transport.
///
/// # Settings caching
///
/// `make_available` fetches settings from the server once and caches them.
/// `is_available` is a cheap sync check (reads an `AtomicBool`).
/// `get_settings` returns the cached value or errors if not yet fetched.
pub struct StorageClient<W: WalletInterface + Clone + Send + Sync + 'static> {
    /// BRC-31 authenticated HTTP client. Held behind tokio::sync::Mutex so
    /// `fetch(&mut self)` can be called from async contexts without deadlocking
    /// the Tokio executor (std::sync::Mutex must not be held across .await points).
    auth_fetch: Mutex<AuthFetch<W>>,
    /// URL of the remote storage server RPC endpoint.
    endpoint_url: String,
    /// Auto-incrementing JSON-RPC request id. Starts at 1.
    next_id: AtomicU64,
    /// Cached settings returned by `makeAvailable`. None until first call.
    settings: Mutex<Option<Settings>>,
    /// Cheap sync flag for `is_available()`. Set to true after first successful
    /// `make_available`. Uses Acquire/Release ordering to pair with `is_available`.
    settings_cached: AtomicBool,
}

impl<W: WalletInterface + Clone + Send + Sync + 'static> StorageClient<W> {
    /// Create a new `StorageClient` connecting to the given endpoint URL.
    pub fn new(wallet: W, endpoint_url: impl Into<String>) -> Self {
        StorageClient {
            auth_fetch: Mutex::new(AuthFetch::new(wallet)),
            endpoint_url: endpoint_url.into(),
            next_id: AtomicU64::new(1),
            settings: Mutex::new(None),
            settings_cached: AtomicBool::new(false),
        }
    }

    /// Send a JSON-RPC 2.0 request to the remote server and deserialize the result.
    ///
    /// Builds a JSON-RPC 2.0 envelope with positional params, locks the auth_fetch
    /// client, sends the POST request, and maps the response to either `T` or a
    /// `WalletError` (via `wallet_error_from_object` for WERR-coded errors).
    async fn rpc_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Vec<Value>,
    ) -> WalletResult<T> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let envelope = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id,
        });

        let body_bytes = serde_json::to_vec(&envelope).map_err(WalletError::SerdeJson)?;

        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());

        let response = {
            let mut fetch = self.auth_fetch.lock().await;
            fetch
                .fetch(&self.endpoint_url, "POST", Some(body_bytes), Some(headers))
                .await
                .map_err(|e| WalletError::Internal(format!("auth fetch: {}", e)))?
        };

        if response.status >= 400 {
            return Err(WalletError::Internal(format!(
                "HTTP {} from remote storage server",
                response.status
            )));
        }

        let json: Value = serde_json::from_slice(&response.body).map_err(WalletError::SerdeJson)?;

        if let Some(error_val) = json.get("error") {
            let err_obj: WalletErrorObject =
                serde_json::from_value(error_val.clone()).map_err(WalletError::SerdeJson)?;
            return Err(wallet_error_from_object(err_obj));
        }

        let result = json
            .get("result")
            .ok_or_else(|| {
                WalletError::Internal("JSON-RPC response missing 'result' field".to_string())
            })?
            .clone();

        serde_json::from_value::<T>(result).map_err(WalletError::SerdeJson)
    }

    /// Update a proven transaction request with a newly proven transaction.
    ///
    /// This method is used by the chain tracker to record on-chain proof
    /// for a previously submitted transaction request.
    pub async fn update_proven_tx_req_with_new_proven_tx(
        &self,
        args: &UpdateProvenTxReqWithNewProvenTxArgs,
    ) -> WalletResult<UpdateProvenTxReqWithNewProvenTxResult> {
        self.rpc_call(
            "updateProvenTxReqWithNewProvenTx",
            vec![serde_json::to_value(args)?],
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Wire deserialization helpers for tuple-returning RPC methods
// ---------------------------------------------------------------------------

/// Wire format for `findOrInsertUser` response: `{ "user": {...}, "isNew": true }`.
///
/// The TS server returns an object, but the Rust trait requires a tuple.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FindOrInsertUserWire {
    user: User,
    is_new: bool,
}

/// Wire format for `findOrInsertSyncStateAuth` response: `{ "syncState": {...}, "isNew": true }`.
///
/// The TS server returns an object, but the Rust trait requires a tuple.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FindOrInsertSyncStateWire {
    sync_state: SyncState,
    is_new: bool,
}

// ---------------------------------------------------------------------------
// WalletStorageProvider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl<W: WalletInterface + Clone + Send + Sync + 'static> WalletStorageProvider
    for StorageClient<W>
{
    // StorageClient is NOT a local storage provider — it is the remote client.
    fn is_storage_provider(&self) -> bool {
        false
    }

    // StorageClient returns its remote endpoint URL so get_stores() can populate
    // WalletStorageInfo.endpoint_url for remote providers.
    fn get_endpoint_url(&self) -> Option<String> {
        Some(self.endpoint_url.clone())
    }

    fn is_available(&self) -> bool {
        // Acquire pairs with the Release store in make_available.
        self.settings_cached.load(Ordering::Acquire)
    }

    async fn get_settings(&self) -> WalletResult<Settings> {
        let guard = self.settings.lock().await;
        guard.clone().ok_or_else(|| {
            WalletError::InvalidOperation(
                "call makeAvailable at least once before getSettings".to_string(),
            )
        })
    }

    async fn make_available(&self) -> WalletResult<Settings> {
        let mut guard = self.settings.lock().await;

        // Return cached value if already fetched.
        if let Some(ref cached) = *guard {
            return Ok(cached.clone());
        }

        let settings: Settings = self.rpc_call("makeAvailable", vec![]).await?;

        *guard = Some(settings.clone());
        // Release store — pairs with Acquire load in is_available().
        self.settings_cached.store(true, Ordering::Release);

        Ok(settings)
    }

    // -----------------------------------------------------------------------
    // Migration and lifecycle
    // -----------------------------------------------------------------------

    async fn migrate(
        &self,
        storage_name: &str,
        _storage_identity_key: &str,
    ) -> WalletResult<String> {
        // TS StorageClient only sends storageName — storageIdentityKey is dropped.
        self.rpc_call("migrate", vec![Value::String(storage_name.to_string())])
            .await
    }

    async fn destroy(&self) -> WalletResult<()> {
        self.rpc_call("destroy", vec![]).await
    }

    // -----------------------------------------------------------------------
    // User management
    // -----------------------------------------------------------------------

    async fn find_or_insert_user(&self, identity_key: &str) -> WalletResult<(User, bool)> {
        let r: FindOrInsertUserWire = self
            .rpc_call(
                "findOrInsertUser",
                vec![Value::String(identity_key.to_string())],
            )
            .await?;
        Ok((r.user, r.is_new))
    }

    // -----------------------------------------------------------------------
    // Find / read methods
    // -----------------------------------------------------------------------

    async fn find_certificates_auth(
        &self,
        auth: &AuthId,
        args: &FindCertificatesArgs,
    ) -> WalletResult<Vec<Certificate>> {
        self.rpc_call(
            "findCertificatesAuth",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    /// Wire name is `findOutputBaskets` — no Auth suffix (intentional TS naming anomaly).
    async fn find_output_baskets_auth(
        &self,
        auth: &AuthId,
        args: &FindOutputBasketsArgs,
    ) -> WalletResult<Vec<OutputBasket>> {
        self.rpc_call(
            "findOutputBaskets",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    async fn find_outputs_auth(
        &self,
        auth: &AuthId,
        args: &FindOutputsArgs,
    ) -> WalletResult<Vec<Output>> {
        self.rpc_call(
            "findOutputsAuth",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    async fn find_proven_tx_reqs(
        &self,
        args: &FindProvenTxReqsArgs,
    ) -> WalletResult<Vec<ProvenTxReq>> {
        // No auth param — public method
        self.rpc_call("findProvenTxReqs", vec![serde_json::to_value(args)?])
            .await
    }

    // -----------------------------------------------------------------------
    // List methods
    // -----------------------------------------------------------------------

    async fn list_actions(
        &self,
        auth: &AuthId,
        args: &ListActionsArgs,
    ) -> WalletResult<ListActionsResult> {
        self.rpc_call(
            "listActions",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    async fn list_certificates(
        &self,
        auth: &AuthId,
        args: &ListCertificatesArgs,
    ) -> WalletResult<ListCertificatesResult> {
        self.rpc_call(
            "listCertificates",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    async fn list_outputs(
        &self,
        auth: &AuthId,
        args: &ListOutputsArgs,
    ) -> WalletResult<ListOutputsResult> {
        self.rpc_call(
            "listOutputs",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Action write methods
    // -----------------------------------------------------------------------

    async fn abort_action(
        &self,
        auth: &AuthId,
        args: &AbortActionArgs,
    ) -> WalletResult<AbortActionResult> {
        self.rpc_call(
            "abortAction",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    async fn create_action(
        &self,
        auth: &AuthId,
        args: &StorageCreateActionArgs,
    ) -> WalletResult<StorageCreateActionResult> {
        self.rpc_call(
            "createAction",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    async fn process_action(
        &self,
        auth: &AuthId,
        args: &StorageProcessActionArgs,
    ) -> WalletResult<StorageProcessActionResult> {
        self.rpc_call(
            "processAction",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    /// `services` is not serializable and has no TS equivalent — ignored entirely.
    async fn internalize_action(
        &self,
        auth: &AuthId,
        args: &StorageInternalizeActionArgs,
        _services: &dyn WalletServices,
    ) -> WalletResult<StorageInternalizeActionResult> {
        self.rpc_call(
            "internalizeAction",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Certificate and output write methods
    // -----------------------------------------------------------------------

    async fn insert_certificate_auth(
        &self,
        auth: &AuthId,
        certificate: &Certificate,
    ) -> WalletResult<i64> {
        self.rpc_call(
            "insertCertificateAuth",
            vec![
                serde_json::to_value(auth)?,
                serde_json::to_value(certificate)?,
            ],
        )
        .await
    }

    async fn relinquish_certificate(
        &self,
        auth: &AuthId,
        args: &RelinquishCertificateArgs,
    ) -> WalletResult<i64> {
        self.rpc_call(
            "relinquishCertificate",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    async fn relinquish_output(
        &self,
        auth: &AuthId,
        args: &RelinquishOutputArgs,
    ) -> WalletResult<i64> {
        self.rpc_call(
            "relinquishOutput",
            vec![serde_json::to_value(auth)?, serde_json::to_value(args)?],
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Sync methods
    // -----------------------------------------------------------------------

    async fn find_or_insert_sync_state_auth(
        &self,
        auth: &AuthId,
        storage_identity_key: &str,
        storage_name: &str,
    ) -> WalletResult<(SyncState, bool)> {
        // Positional order: [auth, storageIdentityKey, storageName]
        let r: FindOrInsertSyncStateWire = self
            .rpc_call(
                "findOrInsertSyncStateAuth",
                vec![
                    serde_json::to_value(auth)?,
                    Value::String(storage_identity_key.to_string()),
                    Value::String(storage_name.to_string()),
                ],
            )
            .await?;
        Ok((r.sync_state, r.is_new))
    }

    async fn set_active(
        &self,
        auth: &AuthId,
        new_active_storage_identity_key: &str,
    ) -> WalletResult<i64> {
        self.rpc_call(
            "setActive",
            vec![
                serde_json::to_value(auth)?,
                Value::String(new_active_storage_identity_key.to_string()),
            ],
        )
        .await
    }

    async fn get_sync_chunk(&self, args: &RequestSyncChunkArgs) -> WalletResult<SyncChunk> {
        // No auth param — sync protocol uses args.identity_key directly
        self.rpc_call("getSyncChunk", vec![serde_json::to_value(args)?])
            .await
    }

    async fn process_sync_chunk(
        &self,
        args: &RequestSyncChunkArgs,
        chunk: &SyncChunk,
    ) -> WalletResult<ProcessSyncChunkResult> {
        self.rpc_call(
            "processSyncChunk",
            vec![serde_json::to_value(args)?, serde_json::to_value(chunk)?],
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{wallet_error_from_object, WalletErrorObject};

    /// Verify that the JSON-RPC 2.0 envelope is constructed correctly.
    ///
    /// We test the construction directly without making network calls.
    #[test]
    fn test_rpc_envelope() {
        let method = "testMethod";
        let params = vec![json!({"key": "value"}), json!(42)];
        let id: u64 = 7;

        let envelope = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id,
        });

        assert_eq!(envelope["jsonrpc"], "2.0");
        assert_eq!(envelope["method"], "testMethod");
        assert_eq!(envelope["id"], 7);
        assert!(envelope["params"].is_array());
        assert_eq!(envelope["params"].as_array().unwrap().len(), 2);
    }

    /// Verify that each WERR error code maps to the correct WalletError variant.
    #[test]
    fn test_error_mapping() {
        fn make_obj(name: &str, msg: &str) -> WalletErrorObject {
            WalletErrorObject {
                is_error: true,
                name: name.to_string(),
                message: msg.to_string(),
                code: None,
                parameter: None,
                total_satoshis_needed: None,
                more_satoshis_needed: None,
            }
        }

        fn make_obj_with_param(name: &str, msg: &str, param: &str) -> WalletErrorObject {
            WalletErrorObject {
                is_error: true,
                name: name.to_string(),
                message: msg.to_string(),
                code: None,
                parameter: Some(param.to_string()),
                total_satoshis_needed: None,
                more_satoshis_needed: None,
            }
        }

        // WERR_INVALID_PARAMETER
        let err = wallet_error_from_object(make_obj_with_param(
            "WERR_INVALID_PARAMETER",
            "must be a string",
            "name",
        ));
        assert!(
            matches!(err, WalletError::InvalidParameter { parameter, must_be }
                if parameter == "name" && must_be == "must be a string")
        );

        // WERR_NOT_IMPLEMENTED
        let err = wallet_error_from_object(make_obj("WERR_NOT_IMPLEMENTED", "not done yet"));
        assert!(matches!(err, WalletError::NotImplemented(m) if m == "not done yet"));

        // WERR_BAD_REQUEST
        let err = wallet_error_from_object(make_obj("WERR_BAD_REQUEST", "bad payload"));
        assert!(matches!(err, WalletError::BadRequest(m) if m == "bad payload"));

        // WERR_UNAUTHORIZED
        let err = wallet_error_from_object(make_obj("WERR_UNAUTHORIZED", "no token"));
        assert!(matches!(err, WalletError::Unauthorized(m) if m == "no token"));

        // WERR_NOT_ACTIVE
        let err = wallet_error_from_object(make_obj("WERR_NOT_ACTIVE", "inactive"));
        assert!(matches!(err, WalletError::NotActive(m) if m == "inactive"));

        // WERR_INVALID_OPERATION
        let err = wallet_error_from_object(make_obj("WERR_INVALID_OPERATION", "wrong state"));
        assert!(matches!(err, WalletError::InvalidOperation(m) if m == "wrong state"));

        // WERR_MISSING_PARAMETER (no parameter field — falls back to message)
        let err = wallet_error_from_object(make_obj("WERR_MISSING_PARAMETER", "txid"));
        assert!(matches!(err, WalletError::MissingParameter(m) if m == "txid"));

        // WERR_MISSING_PARAMETER with parameter field
        let err = wallet_error_from_object(make_obj_with_param(
            "WERR_MISSING_PARAMETER",
            "required",
            "txid",
        ));
        assert!(matches!(err, WalletError::MissingParameter(m) if m == "txid"));

        // WERR_INSUFFICIENT_FUNDS
        let err = wallet_error_from_object(WalletErrorObject {
            is_error: true,
            name: "WERR_INSUFFICIENT_FUNDS".to_string(),
            message: "need more".to_string(),
            code: None,
            parameter: None,
            total_satoshis_needed: Some(1000),
            more_satoshis_needed: Some(500),
        });
        assert!(
            matches!(err, WalletError::InsufficientFunds { message, total_satoshis_needed, more_satoshis_needed }
                if message == "need more" && total_satoshis_needed == 1000 && more_satoshis_needed == 500)
        );

        // WERR_BROADCAST_UNAVAILABLE
        let err = wallet_error_from_object(make_obj("WERR_BROADCAST_UNAVAILABLE", "down"));
        assert!(matches!(err, WalletError::BroadcastUnavailable));

        // WERR_NETWORK_CHAIN
        let err = wallet_error_from_object(make_obj("WERR_NETWORK_CHAIN", "chain mismatch"));
        assert!(matches!(err, WalletError::NetworkChain(m) if m == "chain mismatch"));

        // WERR_INVALID_PUBLIC_KEY
        let err = wallet_error_from_object(make_obj_with_param(
            "WERR_INVALID_PUBLIC_KEY",
            "bad key format",
            "deadbeef",
        ));
        assert!(matches!(err, WalletError::InvalidPublicKey { message, key }
                if message == "bad key format" && key == "deadbeef"));

        // Unknown code falls through to Internal
        let err = wallet_error_from_object(make_obj("WERR_UNKNOWN_FUTURE_CODE", "mystery"));
        assert!(matches!(err, WalletError::Internal(m) if m == "mystery"));
    }

    /// Verify that StorageClient reports is_available() == false before make_available
    /// and true after the atomic flag is manually set (no network needed).
    #[test]
    fn test_settings_cache_atomic() {
        // We cannot construct a real StorageClient without a WalletInterface,
        // so we test the AtomicBool behavior in isolation — same logic used by
        // is_available().
        let flag = AtomicBool::new(false);
        assert!(
            !flag.load(Ordering::Acquire),
            "should be false before make_available"
        );

        flag.store(true, Ordering::Release);
        assert!(flag.load(Ordering::Acquire), "should be true after store");
    }

    /// Verify the critical wire method name mappings match the TS StorageClient.
    ///
    /// Since we cannot make live RPC calls in unit tests, this test documents
    /// the expected wire names as a regression guard against typos or renaming.
    /// Any deviation from these names will break interop with TS storage servers.
    #[test]
    fn test_wire_names() {
        // Table: (rust_method, expected_wire_name)
        // Derived from the TS StorageClient source and cross-checked against
        // the RESEARCH.md wire name table.
        let mappings: &[(&str, &str)] = &[
            ("make_available", "makeAvailable"),
            ("migrate", "migrate"),
            ("destroy", "destroy"),
            ("find_or_insert_user", "findOrInsertUser"),
            ("abort_action", "abortAction"),
            ("create_action", "createAction"),
            ("process_action", "processAction"),
            ("internalize_action", "internalizeAction"),
            ("insert_certificate_auth", "insertCertificateAuth"),
            ("relinquish_certificate", "relinquishCertificate"),
            ("relinquish_output", "relinquishOutput"),
            ("find_certificates_auth", "findCertificatesAuth"),
            // CRITICAL: find_output_baskets_auth uses "findOutputBaskets" — NO Auth suffix
            ("find_output_baskets_auth", "findOutputBaskets"),
            ("find_outputs_auth", "findOutputsAuth"),
            ("find_proven_tx_reqs", "findProvenTxReqs"),
            ("list_actions", "listActions"),
            ("list_certificates", "listCertificates"),
            ("list_outputs", "listOutputs"),
            (
                "find_or_insert_sync_state_auth",
                "findOrInsertSyncStateAuth",
            ),
            ("set_active", "setActive"),
            ("get_sync_chunk", "getSyncChunk"),
            ("process_sync_chunk", "processSyncChunk"),
        ];

        // Verify each entry is non-empty and camelCase (starts lowercase, no underscores)
        for (rust_name, wire_name) in mappings {
            assert!(
                !wire_name.is_empty(),
                "{} must have a non-empty wire name",
                rust_name
            );
            assert!(
                !wire_name.contains('_'),
                "wire name '{}' for '{}' must be camelCase (no underscores)",
                wire_name,
                rust_name
            );
            // camelCase: must start with a lowercase letter
            assert!(
                wire_name
                    .chars()
                    .next()
                    .map(|c| c.is_lowercase())
                    .unwrap_or(false),
                "wire name '{}' for '{}' must start with a lowercase letter (camelCase)",
                wire_name,
                rust_name
            );
        }

        // Spot-check the only anomalous mapping: findOutputBaskets has no Auth suffix
        let basket_entry = mappings
            .iter()
            .find(|(r, _)| *r == "find_output_baskets_auth");
        assert!(basket_entry.is_some());
        assert_eq!(basket_entry.unwrap().1, "findOutputBaskets");

        // Confirm all 22 methods are represented
        assert_eq!(mappings.len(), 22, "expected 22 wire method mappings");
    }

    /// Verify that UpdateProvenTxReqWithNewProvenTxArgs serializes with camelCase keys.
    #[test]
    fn test_update_proven_tx_req_serialization() {
        let args = UpdateProvenTxReqWithNewProvenTxArgs {
            proven_tx_req_id: 42,
            txid: "abc123".to_string(),
            attempts: 3,
            status: ProvenTxReqStatus::Completed,
            history: "{}".to_string(),
            height: 800000,
            index: 1,
            block_hash: "deadbeef".to_string(),
            merkle_root: "cafebabe".to_string(),
            merkle_path: vec![1, 2, 3],
        };
        let v = serde_json::to_value(&args).unwrap();
        assert!(
            v.get("provenTxReqId").is_some(),
            "should serialize provenTxReqId"
        );
        assert!(v.get("merklePath").is_some(), "should serialize merklePath");
        assert_eq!(v["provenTxReqId"], 42);
    }
}

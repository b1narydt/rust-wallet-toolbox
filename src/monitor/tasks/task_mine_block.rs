//! TaskMineBlock -- test-only task for integration testing.
//!
//! This entire module is behind #[cfg(test)].
//! Advances the chain so other tasks (CheckForProofs, NewHeader) can
//! exercise their logic in integration tests.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_SECOND;
use crate::services::traits::WalletServices;

/// Test-only task that mines a block on a mock/regtest chain.
///
/// This is a test utility for integration tests, not production code.
/// It holds a reference to WalletServices so it can query the current chain
/// height. When a real mine_block() capability is added to WalletServices,
/// this task can be updated to call it.
pub struct TaskMineBlock {
    /// Network services, used to query current chain height.
    services: Arc<dyn WalletServices>,
    trigger_msecs: u64,
    last_run_msecs: u64,
}

impl TaskMineBlock {
    /// Create a new TaskMineBlock with the given services and default trigger interval (10s).
    pub fn new(services: Arc<dyn WalletServices>) -> Self {
        Self {
            services,
            trigger_msecs: 10 * ONE_SECOND,
            last_run_msecs: 0,
        }
    }

    /// Create with a custom trigger interval in milliseconds.
    pub fn with_trigger_msecs(mut self, msecs: u64) -> Self {
        self.trigger_msecs = msecs;
        self
    }
}

#[async_trait]
impl WalletMonitorTask for TaskMineBlock {
    fn name(&self) -> &str {
        "MineBlock"
    }

    fn trigger(&mut self, now_msecs: u64) -> bool {
        now_msecs > self.last_run_msecs + self.trigger_msecs
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        self.last_run_msecs = now_msecs();

        // Query current chain height via services. When a real mine_block()
        // method is added to WalletServices this task can call it instead.
        let height = match self.services.get_height().await {
            Ok(h) => h,
            Err(e) => {
                return Ok(format!("mine_block: error getting chain height: {}", e));
            }
        };

        Ok(format!("mine_block: current chain height {}", height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WalletResult;
    use crate::monitor::ONE_SECOND;
    use crate::services::traits::WalletServices;
    use crate::services::types::{
        BlockHeader, GetMerklePathResult, GetRawTxResult, GetScriptHashHistoryResult,
        GetStatusForTxidsResult, GetUtxoStatusOutputFormat, GetUtxoStatusResult, NLockTimeInput,
        PostBeefResult, ServicesCallHistory,
    };
    use crate::types::Chain;
    use async_trait::async_trait;
    use bsv::transaction::Beef;

    /// Minimal mock implementation of WalletServices for unit tests.
    struct MockServices {
        height: u32,
    }

    #[async_trait]
    impl WalletServices for MockServices {
        fn chain(&self) -> Chain {
            Chain::Test
        }

        async fn get_chain_tracker(
            &self,
        ) -> WalletResult<Box<dyn bsv::transaction::chain_tracker::ChainTracker>> {
            Err(WalletError::NotImplemented("mock".into()))
        }

        async fn get_merkle_path(&self, _txid: &str, _use_next: bool) -> GetMerklePathResult {
            GetMerklePathResult::default()
        }

        async fn get_raw_tx(&self, _txid: &str, _use_next: bool) -> GetRawTxResult {
            GetRawTxResult::default()
        }

        async fn post_beef(&self, _beef: &[u8], _txids: &[String]) -> Vec<PostBeefResult> {
            vec![]
        }

        async fn get_utxo_status(
            &self,
            _output: &str,
            _output_format: Option<GetUtxoStatusOutputFormat>,
            _outpoint: Option<&str>,
            _use_next: bool,
        ) -> GetUtxoStatusResult {
            GetUtxoStatusResult {
                name: "mock".to_string(),
                status: "error".to_string(),
                error: Some("mock".to_string()),
                is_utxo: None,
                details: vec![],
            }
        }

        async fn get_status_for_txids(
            &self,
            _txids: &[String],
            _use_next: bool,
        ) -> GetStatusForTxidsResult {
            GetStatusForTxidsResult {
                name: "mock".to_string(),
                status: "error".to_string(),
                error: Some("mock".to_string()),
                results: vec![],
            }
        }

        async fn get_script_hash_history(
            &self,
            _hash: &str,
            _use_next: bool,
        ) -> GetScriptHashHistoryResult {
            GetScriptHashHistoryResult {
                name: "mock".to_string(),
                status: "error".to_string(),
                error: Some("mock".to_string()),
                history: vec![],
            }
        }

        async fn hash_to_header(&self, _hash: &str) -> WalletResult<BlockHeader> {
            Err(WalletError::NotImplemented("mock".into()))
        }

        async fn get_header_for_height(&self, _height: u32) -> WalletResult<Vec<u8>> {
            Err(WalletError::NotImplemented("mock".into()))
        }

        async fn get_height(&self) -> WalletResult<u32> {
            Ok(self.height)
        }

        async fn n_lock_time_is_final(&self, _input: NLockTimeInput) -> WalletResult<bool> {
            Ok(true)
        }

        async fn get_bsv_exchange_rate(
            &self,
        ) -> WalletResult<crate::services::types::BsvExchangeRate> {
            Err(WalletError::NotImplemented("mock".into()))
        }

        async fn get_fiat_exchange_rate(
            &self,
            _currency: &str,
            _base: Option<&str>,
        ) -> WalletResult<f64> {
            Ok(1.0)
        }

        async fn get_fiat_exchange_rates(
            &self,
            _target_currencies: &[String],
        ) -> WalletResult<crate::services::types::FiatExchangeRates> {
            Err(WalletError::NotImplemented("mock".into()))
        }

        fn get_services_call_history(&self, _reset: bool) -> ServicesCallHistory {
            ServicesCallHistory { services: vec![] }
        }

        async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<Beef> {
            Err(WalletError::NotImplemented("mock".into()))
        }

        fn hash_output_script(&self, _script: &[u8]) -> String {
            String::new()
        }

        async fn is_utxo(
            &self,
            _locking_script: &[u8],
            _txid: &str,
            _vout: u32,
        ) -> WalletResult<bool> {
            Ok(false)
        }
    }

    fn mock_services(height: u32) -> Arc<dyn WalletServices> {
        Arc::new(MockServices { height })
    }

    #[test]
    fn test_mine_block_trigger() {
        let mut task = TaskMineBlock::new(mock_services(100));
        assert!(task.trigger(10 * ONE_SECOND + 1)); // should trigger
        assert!(!task.trigger(5 * ONE_SECOND)); // too early
    }

    #[tokio::test]
    async fn test_mine_block_run_task() {
        let mut task = TaskMineBlock::new(mock_services(42));
        let result = task.run_task().await.unwrap();
        assert!(result.contains("mine_block"));
        assert!(result.contains("42"));
    }

    #[test]
    fn test_name() {
        let task = TaskMineBlock::new(mock_services(0));
        assert_eq!(task.name(), "MineBlock");
    }
}

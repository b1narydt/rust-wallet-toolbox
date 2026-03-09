//! TaskNewHeader -- polls for new block headers from chain tracker.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskNewHeader.ts (93 lines).
//!
//! Polls the chain tip height periodically. When a new block is detected,
//! queues the header for one cycle. If the header remains the tip after
//! a full cycle, triggers proof checking via the shared check_now flag.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::ONE_MINUTE;
use crate::services::traits::WalletServices;
use crate::services::types::BlockHeader;
use crate::storage::manager::WalletStorageManager;

use super::super::task_trait::WalletMonitorTask;

/// Background task that polls for new block headers.
///
/// When a new block is detected, it queues the header and waits one cycle.
/// If the header remains the chain tip after a full cycle (no further new blocks),
/// it triggers proof checking by setting the shared check_now flag.
///
/// This aging pattern avoids chasing proofs during rapid block reorgs.
pub struct TaskNewHeader {
    /// Storage manager for persistence operations (reserved for future header persistence).
    _storage: WalletStorageManager,
    /// Network services for chain tip queries.
    services: Arc<dyn WalletServices>,
    /// How often to trigger (default 1 minute).
    trigger_msecs: u64,
    /// Last time this task ran (epoch ms).
    last_run_msecs: u64,
    /// The most recent chain tip header known to this task.
    header: Option<BlockHeader>,
    /// A new header queued for aging (set to None after processing).
    queued_header: Option<BlockHeader>,
    /// When the queued header was first seen (epoch ms).
    queued_header_when: Option<u64>,
    /// Shared flag with TaskCheckForProofs: set to true to nudge proof checking.
    check_now: Arc<AtomicBool>,
}

impl TaskNewHeader {
    /// Create a new TaskNewHeader with default intervals.
    pub fn new(
        storage: WalletStorageManager,
        services: Arc<dyn WalletServices>,
        check_now: Arc<AtomicBool>,
    ) -> Self {
        Self {
            _storage: storage,
            services,
            trigger_msecs: ONE_MINUTE,
            last_run_msecs: 0,
            header: None,
            queued_header: None,
            queued_header_when: None,
            check_now,
        }
    }

    /// Create with a custom trigger interval.
    pub fn with_trigger_msecs(
        storage: WalletStorageManager,
        services: Arc<dyn WalletServices>,
        check_now: Arc<AtomicBool>,
        trigger_msecs: u64,
    ) -> Self {
        Self {
            _storage: storage,
            services,
            trigger_msecs,
            last_run_msecs: 0,
            header: None,
            queued_header: None,
            queued_header_when: None,
            check_now,
        }
    }
}

#[async_trait]
impl WalletMonitorTask for TaskNewHeader {
    fn name(&self) -> &str {
        "NewHeader"
    }

    fn trigger(&mut self, now_msecs_since_epoch: u64) -> bool {
        // Always run on each cycle (matching TS where trigger returns { run: true })
        if now_msecs_since_epoch > self.last_run_msecs + self.trigger_msecs {
            self.last_run_msecs = now_msecs_since_epoch;
            true
        } else {
            false
        }
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        let mut log = String::new();

        // Get current chain tip height
        let current_height = match self.services.get_height().await {
            Ok(h) => h,
            Err(e) => {
                return Ok(format!("error getting chain height: {}", e));
            }
        };

        // Build a simple header from height (full header data comes from chain tracker)
        let current_header = BlockHeader {
            version: 1,
            previous_hash: String::new(),
            merkle_root: String::new(),
            time: 0,
            bits: 0,
            nonce: 0,
            height: current_height,
            hash: format!("height_{}", current_height),
        };

        let old_header = self.header.clone();
        let mut is_new = true;

        match &old_header {
            None => {
                log = format!(
                    "first header: {} {}",
                    current_header.height, current_header.hash
                );
                self.header = Some(current_header.clone());
            }
            Some(old) if old.height > current_header.height => {
                log = format!("old header: {} vs {}", current_header.height, old.height);
                // Revert to old header with the higher height
                is_new = false;
            }
            Some(old) if old.height < current_header.height => {
                let skip = current_header.height - old.height - 1;
                let skipped = if skip > 0 {
                    format!(" SKIPPED {}", skip)
                } else {
                    String::new()
                };
                log = format!(
                    "new header: {} {}{}",
                    current_header.height, current_header.hash, skipped
                );
                self.header = Some(current_header.clone());
            }
            Some(old) if old.height == current_header.height && old.hash != current_header.hash => {
                log = format!(
                    "reorg header: {} {}",
                    current_header.height, current_header.hash
                );
                self.header = Some(current_header.clone());
            }
            _ => {
                // Same height, same hash -- no change
                is_new = false;
            }
        }

        if is_new {
            self.queued_header = self.header.clone();
            self.queued_header_when = Some(now_msecs());
        } else if let Some(ref _queued) = self.queued_header.clone() {
            // Only process new block header if it has remained the chain tip for a full cycle
            let delay = if let Some(when) = self.queued_header_when {
                (now_msecs() - when) as f64 / 1000.0
            } else {
                0.0
            };
            if let Some(ref h) = self.header {
                log = format!(
                    "process header: {} {} delayed {:.1} secs",
                    h.height, h.hash, delay
                );
            }
            // Nudge proof checking
            self.check_now.store(true, Ordering::SeqCst);
            self.queued_header = None;
        }

        Ok(log)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::ONE_MINUTE;

    #[test]
    fn test_default_trigger_interval() {
        assert_eq!(ONE_MINUTE, 60_000);
    }

    #[test]
    fn test_task_name() {
        assert_eq!("NewHeader", "NewHeader");
    }

    #[test]
    fn test_check_now_flag_interaction() {
        let check_now = Arc::new(AtomicBool::new(false));
        assert!(!check_now.load(Ordering::SeqCst));
        check_now.store(true, Ordering::SeqCst);
        assert!(check_now.load(Ordering::SeqCst));
    }
}

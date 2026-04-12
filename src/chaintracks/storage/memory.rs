//! In-memory storage backend for chaintracks.

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;

use crate::chaintracks::{
    calculate_work, BaseBlockHeader, BlockHeader, ChaintracksStorage, ChaintracksStorageIngest,
    ChaintracksStorageQuery, HeightRange, InsertHeaderResult, LiveBlockHeader,
};
use crate::error::WalletResult;
use crate::types::Chain;

const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Inner state protected by a single `RwLock` to eliminate lock-ordering issues.
struct Inner {
    headers: HashMap<u64, LiveBlockHeader>,
    hash_to_id: HashMap<String, u64>,
    height_to_id: HashMap<u32, u64>,
    merkle_to_id: HashMap<String, u64>,
    next_id: u64,
    tip_id: Option<u64>,
}

/// In-memory storage backend implementing the full `ChaintracksStorage` trait.
///
/// Uses a single `std::sync::RwLock` internally -- safe for use from async
/// contexts since HashMap operations are fast and never block for long.
/// The single-lock design eliminates lock-ordering deadlocks and TOCTOU races.
pub struct MemoryStorage {
    chain: Chain,
    live_height_threshold: u32,
    reorg_height_threshold: u32,
    inner: RwLock<Inner>,
}

impl MemoryStorage {
    /// Create a new `MemoryStorage` with default thresholds (2000/400).
    pub fn new(chain: Chain) -> Self {
        Self::with_thresholds(chain, 2000, 400)
    }

    /// Create a new `MemoryStorage` with custom thresholds.
    pub fn with_thresholds(chain: Chain, live_threshold: u32, reorg_threshold: u32) -> Self {
        Self {
            chain,
            live_height_threshold: live_threshold,
            reorg_height_threshold: reorg_threshold,
            inner: RwLock::new(Inner {
                headers: HashMap::new(),
                hash_to_id: HashMap::new(),
                height_to_id: HashMap::new(),
                merkle_to_id: HashMap::new(),
                next_id: 1,
                tip_id: None,
            }),
        }
    }

    // ------------------------------------------------------------------
    // Public helpers (not part of trait)
    // ------------------------------------------------------------------

    /// Returns the number of headers currently stored.
    pub fn header_count(&self) -> usize {
        self.inner.read().unwrap().headers.len()
    }

    /// Returns all headers at the given height.
    pub fn get_headers_at_height(&self, height: u32) -> Vec<LiveBlockHeader> {
        let inner = self.inner.read().unwrap();
        inner
            .headers
            .values()
            .filter(|h| h.height == height)
            .cloned()
            .collect()
    }

    /// Returns all active headers.
    pub fn get_active_headers(&self) -> Vec<LiveBlockHeader> {
        let inner = self.inner.read().unwrap();
        inner
            .headers
            .values()
            .filter(|h| h.is_active)
            .cloned()
            .collect()
    }

    /// Returns all fork (inactive) headers.
    pub fn get_fork_headers(&self) -> Vec<LiveBlockHeader> {
        let inner = self.inner.read().unwrap();
        inner
            .headers
            .values()
            .filter(|h| !h.is_active)
            .cloned()
            .collect()
    }

    /// Returns all headers whose `previous_hash` matches `parent_hash`.
    pub fn find_children(&self, parent_hash: &str) -> Vec<LiveBlockHeader> {
        let inner = self.inner.read().unwrap();
        inner
            .headers
            .values()
            .filter(|h| h.previous_hash == parent_hash)
            .cloned()
            .collect()
    }

    /// Insert multiple headers in a batch.
    pub async fn insert_headers_batch(
        &self,
        headers: Vec<LiveBlockHeader>,
    ) -> WalletResult<Vec<InsertHeaderResult>> {
        let mut results = Vec::with_capacity(headers.len());
        for h in headers {
            results.push(self.insert_header(h).await?);
        }
        Ok(results)
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Handle a reorg: deactivate the old chain branch and activate the new one.
    ///
    /// Operates on the already-locked `Inner` to avoid re-acquiring the lock.
    fn handle_reorg(
        &self,
        inner: &mut Inner,
        old_tip_id: u64,
        new_tip_id: u64,
    ) -> (u32, Vec<BlockHeader>) {
        // Find common ancestor
        let ancestor = Self::find_common_ancestor_sync(
            self.reorg_height_threshold,
            &inner.headers,
            &inner.hash_to_id,
            old_tip_id,
            new_tip_id,
        );

        let ancestor_hash = match &ancestor {
            Some(a) => a.hash.clone(),
            None => return (0, Vec::new()),
        };

        let mut deactivated = Vec::new();

        // Deactivate old chain: walk from old_tip back to ancestor
        let mut current_id = Some(old_tip_id);
        while let Some(cid) = current_id {
            let header = match inner.headers.get(&cid) {
                Some(h) => h.clone(),
                None => break,
            };
            if header.hash == ancestor_hash {
                break;
            }
            // Deactivate
            if let Some(h) = inner.headers.get_mut(&cid) {
                h.is_active = false;
                inner.height_to_id.remove(&h.height);
                inner.merkle_to_id.remove(&h.merkle_root);
                deactivated.push(BlockHeader::from(h.clone()));
            }
            // Walk back
            current_id = inner.headers.get(&cid).and_then(|h| {
                h.previous_header_id.and_then(|pid| {
                    if inner.headers.contains_key(&pid) {
                        Some(pid)
                    } else {
                        None
                    }
                })
            });
        }

        let depth = deactivated.len() as u32;

        // Activate new chain: walk from new_tip back to ancestor
        let mut current_id = Some(new_tip_id);
        while let Some(cid) = current_id {
            let header = match inner.headers.get(&cid) {
                Some(h) => h.clone(),
                None => break,
            };
            if header.hash == ancestor_hash {
                break;
            }
            // Activate
            if let Some(h) = inner.headers.get_mut(&cid) {
                h.is_active = true;
                inner.height_to_id.insert(h.height, cid);
                inner.merkle_to_id.insert(h.merkle_root.clone(), cid);
            }
            // Walk back
            current_id = inner.headers.get(&cid).and_then(|h| {
                h.previous_header_id.and_then(|pid| {
                    if inner.headers.contains_key(&pid) {
                        Some(pid)
                    } else {
                        None
                    }
                })
            });
        }

        (depth, deactivated)
    }

    /// Synchronous common ancestor finding. Walks back from both tips until hashes match.
    fn find_common_ancestor_sync(
        reorg_height_threshold: u32,
        headers: &HashMap<u64, LiveBlockHeader>,
        hash_to_id: &HashMap<String, u64>,
        id1: u64,
        id2: u64,
    ) -> Option<LiveBlockHeader> {
        let h1 = headers.get(&id1)?;
        let h2 = headers.get(&id2)?;

        let mut cur1 = h1.clone();
        let mut cur2 = h2.clone();

        // Bring both to the same height
        while cur1.height > cur2.height {
            let prev_id = match cur1.previous_header_id {
                Some(pid) => pid,
                None => {
                    // Try hash lookup
                    match hash_to_id.get(&cur1.previous_hash) {
                        Some(&pid) => pid,
                        None => return None,
                    }
                }
            };
            cur1 = headers.get(&prev_id)?.clone();
        }
        while cur2.height > cur1.height {
            let prev_id = match cur2.previous_header_id {
                Some(pid) => pid,
                None => match hash_to_id.get(&cur2.previous_hash) {
                    Some(&pid) => pid,
                    None => return None,
                },
            };
            cur2 = headers.get(&prev_id)?.clone();
        }

        // Walk back in parallel until hashes match
        let max_steps = reorg_height_threshold;
        for _ in 0..max_steps {
            if cur1.hash == cur2.hash {
                return Some(cur1);
            }
            let prev1_id = match cur1.previous_header_id {
                Some(pid) => pid,
                None => match hash_to_id.get(&cur1.previous_hash) {
                    Some(&pid) => pid,
                    None => return None,
                },
            };
            let prev2_id = match cur2.previous_header_id {
                Some(pid) => pid,
                None => match hash_to_id.get(&cur2.previous_hash) {
                    Some(&pid) => pid,
                    None => return None,
                },
            };
            cur1 = match headers.get(&prev1_id) {
                Some(h) => h.clone(),
                None => return None,
            };
            cur2 = match headers.get(&prev2_id) {
                Some(h) => h.clone(),
                None => return None,
            };
        }

        if cur1.hash == cur2.hash {
            Some(cur1)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// ChaintracksStorageQuery
// ---------------------------------------------------------------------------

#[async_trait]
impl ChaintracksStorageQuery for MemoryStorage {
    fn chain(&self) -> Chain {
        self.chain.clone()
    }

    fn live_height_threshold(&self) -> u32 {
        self.live_height_threshold
    }

    fn reorg_height_threshold(&self) -> u32 {
        self.reorg_height_threshold
    }

    async fn find_chain_tip_header(&self) -> WalletResult<Option<LiveBlockHeader>> {
        let inner = self.inner.read().unwrap();
        match inner.tip_id {
            Some(id) => Ok(inner.headers.get(&id).cloned()),
            None => Ok(None),
        }
    }

    async fn find_chain_tip_hash(&self) -> WalletResult<Option<String>> {
        let tip = self.find_chain_tip_header().await?;
        Ok(tip.map(|h| h.hash))
    }

    async fn find_header_for_height(&self, height: u32) -> WalletResult<Option<BlockHeader>> {
        let inner = self.inner.read().unwrap();
        match inner.height_to_id.get(&height) {
            Some(&id) => Ok(inner.headers.get(&id).cloned().map(BlockHeader::from)),
            None => Ok(None),
        }
    }

    async fn find_live_header_for_block_hash(
        &self,
        hash: &str,
    ) -> WalletResult<Option<LiveBlockHeader>> {
        let inner = self.inner.read().unwrap();
        match inner.hash_to_id.get(hash) {
            Some(&id) => Ok(inner.headers.get(&id).cloned()),
            None => Ok(None),
        }
    }

    async fn find_live_header_for_merkle_root(
        &self,
        merkle_root: &str,
    ) -> WalletResult<Option<LiveBlockHeader>> {
        let inner = self.inner.read().unwrap();
        match inner.merkle_to_id.get(merkle_root) {
            Some(&id) => Ok(inner.headers.get(&id).cloned()),
            None => Ok(None),
        }
    }

    async fn get_headers_bytes(&self, height: u32, count: u32) -> WalletResult<Vec<u8>> {
        let mut result = Vec::with_capacity(count as usize * 80);
        let inner = self.inner.read().unwrap();

        for h in height..height + count {
            if let Some(&id) = inner.height_to_id.get(&h) {
                if let Some(header) = inner.headers.get(&id) {
                    let base = BaseBlockHeader {
                        version: header.version,
                        previous_hash: header.previous_hash.clone(),
                        merkle_root: header.merkle_root.clone(),
                        time: header.time,
                        bits: header.bits,
                        nonce: header.nonce,
                    };
                    result.extend_from_slice(&base.to_bytes());
                }
            }
        }

        Ok(result)
    }

    async fn get_live_headers(&self) -> WalletResult<Vec<LiveBlockHeader>> {
        let inner = self.inner.read().unwrap();
        let mut all: Vec<LiveBlockHeader> = inner.headers.values().cloned().collect();
        all.sort_by(|a, b| b.height.cmp(&a.height));
        Ok(all)
    }

    async fn get_available_height_ranges(&self) -> WalletResult<Vec<HeightRange>> {
        // In-memory storage doesn't track bulk ranges
        Ok(Vec::new())
    }

    async fn find_live_height_range(&self) -> WalletResult<Option<HeightRange>> {
        let inner = self.inner.read().unwrap();
        let headers = &inner.headers;
        if headers.is_empty() {
            return Ok(None);
        }
        let mut min_h = u32::MAX;
        let mut max_h = u32::MIN;
        for h in headers.values() {
            if h.height < min_h {
                min_h = h.height;
            }
            if h.height > max_h {
                max_h = h.height;
            }
        }
        Ok(Some(HeightRange::new(min_h, max_h)))
    }

    async fn find_common_ancestor(
        &self,
        h1: &LiveBlockHeader,
        h2: &LiveBlockHeader,
    ) -> WalletResult<Option<LiveBlockHeader>> {
        let inner = self.inner.read().unwrap();

        let id1 = match h1.header_id {
            Some(id) => id,
            None => match inner.hash_to_id.get(&h1.hash) {
                Some(&id) => id,
                None => return Ok(None),
            },
        };
        let id2 = match h2.header_id {
            Some(id) => id,
            None => match inner.hash_to_id.get(&h2.hash) {
                Some(&id) => id,
                None => return Ok(None),
            },
        };

        Ok(Self::find_common_ancestor_sync(
            self.reorg_height_threshold,
            &inner.headers,
            &inner.hash_to_id,
            id1,
            id2,
        ))
    }

    async fn find_reorg_depth(&self, new_header: &LiveBlockHeader) -> WalletResult<u32> {
        let tip = self.find_chain_tip_header().await?;
        match tip {
            Some(tip_header) => {
                if tip_header.hash == new_header.previous_hash {
                    // Extends current tip, no reorg
                    return Ok(0);
                }
                let ancestor = self.find_common_ancestor(&tip_header, new_header).await?;
                match ancestor {
                    Some(a) => Ok(tip_header.height - a.height),
                    None => Ok(0),
                }
            }
            None => Ok(0),
        }
    }
}

// ---------------------------------------------------------------------------
// ChaintracksStorageIngest
// ---------------------------------------------------------------------------

#[async_trait]
impl ChaintracksStorageIngest for MemoryStorage {
    async fn insert_header(&self, mut header: LiveBlockHeader) -> WalletResult<InsertHeaderResult> {
        // Calculate chain_work before acquiring the lock (pure computation).
        if header.chain_work.is_empty() {
            header.chain_work = calculate_work(header.bits);
        }

        let mut inner = self.inner.write().unwrap();
        let mut result = InsertHeaderResult::default();

        // 1. Check duplicate by hash (atomic with insertion -- no TOCTOU race)
        if inner.hash_to_id.contains_key(&header.hash) {
            result.dupe = true;
            return Ok(result);
        }

        // 2. Allocate ID
        let id = inner.next_id;
        inner.next_id += 1;
        header.header_id = Some(id);

        // 3. Find previous header and link
        match inner.hash_to_id.get(&header.previous_hash).copied() {
            Some(prev_id) => {
                header.previous_header_id = Some(prev_id);
            }
            None => {
                if header.previous_hash != ZERO_HASH {
                    result.no_prev = true;
                }
            }
        }

        // 4. Determine if this header becomes the new tip
        let old_tip_id = inner.tip_id;
        let old_tip = old_tip_id.and_then(|tid| inner.headers.get(&tid).cloned());

        let mut becomes_tip = false;

        match &old_tip {
            Some(tip) => {
                if header.height > tip.height {
                    becomes_tip = true;
                }
            }
            None => {
                becomes_tip = true;
                result.no_tip = true;
            }
        }

        if becomes_tip {
            header.is_active = true;
            header.is_chain_tip = true;
            result.is_active_tip = true;
            result.added = true;

            // Check if reorg
            let is_reorg = if let Some(ref tip) = old_tip {
                header.previous_hash != tip.hash
            } else {
                false
            };

            // Unmark old tip
            if let Some(ref tip) = old_tip {
                let old_id = old_tip_id.unwrap();
                if let Some(old) = inner.headers.get_mut(&old_id) {
                    old.is_chain_tip = false;
                }
                result.prior_tip = Some(BlockHeader::from(tip.clone()));
            }

            // Insert the new header into all indexes
            inner.headers.insert(id, header.clone());
            inner.hash_to_id.insert(header.hash.clone(), id);
            inner.height_to_id.insert(header.height, id);
            inner.merkle_to_id.insert(header.merkle_root.clone(), id);

            // Handle reorg
            if is_reorg {
                if let Some(old_id) = old_tip_id {
                    let (depth, deactivated) = self.handle_reorg(&mut inner, old_id, id);
                    result.reorg_depth = depth;
                    result.deactivated_headers = deactivated;
                }
            }

            // Update tip
            inner.tip_id = Some(id);
        } else {
            result.added = true;

            // Insert into indexes (not active, not tip)
            inner.headers.insert(id, header.clone());
            inner.hash_to_id.insert(header.hash.clone(), id);
            // Don't add to height_to_id or merkle_to_id since not active
        }

        Ok(result)
    }

    async fn prune_live_block_headers(&self, active_tip_height: u32) -> WalletResult<u32> {
        let threshold = active_tip_height.saturating_sub(self.live_height_threshold);
        let mut inner = self.inner.write().unwrap();

        let to_remove: Vec<u64> = inner
            .headers
            .iter()
            .filter(|(_, h)| !h.is_active && h.height < threshold)
            .map(|(&id, _)| id)
            .collect();

        let count = to_remove.len() as u32;
        for id in to_remove {
            if let Some(h) = inner.headers.remove(&id) {
                inner.hash_to_id.remove(&h.hash);
                inner.height_to_id.remove(&h.height);
                inner.merkle_to_id.remove(&h.merkle_root);
            }
        }

        Ok(count)
    }

    async fn migrate_live_to_bulk(&self, _count: u32) -> WalletResult<u32> {
        Ok(0)
    }

    async fn delete_older_live_block_headers(&self, max_height: u32) -> WalletResult<u32> {
        let mut inner = self.inner.write().unwrap();

        let to_remove: Vec<u64> = inner
            .headers
            .iter()
            .filter(|(_, h)| h.height <= max_height)
            .map(|(&id, _)| id)
            .collect();

        let count = to_remove.len() as u32;
        for id in to_remove {
            if let Some(h) = inner.headers.remove(&id) {
                inner.hash_to_id.remove(&h.hash);
                inner.height_to_id.remove(&h.height);
                inner.merkle_to_id.remove(&h.merkle_root);
            }
        }

        // If deleted headers included the tip, clear it
        if let Some(tid) = inner.tip_id {
            if !inner.headers.contains_key(&tid) {
                inner.tip_id = None;
            }
        }

        Ok(count)
    }

    async fn make_available(&self) -> WalletResult<()> {
        Ok(())
    }

    async fn migrate_latest(&self) -> WalletResult<()> {
        Ok(())
    }

    async fn drop_all_data(&self) -> WalletResult<()> {
        let mut inner = self.inner.write().unwrap();
        inner.headers.clear();
        inner.hash_to_id.clear();
        inner.height_to_id.clear();
        inner.merkle_to_id.clear();
        inner.next_id = 1;
        inner.tip_id = None;
        Ok(())
    }

    async fn destroy(&self) -> WalletResult<()> {
        self.drop_all_data().await
    }
}

// ---------------------------------------------------------------------------
// ChaintracksStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl ChaintracksStorage for MemoryStorage {
    fn storage_type(&self) -> &str {
        "memory"
    }

    async fn is_available(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_header(height: u32, prev_hash: &str, hash: &str) -> LiveBlockHeader {
        LiveBlockHeader {
            version: 1,
            previous_hash: prev_hash.to_string(),
            merkle_root: format!("merkle_{}", hash),
            time: 1231006505 + height,
            bits: 0x1d00ffff,
            nonce: height,
            height,
            hash: hash.to_string(),
            chain_work: calculate_work(0x1d00ffff),
            is_chain_tip: false,
            is_active: false,
            header_id: None,
            previous_header_id: None,
        }
    }

    #[tokio::test]
    async fn test_memory_storage_basic() {
        let storage = MemoryStorage::new(Chain::Main);
        assert_eq!(storage.storage_type(), "memory");
        assert!(storage.is_available().await);
        assert_eq!(storage.header_count(), 0);
        let tip = storage.find_chain_tip_header().await.unwrap();
        assert!(tip.is_none());
    }

    #[tokio::test]
    async fn test_chain_growth() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        let block1 = create_test_header(1, "hash0", "hash1");
        let block2 = create_test_header(2, "hash1", "hash2");

        storage.insert_header(genesis).await.unwrap();
        storage.insert_header(block1).await.unwrap();
        storage.insert_header(block2).await.unwrap();

        let tip = storage.find_chain_tip_header().await.unwrap().unwrap();
        assert_eq!(tip.height, 2);
        assert_eq!(tip.hash, "hash2");

        let h1 = storage.find_header_for_height(1).await.unwrap().unwrap();
        assert_eq!(h1.hash, "hash1");
    }

    #[tokio::test]
    async fn test_duplicate_detection() {
        let storage = MemoryStorage::new(Chain::Main);

        let header = create_test_header(0, ZERO_HASH, "hash0");
        let r1 = storage.insert_header(header.clone()).await.unwrap();
        assert!(r1.added);
        assert!(!r1.dupe);

        let r2 = storage.insert_header(header).await.unwrap();
        assert!(r2.dupe);
        assert_eq!(storage.header_count(), 1);
    }

    #[tokio::test]
    async fn test_hash_lookup() {
        let storage = MemoryStorage::new(Chain::Main);

        let header = create_test_header(0, ZERO_HASH, "hash0");
        storage.insert_header(header).await.unwrap();

        let found = storage
            .find_live_header_for_block_hash("hash0")
            .await
            .unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().hash, "hash0");

        let not_found = storage
            .find_live_header_for_block_hash("nonexistent")
            .await
            .unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_merkle_root_lookup() {
        let storage = MemoryStorage::new(Chain::Main);

        let header = create_test_header(0, ZERO_HASH, "hash0");
        storage.insert_header(header).await.unwrap();

        // Active header should be findable by merkle root
        let found = storage
            .find_live_header_for_merkle_root("merkle_hash0")
            .await
            .unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().merkle_root, "merkle_hash0");
    }

    #[tokio::test]
    async fn test_live_height_range() {
        let storage = MemoryStorage::new(Chain::Main);

        let range = storage.find_live_height_range().await.unwrap();
        assert!(range.is_none());

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        let block1 = create_test_header(1, "hash0", "hash1");
        storage.insert_header(genesis).await.unwrap();
        storage.insert_header(block1).await.unwrap();

        let range = storage.find_live_height_range().await.unwrap().unwrap();
        assert_eq!(range.low, 0);
        assert_eq!(range.high, 1);
    }

    #[tokio::test]
    async fn test_storage_type_and_chain() {
        let storage = MemoryStorage::new(Chain::Test);
        assert_eq!(storage.storage_type(), "memory");
        assert_eq!(storage.chain(), Chain::Test);
    }

    #[tokio::test]
    async fn test_default_thresholds() {
        let storage = MemoryStorage::new(Chain::Main);
        assert_eq!(storage.live_height_threshold(), 2000);
        assert_eq!(storage.reorg_height_threshold(), 400);
    }

    #[tokio::test]
    async fn test_custom_thresholds() {
        let storage = MemoryStorage::with_thresholds(Chain::Main, 500, 100);
        assert_eq!(storage.live_height_threshold(), 500);
        assert_eq!(storage.reorg_height_threshold(), 100);
    }

    #[tokio::test]
    async fn test_headers_bytes_serialization() {
        let storage = MemoryStorage::new(Chain::Main);

        let header = create_test_header(0, ZERO_HASH, "hash0");
        storage.insert_header(header).await.unwrap();

        let bytes = storage.get_headers_bytes(0, 1).await.unwrap();
        assert_eq!(bytes.len(), 80);
    }

    #[tokio::test]
    async fn test_headers_bytes_multiple() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        let block1 = create_test_header(1, "hash0", "hash1");
        storage.insert_header(genesis).await.unwrap();
        storage.insert_header(block1).await.unwrap();

        let bytes = storage.get_headers_bytes(0, 2).await.unwrap();
        assert_eq!(bytes.len(), 160);
    }

    #[tokio::test]
    async fn test_drop_all_data() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        storage.insert_header(genesis).await.unwrap();
        assert_eq!(storage.header_count(), 1);

        storage.drop_all_data().await.unwrap();
        assert_eq!(storage.header_count(), 0);
        assert!(storage.find_chain_tip_header().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_destroy() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        storage.insert_header(genesis).await.unwrap();
        assert_eq!(storage.header_count(), 1);

        storage.destroy().await.unwrap();
        assert_eq!(storage.header_count(), 0);
        assert!(storage.find_chain_tip_header().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_make_available_and_migrate() {
        let storage = MemoryStorage::new(Chain::Main);
        storage.make_available().await.unwrap();
        storage.migrate_latest().await.unwrap();
        // No-ops succeed without error
    }

    #[tokio::test]
    async fn test_available_height_ranges() {
        let storage = MemoryStorage::new(Chain::Main);
        let ranges = storage.get_available_height_ranges().await.unwrap();
        assert!(ranges.is_empty());
    }

    #[tokio::test]
    async fn test_migrate_live_to_bulk() {
        let storage = MemoryStorage::new(Chain::Main);
        let count = storage.migrate_live_to_bulk(100).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_no_prev_header() {
        let storage = MemoryStorage::new(Chain::Main);

        // Insert an orphan header whose previous_hash doesn't point to any stored header
        let orphan = create_test_header(5, "unknown_prev", "orphan_hash");
        let result = storage.insert_header(orphan).await.unwrap();
        assert!(result.no_prev);
    }

    #[tokio::test]
    async fn test_get_live_headers_sorted() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        let block1 = create_test_header(1, "hash0", "hash1");
        let block2 = create_test_header(2, "hash1", "hash2");
        storage.insert_header(genesis).await.unwrap();
        storage.insert_header(block1).await.unwrap();
        storage.insert_header(block2).await.unwrap();

        let live = storage.get_live_headers().await.unwrap();
        assert_eq!(live.len(), 3);
        // Sorted descending by height
        assert_eq!(live[0].height, 2);
        assert_eq!(live[1].height, 1);
        assert_eq!(live[2].height, 0);
    }

    #[tokio::test]
    async fn test_get_active_headers() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        let block1 = create_test_header(1, "hash0", "hash1");
        storage.insert_header(genesis).await.unwrap();
        storage.insert_header(block1).await.unwrap();

        let active = storage.get_active_headers();
        // All headers in a linear chain should be active
        assert!(active.len() >= 1);
        for h in &active {
            assert!(h.is_active);
        }
    }

    #[tokio::test]
    async fn test_get_fork_headers_empty() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        let block1 = create_test_header(1, "hash0", "hash1");
        storage.insert_header(genesis).await.unwrap();
        storage.insert_header(block1).await.unwrap();

        let forks = storage.get_fork_headers();
        // Linear chain should have no forks
        assert!(forks.is_empty());
    }

    #[tokio::test]
    async fn test_find_children() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        let block1 = create_test_header(1, "hash0", "hash1");
        storage.insert_header(genesis).await.unwrap();
        storage.insert_header(block1).await.unwrap();

        let children = storage.find_children("hash0");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].hash, "hash1");
    }

    #[tokio::test]
    async fn test_merkle_root_lookup_inactive() {
        let storage = MemoryStorage::new(Chain::Main);

        // Lookup a merkle root that doesn't exist
        let found = storage
            .find_live_header_for_merkle_root("nonexistent_merkle")
            .await
            .unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_pruning() {
        let storage = MemoryStorage::with_thresholds(Chain::Main, 2, 400);

        // Build a chain of 5 blocks
        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        let block1 = create_test_header(1, "hash0", "hash1");
        let block2 = create_test_header(2, "hash1", "hash2");
        let block3 = create_test_header(3, "hash2", "hash3");
        let block4 = create_test_header(4, "hash3", "hash4");
        storage.insert_header(genesis).await.unwrap();
        storage.insert_header(block1).await.unwrap();
        storage.insert_header(block2).await.unwrap();
        storage.insert_header(block3).await.unwrap();
        storage.insert_header(block4).await.unwrap();

        assert_eq!(storage.header_count(), 5);

        // Add a fork header at height 2 (not active)
        let mut fork = create_test_header(2, "hash1", "fork_hash2");
        fork.is_active = false;
        // Insert it -- it won't become tip because the chain already has height 4
        storage.insert_header(fork).await.unwrap();
        assert_eq!(storage.header_count(), 6);

        // Prune with active tip height 4, threshold 2 => remove inactive below height 2
        let pruned = storage.prune_live_block_headers(4).await.unwrap();
        // threshold = tip - live_height_threshold = 4 - 2 = 2.
        // Pruning removes inactive headers with height < 2.
        // All main-chain headers (heights 0-4) are active; the fork at height 2 is
        // inactive but height 2 is NOT below the threshold of 2, so nothing is pruned.
        assert_eq!(pruned, 0);
    }

    #[tokio::test]
    async fn test_delete_older() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        let block1 = create_test_header(1, "hash0", "hash1");
        let block2 = create_test_header(2, "hash1", "hash2");
        storage.insert_header(genesis).await.unwrap();
        storage.insert_header(block1).await.unwrap();
        storage.insert_header(block2).await.unwrap();

        assert_eq!(storage.header_count(), 3);

        let deleted = storage.delete_older_live_block_headers(1).await.unwrap();
        assert_eq!(deleted, 2); // heights 0 and 1
        assert_eq!(storage.header_count(), 1);
    }

    #[tokio::test]
    async fn test_common_ancestor_same_chain() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        let block1 = create_test_header(1, "hash0", "hash1");
        let block2 = create_test_header(2, "hash1", "hash2");
        storage.insert_header(genesis).await.unwrap();
        storage.insert_header(block1).await.unwrap();
        storage.insert_header(block2).await.unwrap();

        let h1 = storage
            .find_live_header_for_block_hash("hash1")
            .await
            .unwrap()
            .unwrap();
        let h2 = storage
            .find_live_header_for_block_hash("hash2")
            .await
            .unwrap()
            .unwrap();

        let ancestor = storage.find_common_ancestor(&h1, &h2).await.unwrap();
        assert!(ancestor.is_some());
        let a = ancestor.unwrap();
        assert_eq!(a.hash, "hash1");
    }

    #[tokio::test]
    async fn test_find_reorg_depth() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        let block1 = create_test_header(1, "hash0", "hash1");
        storage.insert_header(genesis).await.unwrap();
        storage.insert_header(block1).await.unwrap();

        // A header extending the current tip should have reorg depth 0
        let block2 = create_test_header(2, "hash1", "hash2");
        let depth = storage.find_reorg_depth(&block2).await.unwrap();
        assert_eq!(depth, 0);
    }

    #[tokio::test]
    async fn test_batch_insert() {
        let storage = MemoryStorage::new(Chain::Main);

        let headers = vec![
            create_test_header(0, ZERO_HASH, "hash0"),
            create_test_header(1, "hash0", "hash1"),
            create_test_header(2, "hash1", "hash2"),
        ];

        let results = storage.insert_headers_batch(headers).await.unwrap();
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.added));
        assert_eq!(storage.header_count(), 3);
    }

    #[tokio::test]
    async fn test_get_headers_at_height() {
        let storage = MemoryStorage::new(Chain::Main);

        let genesis = create_test_header(0, ZERO_HASH, "hash0");
        storage.insert_header(genesis).await.unwrap();

        let at_0 = storage.get_headers_at_height(0);
        assert_eq!(at_0.len(), 1);
        assert_eq!(at_0[0].hash, "hash0");

        let at_99 = storage.get_headers_at_height(99);
        assert!(at_99.is_empty());
    }

    #[tokio::test]
    async fn test_reorg_execution() {
        let storage = MemoryStorage::new(Chain::Test);

        // Build main chain: genesis -> A -> B
        let genesis = create_test_header(0, &"0".repeat(64), "hash_0");
        storage.insert_header(genesis).await.unwrap();
        let a = create_test_header(1, "hash_0", "hash_a");
        storage.insert_header(a).await.unwrap();
        let b = create_test_header(2, "hash_a", "hash_b");
        storage.insert_header(b).await.unwrap();

        // Verify tip is B at height 2
        let tip = storage.find_chain_tip_header().await.unwrap().unwrap();
        assert_eq!(tip.hash, "hash_b");

        // Build competing fork from genesis: genesis -> C -> D -> E (longer)
        let c = create_test_header(1, "hash_0", "hash_c");
        storage.insert_header(c).await.unwrap();
        let d = create_test_header(2, "hash_c", "hash_d");
        storage.insert_header(d).await.unwrap();
        let e = create_test_header(3, "hash_d", "hash_e");
        let r = storage.insert_header(e).await.unwrap();

        // Verify reorg happened
        assert!(r.added);
        assert!(r.is_active_tip);
        assert!(
            r.reorg_depth > 0,
            "Expected reorg_depth > 0, got {}",
            r.reorg_depth
        );
        assert!(
            !r.deactivated_headers.is_empty(),
            "Expected deactivated headers"
        );

        // New tip should be E at height 3
        let tip = storage.find_chain_tip_header().await.unwrap().unwrap();
        assert_eq!(tip.hash, "hash_e");
        assert_eq!(tip.height, 3);

        // Old chain headers (A, B) should be inactive
        let a_header = storage
            .find_live_header_for_block_hash("hash_a")
            .await
            .unwrap()
            .unwrap();
        assert!(!a_header.is_active, "hash_a should be inactive after reorg");
        let b_header = storage
            .find_live_header_for_block_hash("hash_b")
            .await
            .unwrap()
            .unwrap();
        assert!(!b_header.is_active, "hash_b should be inactive after reorg");

        // New chain headers should be active and findable by height
        let h1 = storage.find_header_for_height(1).await.unwrap().unwrap();
        assert_eq!(h1.hash, "hash_c");
        let h2 = storage.find_header_for_height(2).await.unwrap().unwrap();
        assert_eq!(h2.hash, "hash_d");
    }
}

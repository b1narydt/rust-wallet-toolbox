//! Entity merge logic for convergent merging during sync.
//!
//! When two storage providers have copies of the same entity, we need to
//! determine which version to keep. The default strategy is: incoming wins
//! if its updated_at is newer, otherwise keep existing.
//!
//! Entity types with special merge rules override the default behavior.

use chrono::NaiveDateTime;

use crate::tables::*;

/// Trait for determining how to merge two copies of the same entity.
///
/// Returns true if the existing entity should be updated with values from incoming.
pub trait MergeEntity {
    /// The primary key ID of this entity.
    fn entity_id(&self) -> i64;

    /// The updated_at timestamp of this entity.
    fn entity_updated_at(&self) -> NaiveDateTime;

    /// Determine whether the incoming entity should replace the existing one.
    /// Default: incoming wins if it has a newer updated_at.
    fn should_update(&self, incoming: &Self) -> bool {
        incoming.entity_updated_at() > self.entity_updated_at()
    }
}

// ---------------------------------------------------------------------------
// MergeEntity implementations for all syncable entity types
// ---------------------------------------------------------------------------

impl MergeEntity for ProvenTx {
    fn entity_id(&self) -> i64 {
        self.proven_tx_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

impl MergeEntity for OutputBasket {
    fn entity_id(&self) -> i64 {
        self.basket_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

impl MergeEntity for Transaction {
    fn entity_id(&self) -> i64 {
        self.transaction_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

impl MergeEntity for Output {
    fn entity_id(&self) -> i64 {
        self.output_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

impl MergeEntity for TxLabel {
    fn entity_id(&self) -> i64 {
        self.tx_label_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

impl MergeEntity for TxLabelMap {
    /// TxLabelMap uses tx_label_id as its primary identity for the sync map.
    fn entity_id(&self) -> i64 {
        // Composite key entity -- no single primary ID.
        // Use tx_label_id as a convention; actual matching is by natural key.
        self.tx_label_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

impl MergeEntity for OutputTag {
    fn entity_id(&self) -> i64 {
        self.output_tag_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

impl MergeEntity for OutputTagMap {
    /// OutputTagMap uses output_tag_id as its primary identity for the sync map.
    fn entity_id(&self) -> i64 {
        // Composite key entity -- no single primary ID.
        self.output_tag_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

impl MergeEntity for Certificate {
    fn entity_id(&self) -> i64 {
        self.certificate_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

impl MergeEntity for CertificateField {
    /// CertificateField uses certificate_id as its primary identity for the sync map.
    fn entity_id(&self) -> i64 {
        // Composite key entity -- no single primary ID.
        self.certificate_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

impl MergeEntity for Commission {
    fn entity_id(&self) -> i64 {
        self.commission_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

impl MergeEntity for ProvenTxReq {
    fn entity_id(&self) -> i64 {
        self.proven_tx_req_id
    }

    fn entity_updated_at(&self) -> NaiveDateTime {
        self.updated_at
    }
}

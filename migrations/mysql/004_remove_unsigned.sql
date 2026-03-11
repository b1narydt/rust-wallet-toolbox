-- Remove UNSIGNED from INT/BIGINT columns to match Rust i32/i64 types.
-- sqlx 0.8 treats INT UNSIGNED as incompatible with Rust i32.

ALTER TABLE proven_txs MODIFY COLUMN height INT NOT NULL;
ALTER TABLE proven_txs MODIFY COLUMN `index` INT NOT NULL;
ALTER TABLE proven_tx_reqs MODIFY COLUMN attempts INT NOT NULL DEFAULT 0;
ALTER TABLE transactions MODIFY COLUMN version INT;
ALTER TABLE transactions MODIFY COLUMN lockTime INT;
ALTER TABLE outputs MODIFY COLUMN sequenceNumber INT;
ALTER TABLE outputs MODIFY COLUMN scriptLength BIGINT;
ALTER TABLE outputs MODIFY COLUMN scriptOffset BIGINT;

-- Convert VARCHAR columns backing Rust enums to MySQL ENUM types.
-- sqlx's #[derive(Type)] on Rust enums maps to MySQL ENUM, not VARCHAR.

ALTER TABLE settings
    MODIFY COLUMN chain ENUM('main', 'test') NOT NULL;

ALTER TABLE proven_tx_reqs
    MODIFY COLUMN status ENUM('sending', 'unsent', 'nosend', 'unknown', 'nonfinal', 'unprocessed', 'unmined', 'callback', 'unconfirmed', 'completed', 'invalid', 'doubleSpend', 'unfail') NOT NULL DEFAULT 'unknown';

ALTER TABLE transactions
    MODIFY COLUMN status ENUM('completed', 'failed', 'unprocessed', 'sending', 'unproven', 'unsigned', 'nosend', 'nonfinal', 'unfail') NOT NULL;

ALTER TABLE outputs
    MODIFY COLUMN providedBy ENUM('storage', 'you') NOT NULL;

ALTER TABLE sync_states
    MODIFY COLUMN status ENUM('success', 'error', 'identified', 'updated', 'unknown') NOT NULL DEFAULT 'unknown';

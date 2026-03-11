-- Revert ENUM columns back to VARCHAR.
-- sqlx 0.8 derive(Type) on Rust enums has strict ENUM compatibility checks
-- that fail on MySQL. Using VARCHAR with manual string encode/decode instead.

ALTER TABLE settings
    MODIFY COLUMN chain VARCHAR(10) NOT NULL;

ALTER TABLE proven_tx_reqs
    MODIFY COLUMN status VARCHAR(16) NOT NULL DEFAULT 'unknown';

ALTER TABLE transactions
    MODIFY COLUMN status VARCHAR(16) NOT NULL;

ALTER TABLE outputs
    MODIFY COLUMN providedBy VARCHAR(10) NOT NULL;

ALTER TABLE sync_states
    MODIFY COLUMN status VARCHAR(16) NOT NULL DEFAULT 'unknown';

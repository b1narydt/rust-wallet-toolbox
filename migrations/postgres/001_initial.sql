-- PostgreSQL initial migration
-- Source: Translated from wallet-toolbox/src/storage/schema/KnexMigrations.ts
-- Column names are camelCase to match TS Knex schema for cross-language DB compatibility.

CREATE TABLE proven_txs (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "provenTxId" SERIAL PRIMARY KEY,
    txid VARCHAR(64) NOT NULL UNIQUE,
    height INTEGER NOT NULL,
    "index" INTEGER NOT NULL,
    "merklePath" BYTEA NOT NULL,
    "rawTx" BYTEA NOT NULL,
    "blockHash" VARCHAR(64) NOT NULL,
    "merkleRoot" VARCHAR(64) NOT NULL
);
CREATE INDEX idx_proven_txs_blockHash ON proven_txs("blockHash");

CREATE TABLE proven_tx_reqs (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "provenTxReqId" SERIAL PRIMARY KEY,
    "provenTxId" INTEGER REFERENCES proven_txs("provenTxId"),
    status VARCHAR(16) NOT NULL DEFAULT 'unknown',
    attempts INTEGER NOT NULL DEFAULT 0,
    notified BOOLEAN NOT NULL DEFAULT FALSE,
    txid VARCHAR(64) NOT NULL UNIQUE,
    batch VARCHAR(64),
    history TEXT NOT NULL DEFAULT '{}',
    notify TEXT NOT NULL DEFAULT '{}',
    "rawTx" BYTEA NOT NULL,
    "inputBEEF" BYTEA
);
CREATE INDEX idx_proven_tx_reqs_status ON proven_tx_reqs(status);
CREATE INDEX idx_proven_tx_reqs_batch ON proven_tx_reqs(batch);
CREATE INDEX idx_proven_tx_reqs_txid ON proven_tx_reqs(txid);

CREATE TABLE users (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "userId" SERIAL PRIMARY KEY,
    "identityKey" VARCHAR(130) NOT NULL UNIQUE,
    "activeStorage" VARCHAR(130) NOT NULL
);

CREATE TABLE certificates (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "certificateId" SERIAL PRIMARY KEY,
    "userId" INTEGER NOT NULL REFERENCES users("userId"),
    "serialNumber" VARCHAR(100) NOT NULL,
    type VARCHAR(100) NOT NULL,
    certifier VARCHAR(100) NOT NULL,
    subject VARCHAR(100) NOT NULL,
    verifier VARCHAR(100),
    "revocationOutpoint" VARCHAR(100) NOT NULL,
    signature VARCHAR(255) NOT NULL,
    "isDeleted" BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE("userId", type, certifier, "serialNumber")
);

CREATE TABLE certificate_fields (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "userId" INTEGER NOT NULL REFERENCES users("userId"),
    "certificateId" INTEGER NOT NULL REFERENCES certificates("certificateId"),
    "fieldName" VARCHAR(100) NOT NULL,
    "fieldValue" VARCHAR(255) NOT NULL,
    "masterKey" VARCHAR(255) NOT NULL DEFAULT '',
    UNIQUE("fieldName", "certificateId")
);

CREATE TABLE output_baskets (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "basketId" SERIAL PRIMARY KEY,
    "userId" INTEGER NOT NULL REFERENCES users("userId"),
    name VARCHAR(300) NOT NULL,
    "numberOfDesiredUTXOs" INTEGER NOT NULL DEFAULT 6,
    "minimumDesiredUTXOValue" INTEGER NOT NULL DEFAULT 10000,
    "isDeleted" BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE(name, "userId")
);

CREATE TABLE transactions (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "transactionId" SERIAL PRIMARY KEY,
    "userId" INTEGER NOT NULL REFERENCES users("userId"),
    "provenTxId" INTEGER REFERENCES proven_txs("provenTxId"),
    status VARCHAR(64) NOT NULL,
    reference VARCHAR(64) NOT NULL UNIQUE,
    "isOutgoing" BOOLEAN NOT NULL,
    satoshis BIGINT NOT NULL DEFAULT 0,
    version INTEGER,
    "lockTime" INTEGER,
    description VARCHAR(2048) NOT NULL,
    txid VARCHAR(64),
    "inputBEEF" BYTEA,
    "rawTx" BYTEA
);
CREATE INDEX idx_transactions_status ON transactions(status);
CREATE INDEX idx_transactions_txid ON transactions(txid);

CREATE TABLE commissions (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "commissionId" SERIAL PRIMARY KEY,
    "userId" INTEGER NOT NULL REFERENCES users("userId"),
    "transactionId" INTEGER NOT NULL UNIQUE REFERENCES transactions("transactionId"),
    satoshis INTEGER NOT NULL,
    "keyOffset" VARCHAR(130) NOT NULL,
    "isRedeemed" BOOLEAN NOT NULL DEFAULT FALSE,
    "lockingScript" BYTEA NOT NULL
);
CREATE INDEX idx_commissions_transactionId ON commissions("transactionId");

CREATE TABLE outputs (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "outputId" SERIAL PRIMARY KEY,
    "userId" INTEGER NOT NULL REFERENCES users("userId"),
    "transactionId" INTEGER NOT NULL REFERENCES transactions("transactionId"),
    "basketId" INTEGER REFERENCES output_baskets("basketId"),
    spendable BOOLEAN NOT NULL DEFAULT FALSE,
    change BOOLEAN NOT NULL DEFAULT FALSE,
    vout INTEGER NOT NULL,
    satoshis BIGINT NOT NULL,
    "providedBy" VARCHAR(130) NOT NULL,
    purpose VARCHAR(20) NOT NULL,
    type VARCHAR(50) NOT NULL,
    "outputDescription" VARCHAR(2048),
    txid VARCHAR(64),
    "senderIdentityKey" VARCHAR(130),
    "derivationPrefix" VARCHAR(200),
    "derivationSuffix" VARCHAR(200),
    "customInstructions" VARCHAR(2500),
    "spentBy" INTEGER REFERENCES transactions("transactionId"),
    "sequenceNumber" INTEGER,
    "spendingDescription" VARCHAR(2048),
    "scriptLength" BIGINT,
    "scriptOffset" BIGINT,
    "lockingScript" BYTEA,
    UNIQUE("transactionId", vout, "userId")
);
CREATE INDEX idx_outputs_spendable ON outputs(spendable);
CREATE INDEX idx_outputs_user_spendable_outputid ON outputs("userId", spendable, "outputId");
CREATE INDEX idx_outputs_user_basket_spendable_outputid ON outputs("userId", "basketId", spendable, "outputId");
CREATE INDEX idx_outputs_user_basket_spendable_satoshis ON outputs("userId", "basketId", spendable, satoshis);
CREATE INDEX idx_outputs_spentby ON outputs("spentBy");

CREATE TABLE output_tags (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "outputTagId" SERIAL PRIMARY KEY,
    "userId" INTEGER NOT NULL REFERENCES users("userId"),
    tag VARCHAR(150) NOT NULL,
    "isDeleted" BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE(tag, "userId")
);

CREATE TABLE output_tags_map (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "outputTagId" INTEGER NOT NULL REFERENCES output_tags("outputTagId"),
    "outputId" INTEGER NOT NULL REFERENCES outputs("outputId"),
    "isDeleted" BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE("outputTagId", "outputId")
);
CREATE INDEX idx_output_tags_map_outputId ON output_tags_map("outputId");
CREATE INDEX idx_output_tags_map_output_deleted_tag ON output_tags_map("outputId", "isDeleted", "outputTagId");

CREATE TABLE tx_labels (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "txLabelId" SERIAL PRIMARY KEY,
    "userId" INTEGER NOT NULL REFERENCES users("userId"),
    label VARCHAR(300) NOT NULL,
    "isDeleted" BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE(label, "userId")
);

CREATE TABLE tx_labels_map (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "txLabelId" INTEGER NOT NULL REFERENCES tx_labels("txLabelId"),
    "transactionId" INTEGER NOT NULL REFERENCES transactions("transactionId"),
    "isDeleted" BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE("txLabelId", "transactionId")
);
CREATE INDEX idx_tx_labels_map_transactionId ON tx_labels_map("transactionId");
CREATE INDEX idx_tx_labels_map_tx_deleted ON tx_labels_map("transactionId", "isDeleted");

CREATE TABLE monitor_events (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    id SERIAL PRIMARY KEY,
    event VARCHAR(64) NOT NULL,
    details TEXT
);
CREATE INDEX idx_monitor_events_event ON monitor_events(event);

CREATE TABLE settings (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "storageIdentityKey" VARCHAR(130) NOT NULL,
    "storageName" VARCHAR(128) NOT NULL,
    chain VARCHAR(10) NOT NULL,
    dbtype VARCHAR(10) NOT NULL,
    "maxOutputScript" INTEGER NOT NULL
);

CREATE TABLE sync_states (
    created_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP(3) NOT NULL DEFAULT NOW(),
    "syncStateId" SERIAL PRIMARY KEY,
    "userId" INTEGER NOT NULL REFERENCES users("userId"),
    "storageIdentityKey" VARCHAR(130) NOT NULL DEFAULT '',
    "storageName" TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'unknown',
    init BOOLEAN NOT NULL DEFAULT FALSE,
    "refNum" VARCHAR(100) NOT NULL UNIQUE,
    "syncMap" TEXT NOT NULL,
    "when" TIMESTAMP(3),
    satoshis BIGINT,
    "errorLocal" TEXT,
    "errorOther" TEXT
);
CREATE INDEX idx_sync_states_status ON sync_states(status);
CREATE INDEX idx_sync_states_refNum ON sync_states("refNum");

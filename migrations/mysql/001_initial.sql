-- MySQL initial migration
-- Source: Translated from wallet-toolbox/src/storage/schema/KnexMigrations.ts
-- Column names are camelCase to match TS Knex schema for cross-language DB compatibility.

CREATE TABLE proven_txs (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    provenTxId INT AUTO_INCREMENT PRIMARY KEY,
    txid VARCHAR(64) NOT NULL UNIQUE,
    height INT UNSIGNED NOT NULL,
    `index` INT UNSIGNED NOT NULL,
    merklePath LONGBLOB NOT NULL,
    rawTx LONGBLOB NOT NULL,
    blockHash VARCHAR(64) NOT NULL,
    merkleRoot VARCHAR(64) NOT NULL,
    INDEX idx_proven_txs_blockHash (blockHash)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE proven_tx_reqs (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    provenTxReqId INT AUTO_INCREMENT PRIMARY KEY,
    provenTxId INT UNSIGNED,
    status VARCHAR(16) NOT NULL DEFAULT 'unknown',
    attempts INT UNSIGNED NOT NULL DEFAULT 0,
    notified BOOLEAN NOT NULL DEFAULT FALSE,
    txid VARCHAR(64) NOT NULL UNIQUE,
    batch VARCHAR(64),
    history LONGTEXT NOT NULL DEFAULT ('{}'),
    notify LONGTEXT NOT NULL DEFAULT ('{}'),
    rawTx LONGBLOB NOT NULL,
    inputBEEF LONGBLOB,
    INDEX idx_proven_tx_reqs_status (status),
    INDEX idx_proven_tx_reqs_batch (batch),
    INDEX idx_proven_tx_reqs_txid (txid),
    FOREIGN KEY (provenTxId) REFERENCES proven_txs(provenTxId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE users (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    userId INT AUTO_INCREMENT PRIMARY KEY,
    identityKey VARCHAR(130) NOT NULL UNIQUE,
    activeStorage VARCHAR(130) NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE certificates (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    certificateId INT AUTO_INCREMENT PRIMARY KEY,
    userId INT UNSIGNED NOT NULL,
    serialNumber VARCHAR(100) NOT NULL,
    type VARCHAR(100) NOT NULL,
    certifier VARCHAR(100) NOT NULL,
    subject VARCHAR(100) NOT NULL,
    verifier VARCHAR(100),
    revocationOutpoint VARCHAR(100) NOT NULL,
    signature VARCHAR(255) NOT NULL,
    isDeleted BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE(userId, type, certifier, serialNumber),
    FOREIGN KEY (userId) REFERENCES users(userId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE certificate_fields (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    userId INT UNSIGNED NOT NULL,
    certificateId INT UNSIGNED NOT NULL,
    fieldName VARCHAR(100) NOT NULL,
    fieldValue VARCHAR(255) NOT NULL,
    masterKey VARCHAR(255) NOT NULL DEFAULT '',
    UNIQUE(fieldName, certificateId),
    FOREIGN KEY (userId) REFERENCES users(userId),
    FOREIGN KEY (certificateId) REFERENCES certificates(certificateId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE output_baskets (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    basketId INT AUTO_INCREMENT PRIMARY KEY,
    userId INT UNSIGNED NOT NULL,
    name VARCHAR(300) NOT NULL,
    numberOfDesiredUTXOs INT NOT NULL DEFAULT 6,
    minimumDesiredUTXOValue INT NOT NULL DEFAULT 10000,
    isDeleted BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE(name, userId),
    FOREIGN KEY (userId) REFERENCES users(userId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE transactions (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    transactionId INT AUTO_INCREMENT PRIMARY KEY,
    userId INT UNSIGNED NOT NULL,
    provenTxId INT UNSIGNED,
    status VARCHAR(64) NOT NULL,
    reference VARCHAR(64) NOT NULL UNIQUE,
    isOutgoing BOOLEAN NOT NULL,
    satoshis BIGINT NOT NULL DEFAULT 0,
    version INT UNSIGNED,
    lockTime INT UNSIGNED,
    description VARCHAR(2048) NOT NULL,
    txid VARCHAR(64),
    inputBEEF LONGBLOB,
    rawTx LONGBLOB,
    INDEX idx_transactions_status (status),
    INDEX idx_transactions_txid (txid),
    FOREIGN KEY (userId) REFERENCES users(userId),
    FOREIGN KEY (provenTxId) REFERENCES proven_txs(provenTxId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE commissions (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    commissionId INT AUTO_INCREMENT PRIMARY KEY,
    userId INT UNSIGNED NOT NULL,
    transactionId INT UNSIGNED NOT NULL UNIQUE,
    satoshis INT NOT NULL,
    keyOffset VARCHAR(130) NOT NULL,
    isRedeemed BOOLEAN NOT NULL DEFAULT FALSE,
    lockingScript LONGBLOB NOT NULL,
    INDEX idx_commissions_transactionId (transactionId),
    FOREIGN KEY (userId) REFERENCES users(userId),
    FOREIGN KEY (transactionId) REFERENCES transactions(transactionId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE outputs (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    outputId INT AUTO_INCREMENT PRIMARY KEY,
    userId INT UNSIGNED NOT NULL,
    transactionId INT UNSIGNED NOT NULL,
    basketId INT UNSIGNED,
    spendable BOOLEAN NOT NULL DEFAULT FALSE,
    `change` BOOLEAN NOT NULL DEFAULT FALSE,
    vout INT NOT NULL,
    satoshis BIGINT NOT NULL,
    providedBy VARCHAR(130) NOT NULL,
    purpose VARCHAR(20) NOT NULL,
    type VARCHAR(50) NOT NULL,
    outputDescription VARCHAR(2048),
    txid VARCHAR(64),
    senderIdentityKey VARCHAR(130),
    derivationPrefix VARCHAR(200),
    derivationSuffix VARCHAR(200),
    customInstructions VARCHAR(2500),
    spentBy INT UNSIGNED,
    sequenceNumber INT UNSIGNED,
    spendingDescription VARCHAR(2048),
    scriptLength BIGINT UNSIGNED,
    scriptOffset BIGINT UNSIGNED,
    lockingScript LONGBLOB,
    UNIQUE(transactionId, vout, userId),
    INDEX idx_outputs_spendable (spendable),
    INDEX idx_outputs_user_spendable_outputid (userId, spendable, outputId),
    INDEX idx_outputs_user_basket_spendable_outputid (userId, basketId, spendable, outputId),
    INDEX idx_outputs_user_basket_spendable_satoshis (userId, basketId, spendable, satoshis),
    INDEX idx_outputs_spentby (spentBy),
    FOREIGN KEY (userId) REFERENCES users(userId),
    FOREIGN KEY (transactionId) REFERENCES transactions(transactionId),
    FOREIGN KEY (basketId) REFERENCES output_baskets(basketId),
    FOREIGN KEY (spentBy) REFERENCES transactions(transactionId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE output_tags (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    outputTagId INT AUTO_INCREMENT PRIMARY KEY,
    userId INT UNSIGNED NOT NULL,
    tag VARCHAR(150) NOT NULL,
    isDeleted BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE(tag, userId),
    FOREIGN KEY (userId) REFERENCES users(userId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE output_tags_map (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    outputTagId INT UNSIGNED NOT NULL,
    outputId INT UNSIGNED NOT NULL,
    isDeleted BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE(outputTagId, outputId),
    INDEX idx_output_tags_map_outputId (outputId),
    INDEX idx_output_tags_map_output_deleted_tag (outputId, isDeleted, outputTagId),
    FOREIGN KEY (outputTagId) REFERENCES output_tags(outputTagId),
    FOREIGN KEY (outputId) REFERENCES outputs(outputId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE tx_labels (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    txLabelId INT AUTO_INCREMENT PRIMARY KEY,
    userId INT UNSIGNED NOT NULL,
    label VARCHAR(300) NOT NULL,
    isDeleted BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE(label, userId),
    FOREIGN KEY (userId) REFERENCES users(userId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE tx_labels_map (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    txLabelId INT UNSIGNED NOT NULL,
    transactionId INT UNSIGNED NOT NULL,
    isDeleted BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE(txLabelId, transactionId),
    INDEX idx_tx_labels_map_transactionId (transactionId),
    INDEX idx_tx_labels_map_tx_deleted (transactionId, isDeleted),
    FOREIGN KEY (txLabelId) REFERENCES tx_labels(txLabelId),
    FOREIGN KEY (transactionId) REFERENCES transactions(transactionId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE monitor_events (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    id INT AUTO_INCREMENT PRIMARY KEY,
    event VARCHAR(64) NOT NULL,
    details LONGTEXT,
    INDEX idx_monitor_events_event (event)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE settings (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    storageIdentityKey VARCHAR(130) NOT NULL,
    storageName VARCHAR(128) NOT NULL,
    chain VARCHAR(10) NOT NULL,
    dbtype VARCHAR(10) NOT NULL,
    maxOutputScript INT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE sync_states (
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    syncStateId INT AUTO_INCREMENT PRIMARY KEY,
    userId INT UNSIGNED NOT NULL,
    storageIdentityKey VARCHAR(130) NOT NULL DEFAULT '',
    storageName VARCHAR(255) NOT NULL,
    status VARCHAR(255) NOT NULL DEFAULT 'unknown',
    init BOOLEAN NOT NULL DEFAULT FALSE,
    refNum VARCHAR(100) NOT NULL UNIQUE,
    syncMap LONGTEXT NOT NULL,
    `when` DATETIME(3),
    satoshis BIGINT,
    errorLocal LONGTEXT,
    errorOther LONGTEXT,
    INDEX idx_sync_states_status (status),
    INDEX idx_sync_states_refNum (refNum),
    FOREIGN KEY (userId) REFERENCES users(userId)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

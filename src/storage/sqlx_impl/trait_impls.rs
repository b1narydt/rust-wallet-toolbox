//! Trait implementations wiring StorageReaderWriter and StorageProvider
//! for StorageSqlx backends (SQLite, MySQL, PostgreSQL).
//! Delegates to the concrete insert/update methods defined in insert.rs
//! and update.rs. MySQL and PostgreSQL impls are generated via the
//! `impl_storage_rw_and_provider!` macro.

#[cfg(feature = "sqlite")]
mod sqlite_impl {
    use std::sync::atomic::Ordering;

    use async_trait::async_trait;
    use sqlx::Sqlite;

    use crate::error::{WalletError, WalletResult};
    use crate::storage::find_args::*;
    use crate::storage::sqlx_impl::StorageSqlx;
    use crate::storage::traits::provider::StorageProvider;
    use crate::storage::traits::reader::StorageReader;
    use crate::storage::traits::reader_writer::StorageReaderWriter;
    use crate::storage::TrxToken;
    use crate::tables::*;
    use crate::types::Chain;

    #[async_trait]
    impl StorageReaderWriter for StorageSqlx<Sqlite> {
        async fn begin_transaction(&self) -> WalletResult<TrxToken> {
            self.begin_sqlite_transaction().await
        }

        async fn commit_transaction(&self, trx: TrxToken) -> WalletResult<()> {
            let inner = trx
                .downcast::<crate::storage::sqlx_impl::SqliteTrxInner>()
                .map_err(|_| {
                    WalletError::Internal(
                        "TrxToken does not contain a SQLite transaction".to_string(),
                    )
                })?;
            let mut guard = inner.lock().await;
            let tx = guard
                .take()
                .ok_or_else(|| WalletError::Internal("Transaction already consumed".to_string()))?;
            tx.commit().await?;
            Ok(())
        }

        async fn rollback_transaction(&self, trx: TrxToken) -> WalletResult<()> {
            let inner = trx
                .downcast::<crate::storage::sqlx_impl::SqliteTrxInner>()
                .map_err(|_| {
                    WalletError::Internal(
                        "TrxToken does not contain a SQLite transaction".to_string(),
                    )
                })?;
            let mut guard = inner.lock().await;
            let tx = guard
                .take()
                .ok_or_else(|| WalletError::Internal("Transaction already consumed".to_string()))?;
            tx.rollback().await?;
            Ok(())
        }

        // ----- Insert methods -----

        async fn insert_user(&self, user: &User, trx: Option<&TrxToken>) -> WalletResult<i64> {
            self.insert_user_impl(user, trx).await
        }

        async fn insert_certificate(
            &self,
            certificate: &Certificate,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.insert_certificate_impl(certificate, trx).await
        }

        async fn insert_certificate_field(
            &self,
            field: &CertificateField,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            self.insert_certificate_field_impl(field, trx).await
        }

        async fn insert_commission(
            &self,
            commission: &Commission,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.insert_commission_impl(commission, trx).await
        }

        async fn insert_monitor_event(
            &self,
            event: &MonitorEvent,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.insert_monitor_event_impl(event, trx).await
        }

        async fn insert_output_basket(
            &self,
            basket: &OutputBasket,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.insert_output_basket_impl(basket, trx).await
        }

        async fn insert_output_tag(
            &self,
            tag: &OutputTag,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.insert_output_tag_impl(tag, trx).await
        }

        async fn insert_output_tag_map(
            &self,
            tag_map: &OutputTagMap,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            self.insert_output_tag_map_impl(tag_map, trx).await
        }

        async fn insert_output(
            &self,
            output: &Output,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.insert_output_impl(output, trx).await
        }

        async fn insert_proven_tx(
            &self,
            proven_tx: &ProvenTx,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.insert_proven_tx_impl(proven_tx, trx).await
        }

        async fn insert_proven_tx_req(
            &self,
            proven_tx_req: &ProvenTxReq,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.insert_proven_tx_req_impl(proven_tx_req, trx).await
        }

        async fn insert_transaction(
            &self,
            transaction: &Transaction,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.insert_transaction_impl(transaction, trx).await
        }

        async fn insert_tx_label(
            &self,
            label: &TxLabel,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.insert_tx_label_impl(label, trx).await
        }

        async fn insert_tx_label_map(
            &self,
            label_map: &TxLabelMap,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            self.insert_tx_label_map_impl(label_map, trx).await
        }

        async fn insert_sync_state(
            &self,
            sync_state: &SyncState,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.insert_sync_state_impl(sync_state, trx).await
        }

        // ----- Update methods -----

        async fn update_user(
            &self,
            id: i64,
            update: &UserPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_user_impl(id, update, trx).await
        }

        async fn update_certificate(
            &self,
            id: i64,
            update: &CertificatePartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_certificate_impl(id, update, trx).await
        }

        async fn update_certificate_field(
            &self,
            certificate_id: i64,
            field_name: &str,
            update: &CertificateFieldPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_certificate_field_impl(certificate_id, field_name, update, trx)
                .await
        }

        async fn update_commission(
            &self,
            id: i64,
            update: &CommissionPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_commission_impl(id, update, trx).await
        }

        async fn update_monitor_event(
            &self,
            id: i64,
            update: &MonitorEventPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_monitor_event_impl(id, update, trx).await
        }

        async fn update_output_basket(
            &self,
            id: i64,
            update: &OutputBasketPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_output_basket_impl(id, update, trx).await
        }

        async fn update_output_tag(
            &self,
            id: i64,
            update: &OutputTagPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_output_tag_impl(id, update, trx).await
        }

        async fn update_output_tag_map(
            &self,
            output_id: i64,
            tag_id: i64,
            update: &OutputTagMapPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_output_tag_map_impl(output_id, tag_id, update, trx)
                .await
        }

        async fn update_output(
            &self,
            id: i64,
            update: &OutputPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_output_impl(id, update, trx).await
        }

        async fn update_proven_tx(
            &self,
            id: i64,
            update: &ProvenTxPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_proven_tx_impl(id, update, trx).await
        }

        async fn update_proven_tx_req(
            &self,
            id: i64,
            update: &ProvenTxReqPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_proven_tx_req_impl(id, update, trx).await
        }

        async fn update_settings(
            &self,
            update: &SettingsPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_settings_impl(update, trx).await
        }

        async fn update_transaction(
            &self,
            id: i64,
            update: &TransactionPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_transaction_impl(id, update, trx).await
        }

        async fn update_tx_label(
            &self,
            id: i64,
            update: &TxLabelPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_tx_label_impl(id, update, trx).await
        }

        async fn update_tx_label_map(
            &self,
            transaction_id: i64,
            tx_label_id: i64,
            update: &TxLabelMapPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_tx_label_map_impl(transaction_id, tx_label_id, update, trx)
                .await
        }

        async fn update_sync_state(
            &self,
            id: i64,
            update: &SyncStatePartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            self.update_sync_state_impl(id, update, trx).await
        }

        // ----- Monitor-related composite methods (Phase 6) -----

        async fn update_transaction_status(
            &self,
            txid: &str,
            new_status: crate::status::TransactionStatus,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            use crate::storage::traits::reader::StorageReader;

            // Find the transaction by txid
            let txs = self
                .find_transactions(
                    &FindTransactionsArgs {
                        partial: TransactionPartial {
                            txid: Some(txid.to_string()),
                            ..Default::default()
                        },
                        no_raw_tx: true,
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            let tx = txs.first().ok_or_else(|| WalletError::InvalidParameter {
                parameter: "txid".to_string(),
                must_be: format!("an existing transaction, '{}' not found", txid),
            })?;

            // Update transaction status
            self.update_transaction_impl(
                tx.transaction_id,
                &TransactionPartial {
                    status: Some(new_status.clone()),
                    ..Default::default()
                },
                trx,
            )
            .await?;

            // If setting to Failed, release locked UTXOs
            if new_status == crate::status::TransactionStatus::Failed {
                let outputs = self
                    .find_outputs(
                        &FindOutputsArgs {
                            partial: OutputPartial {
                                spent_by: Some(tx.transaction_id),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        trx,
                    )
                    .await?;

                for output in &outputs {
                    self.update_output_impl(
                        output.output_id,
                        &OutputPartial {
                            spendable: Some(true),
                            spent_by: Some(0),
                            ..Default::default()
                        },
                        trx,
                    )
                    .await?;
                }
            }

            Ok(())
        }

        async fn update_transactions_status(
            &self,
            txids: &[String],
            new_status: crate::status::TransactionStatus,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            for txid in txids {
                self.update_transaction_status(txid, new_status.clone(), trx)
                    .await?;
            }
            Ok(())
        }

        async fn update_proven_tx_req_with_new_proven_tx(
            &self,
            req_id: i64,
            proven_tx: &ProvenTx,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            // Insert the proven tx
            let proven_tx_id = self.insert_proven_tx_impl(proven_tx, trx).await?;

            // Update the proven tx req with the proven_tx_id and status=completed
            self.update_proven_tx_req_impl(
                req_id,
                &ProvenTxReqPartial {
                    proven_tx_id: Some(proven_tx_id),
                    status: Some(crate::status::ProvenTxReqStatus::Completed),
                    ..Default::default()
                },
                trx,
            )
            .await?;

            Ok(proven_tx_id)
        }

        async fn review_status(
            &self,
            aged_limit: chrono::NaiveDateTime,
            trx: Option<&TrxToken>,
        ) -> WalletResult<String> {
            use crate::storage::traits::reader::StorageReader;

            // Find ProvenTxReqs with stale statuses older than aged_limit
            let stale_statuses = vec![
                crate::status::ProvenTxReqStatus::Unsent,
                crate::status::ProvenTxReqStatus::Sending,
            ];

            let reqs = self
                .find_proven_tx_reqs(
                    &FindProvenTxReqsArgs {
                        statuses: Some(stale_statuses),
                        since: None,
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            let mut corrected = 0u32;
            for req in &reqs {
                if req.updated_at < aged_limit {
                    // Reset stale req to unsent for retry
                    self.update_proven_tx_req_impl(
                        req.proven_tx_req_id,
                        &ProvenTxReqPartial {
                            status: Some(crate::status::ProvenTxReqStatus::Unsent),
                            ..Default::default()
                        },
                        trx,
                    )
                    .await?;
                    corrected += 1;
                }
            }

            Ok(format!(
                "review_status: checked {} reqs, corrected {}",
                reqs.len(),
                corrected
            ))
        }

        async fn purge_data(
            &self,
            params: &crate::storage::find_args::PurgeParams,
            _trx: Option<&TrxToken>,
        ) -> WalletResult<String> {
            let mut summary = Vec::new();

            if params.purge_failed {
                let age_ms = params.purge_failed_age;
                let cutoff =
                    chrono::Utc::now().naive_utc() - chrono::Duration::milliseconds(age_ms as i64);
                let sql = "DELETE FROM proven_tx_reqs WHERE status = 'invalid' AND updated_at < ?";
                let result = sqlx::query(sql)
                    .bind(cutoff)
                    .execute(&self.write_pool)
                    .await?;
                summary.push(format!("purged {} failed reqs", result.rows_affected()));
            }

            if params.purge_completed {
                let age_ms = params.purge_completed_age;
                let cutoff =
                    chrono::Utc::now().naive_utc() - chrono::Duration::milliseconds(age_ms as i64);
                let sql =
                    "DELETE FROM proven_tx_reqs WHERE status = 'completed' AND updated_at < ?";
                let result = sqlx::query(sql)
                    .bind(cutoff)
                    .execute(&self.write_pool)
                    .await?;
                summary.push(format!("purged {} completed reqs", result.rows_affected()));
            }

            if params.purge_spent {
                let age_ms = params.purge_spent_age;
                let cutoff =
                    chrono::Utc::now().naive_utc() - chrono::Duration::milliseconds(age_ms as i64);
                let sql = "DELETE FROM outputs WHERE spendable = 0 AND spent_by IS NOT NULL AND updated_at < ?";
                let result = sqlx::query(sql)
                    .bind(cutoff)
                    .execute(&self.write_pool)
                    .await?;
                summary.push(format!("purged {} spent outputs", result.rows_affected()));
            }

            Ok(summary.join("; "))
        }
    }

    // -----------------------------------------------------------------------
    // StorageProvider implementation
    // -----------------------------------------------------------------------

    #[async_trait]
    impl StorageProvider for StorageSqlx<Sqlite> {
        async fn migrate_database(&self) -> WalletResult<String> {
            crate::migrations::run_sqlite_migrations(&self.write_pool).await?;
            Ok("SQLite migrations completed".to_string())
        }

        async fn get_settings(&self, trx: Option<&TrxToken>) -> WalletResult<Settings> {
            // First check cache
            {
                let guard = self.settings.read().await;
                if let Some(ref s) = *guard {
                    return Ok(s.clone());
                }
            }
            // Not cached, query from DB
            let args = FindSettingsArgs::default();
            let results = StorageReader::find_settings(self, &args, trx).await?;
            let settings = crate::storage::verify_one(results)?;
            // Cache it
            {
                let mut guard = self.settings.write().await;
                *guard = Some(settings.clone());
            }
            Ok(settings)
        }

        async fn make_available(&self) -> WalletResult<Settings> {
            self.migrate_database().await?;

            // Try to load settings; if none exist, create defaults
            let args = FindSettingsArgs::default();
            let results = StorageReader::find_settings(self, &args, None).await?;
            let settings = if let Some(s) = results.into_iter().next() {
                s
            } else {
                // Create default settings
                let now = chrono::Utc::now().naive_utc();
                let new_settings = Settings {
                    created_at: now,
                    updated_at: now,
                    storage_identity_key: self.storage_identity_key.clone(),
                    storage_name: String::from("default"),
                    chain: self.chain.clone(),
                    dbtype: String::from("SQLite"),
                    max_output_script: 100,
                };
                self.insert_settings_impl(&new_settings, None).await?;
                new_settings
            };

            // Cache and activate
            {
                let mut guard = self.settings.write().await;
                *guard = Some(settings.clone());
            }
            self.active.store(true, Ordering::Relaxed);
            Ok(settings)
        }

        async fn destroy(&self) -> WalletResult<()> {
            self.active.store(false, Ordering::Relaxed);
            self.write_pool.close().await;
            if let Some(ref rp) = self.read_pool {
                rp.close().await;
            }
            Ok(())
        }

        async fn drop_all_data(&self) -> WalletResult<()> {
            // Delete from all tables in reverse dependency order
            let tables = [
                "output_tags_map",
                "tx_labels_map",
                "certificate_fields",
                "commissions",
                "outputs",
                "output_tags",
                "output_baskets",
                "tx_labels",
                "transactions",
                "proven_tx_reqs",
                "proven_txs",
                "certificates",
                "sync_states",
                "monitor_events",
                "settings",
                "users",
            ];
            for table in &tables {
                let sql = format!("DELETE FROM {}", table);
                sqlx::query(&sql).execute(&self.write_pool).await?;
            }
            Ok(())
        }

        fn get_storage_identity_key(&self) -> WalletResult<String> {
            Ok(self.storage_identity_key.clone())
        }

        fn is_available(&self) -> bool {
            self.active.load(Ordering::Relaxed)
        }

        fn get_chain(&self) -> Chain {
            self.chain.clone()
        }

        fn set_active(&self, active: bool) {
            self.active.store(active, Ordering::Relaxed);
        }

        fn is_active(&self) -> bool {
            self.active.load(Ordering::Relaxed)
        }
    }
}

// ---------------------------------------------------------------------------
// Macro for MySQL/PostgreSQL StorageReaderWriter + StorageProvider
// ---------------------------------------------------------------------------

macro_rules! impl_storage_rw_and_provider {
    (
        feature = $feature:literal,
        mod_name = $mod_name:ident,
        db = $db:ty,
        trx_inner = $trx_inner:ty,
        begin_trx = $begin_trx:ident,
        extract_trx = $extract_trx:path,
        migrate_fn = $migrate_fn:path,
        trx_name = $trx_name:literal,
        dbtype_str = $dbtype_str:literal
    ) => {
        #[cfg(feature = $feature)]
        mod $mod_name {
            use std::sync::atomic::Ordering;

            use async_trait::async_trait;

            use crate::error::{WalletError, WalletResult};
            use crate::storage::find_args::*;
            use crate::storage::sqlx_impl::StorageSqlx;
            use crate::storage::traits::provider::StorageProvider;
            use crate::storage::traits::reader::StorageReader;
            use crate::storage::traits::reader_writer::StorageReaderWriter;
            use crate::storage::TrxToken;
            use crate::tables::*;
            use crate::types::Chain;

            #[async_trait]
            impl StorageReaderWriter for StorageSqlx<$db> {
                async fn begin_transaction(&self) -> WalletResult<TrxToken> {
                    self.$begin_trx().await
                }

                async fn commit_transaction(&self, trx: TrxToken) -> WalletResult<()> {
                    let inner = trx.downcast::<$trx_inner>().map_err(|_| {
                        WalletError::Internal(
                            concat!("TrxToken does not contain a ", $trx_name, " transaction")
                                .to_string(),
                        )
                    })?;
                    let mut guard = inner.lock().await;
                    let tx = guard.take().ok_or_else(|| {
                        WalletError::Internal("Transaction already consumed".to_string())
                    })?;
                    tx.commit().await?;
                    Ok(())
                }

                async fn rollback_transaction(&self, trx: TrxToken) -> WalletResult<()> {
                    let inner = trx.downcast::<$trx_inner>().map_err(|_| {
                        WalletError::Internal(
                            concat!("TrxToken does not contain a ", $trx_name, " transaction")
                                .to_string(),
                        )
                    })?;
                    let mut guard = inner.lock().await;
                    let tx = guard.take().ok_or_else(|| {
                        WalletError::Internal("Transaction already consumed".to_string())
                    })?;
                    tx.rollback().await?;
                    Ok(())
                }

                // Insert methods
                async fn insert_user(
                    &self,
                    user: &User,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_user_impl(user, trx).await
                }
                async fn insert_certificate(
                    &self,
                    cert: &Certificate,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_certificate_impl(cert, trx).await
                }
                async fn insert_certificate_field(
                    &self,
                    field: &CertificateField,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<()> {
                    self.insert_certificate_field_impl(field, trx).await
                }
                async fn insert_commission(
                    &self,
                    c: &Commission,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_commission_impl(c, trx).await
                }
                async fn insert_monitor_event(
                    &self,
                    e: &MonitorEvent,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_monitor_event_impl(e, trx).await
                }
                async fn insert_output_basket(
                    &self,
                    b: &OutputBasket,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_output_basket_impl(b, trx).await
                }
                async fn insert_output_tag(
                    &self,
                    t: &OutputTag,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_output_tag_impl(t, trx).await
                }
                async fn insert_output_tag_map(
                    &self,
                    m: &OutputTagMap,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<()> {
                    self.insert_output_tag_map_impl(m, trx).await
                }
                async fn insert_output(
                    &self,
                    o: &Output,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_output_impl(o, trx).await
                }
                async fn insert_proven_tx(
                    &self,
                    p: &ProvenTx,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_proven_tx_impl(p, trx).await
                }
                async fn insert_proven_tx_req(
                    &self,
                    r: &ProvenTxReq,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_proven_tx_req_impl(r, trx).await
                }
                async fn insert_transaction(
                    &self,
                    t: &Transaction,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_transaction_impl(t, trx).await
                }
                async fn insert_tx_label(
                    &self,
                    l: &TxLabel,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_tx_label_impl(l, trx).await
                }
                async fn insert_tx_label_map(
                    &self,
                    m: &TxLabelMap,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<()> {
                    self.insert_tx_label_map_impl(m, trx).await
                }
                async fn insert_sync_state(
                    &self,
                    ss: &SyncState,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.insert_sync_state_impl(ss, trx).await
                }

                // Update methods
                async fn update_user(
                    &self,
                    id: i64,
                    u: &UserPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_user_impl(id, u, trx).await
                }
                async fn update_certificate(
                    &self,
                    id: i64,
                    u: &CertificatePartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_certificate_impl(id, u, trx).await
                }
                async fn update_certificate_field(
                    &self,
                    cid: i64,
                    fname: &str,
                    u: &CertificateFieldPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_certificate_field_impl(cid, fname, u, trx).await
                }
                async fn update_commission(
                    &self,
                    id: i64,
                    u: &CommissionPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_commission_impl(id, u, trx).await
                }
                async fn update_monitor_event(
                    &self,
                    id: i64,
                    u: &MonitorEventPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_monitor_event_impl(id, u, trx).await
                }
                async fn update_output_basket(
                    &self,
                    id: i64,
                    u: &OutputBasketPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_output_basket_impl(id, u, trx).await
                }
                async fn update_output_tag(
                    &self,
                    id: i64,
                    u: &OutputTagPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_output_tag_impl(id, u, trx).await
                }
                async fn update_output_tag_map(
                    &self,
                    oid: i64,
                    tid: i64,
                    u: &OutputTagMapPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_output_tag_map_impl(oid, tid, u, trx).await
                }
                async fn update_output(
                    &self,
                    id: i64,
                    u: &OutputPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_output_impl(id, u, trx).await
                }
                async fn update_proven_tx(
                    &self,
                    id: i64,
                    u: &ProvenTxPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_proven_tx_impl(id, u, trx).await
                }
                async fn update_proven_tx_req(
                    &self,
                    id: i64,
                    u: &ProvenTxReqPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_proven_tx_req_impl(id, u, trx).await
                }
                async fn update_settings(
                    &self,
                    u: &SettingsPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_settings_impl(u, trx).await
                }
                async fn update_transaction(
                    &self,
                    id: i64,
                    u: &TransactionPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_transaction_impl(id, u, trx).await
                }
                async fn update_tx_label(
                    &self,
                    id: i64,
                    u: &TxLabelPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_tx_label_impl(id, u, trx).await
                }
                async fn update_tx_label_map(
                    &self,
                    tid: i64,
                    lid: i64,
                    u: &TxLabelMapPartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_tx_label_map_impl(tid, lid, u, trx).await
                }
                async fn update_sync_state(
                    &self,
                    id: i64,
                    u: &SyncStatePartial,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    self.update_sync_state_impl(id, u, trx).await
                }
            }

            #[async_trait]
            impl StorageProvider for StorageSqlx<$db> {
                async fn migrate_database(&self) -> WalletResult<String> {
                    $migrate_fn(&self.write_pool).await?;
                    Ok(concat!($trx_name, " migrations completed").to_string())
                }

                async fn get_settings(&self, trx: Option<&TrxToken>) -> WalletResult<Settings> {
                    {
                        let guard = self.settings.read().await;
                        if let Some(ref s) = *guard {
                            return Ok(s.clone());
                        }
                    }
                    let args = FindSettingsArgs::default();
                    let results = StorageReader::find_settings(self, &args, trx).await?;
                    let settings = crate::storage::verify_one(results)?;
                    {
                        let mut guard = self.settings.write().await;
                        *guard = Some(settings.clone());
                    }
                    Ok(settings)
                }

                async fn make_available(&self) -> WalletResult<Settings> {
                    self.migrate_database().await?;
                    let args = FindSettingsArgs::default();
                    let results = StorageReader::find_settings(self, &args, None).await?;
                    let settings = if let Some(s) = results.into_iter().next() {
                        s
                    } else {
                        let now = chrono::Utc::now().naive_utc();
                        let new_settings = Settings {
                            created_at: now,
                            updated_at: now,
                            storage_identity_key: self.storage_identity_key.clone(),
                            storage_name: String::from("default"),
                            chain: self.chain.clone(),
                            dbtype: String::from($dbtype_str),
                            max_output_script: 100,
                        };
                        self.insert_settings_impl(&new_settings, None).await?;
                        new_settings
                    };
                    {
                        let mut guard = self.settings.write().await;
                        *guard = Some(settings.clone());
                    }
                    self.active.store(true, Ordering::Relaxed);
                    Ok(settings)
                }

                async fn destroy(&self) -> WalletResult<()> {
                    self.active.store(false, Ordering::Relaxed);
                    self.write_pool.close().await;
                    if let Some(ref rp) = self.read_pool {
                        rp.close().await;
                    }
                    Ok(())
                }

                async fn drop_all_data(&self) -> WalletResult<()> {
                    let tables = [
                        "output_tags_map",
                        "tx_labels_map",
                        "certificate_fields",
                        "commissions",
                        "outputs",
                        "output_tags",
                        "output_baskets",
                        "tx_labels",
                        "transactions",
                        "proven_tx_reqs",
                        "proven_txs",
                        "certificates",
                        "sync_states",
                        "monitor_events",
                        "settings",
                        "users",
                    ];
                    for table in &tables {
                        let sql = format!("DELETE FROM {}", table);
                        sqlx::query(&sql).execute(&self.write_pool).await?;
                    }
                    Ok(())
                }

                fn get_storage_identity_key(&self) -> WalletResult<String> {
                    Ok(self.storage_identity_key.clone())
                }

                fn is_available(&self) -> bool {
                    self.active.load(Ordering::Relaxed)
                }

                fn get_chain(&self) -> Chain {
                    self.chain.clone()
                }

                fn set_active(&self, active: bool) {
                    self.active.store(active, Ordering::Relaxed);
                }

                fn is_active(&self) -> bool {
                    self.active.load(Ordering::Relaxed)
                }
            }
        }
    };
}

impl_storage_rw_and_provider! {
    feature = "mysql",
    mod_name = mysql_trait_impls,
    db = sqlx::MySql,
    trx_inner = crate::storage::sqlx_impl::MysqlTrxInner,
    begin_trx = begin_mysql_transaction,
    extract_trx = StorageSqlx::<sqlx::MySql>::extract_mysql_trx,
    migrate_fn = crate::migrations::run_mysql_migrations,
    trx_name = "MySQL",
    dbtype_str = "MySQL"
}

impl_storage_rw_and_provider! {
    feature = "postgres",
    mod_name = postgres_trait_impls,
    db = sqlx::Postgres,
    trx_inner = crate::storage::sqlx_impl::PgTrxInner,
    begin_trx = begin_pg_transaction,
    extract_trx = StorageSqlx::<sqlx::Postgres>::extract_pg_trx,
    migrate_fn = crate::migrations::run_postgres_migrations,
    trx_name = "PostgreSQL",
    dbtype_str = "PostgreSQL"
}

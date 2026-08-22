//! Arcade broadcaster implementing the Teranode-native `/tx` transport.

use async_trait::async_trait;
use bsv::transaction::Beef;
use futures::future::join_all;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

use crate::services::traits::PostBeefProvider;
use crate::services::types::{ArcConfig, PostBeefResult, PostTxResultForTxid};

use super::arc::build_arc_headers;

const ARCADE_POST_BEEF_CONCURRENCY: usize = 4;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArcadeResponse {
    txid: String,
    #[serde(default)]
    tx_status: String,
    #[serde(default)]
    competing_txs: Option<Vec<String>>,
}

/// Broadcasts each requested BEEF transaction to Arcade as Extended Format.
pub struct ArcadeProvider {
    base_url: String,
    config: ArcConfig,
    client: reqwest::Client,
}

impl ArcadeProvider {
    pub fn new(base_url: &str, config: ArcConfig, client: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            config,
            client,
        }
    }

    pub fn build_headers(&self) -> HeaderMap {
        build_arc_headers(&self.config)
    }

    fn error_result(txid: &str) -> PostTxResultForTxid {
        PostTxResultForTxid {
            txid: txid.to_string(),
            status: "error".to_string(),
            already_known: None,
            double_spend: None,
            block_hash: None,
            block_height: None,
            competing_txs: None,
            service_error: Some(true),
            orphan_mempool: None,
        }
    }

    async fn post_ef(&self, txid: &str, ef_hex: String) -> PostTxResultForTxid {
        let response = self
            .client
            .post(format!("{}/tx", self.base_url))
            .headers(self.build_headers())
            .json(&serde_json::json!({ "rawTx": ef_hex }))
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(_) => return Self::error_result(txid),
        };
        let status = response.status();
        let body = match response.json::<ArcadeResponse>().await {
            Ok(body) => body,
            Err(_) => return Self::error_result(txid),
        };
        let is_double_spend = body.tx_status == "DOUBLE_SPEND_ATTEMPTED";
        let is_orphan_mempool = body.tx_status == "SEEN_IN_ORPHAN_MEMPOOL";
        let accepted = status.is_success() && !is_double_spend && !is_orphan_mempool;
        let service_error = matches!(status.as_u16(), 408 | 429 | 476) || status.is_server_error();

        PostTxResultForTxid {
            txid: body.txid,
            status: if accepted { "success" } else { "error" }.to_string(),
            already_known: None,
            double_spend: is_double_spend.then_some(true),
            block_hash: None,
            block_height: None,
            competing_txs: body.competing_txs,
            service_error: (!accepted).then_some(service_error),
            orphan_mempool: is_orphan_mempool.then_some(true),
        }
    }
}

#[async_trait]
impl PostBeefProvider for ArcadeProvider {
    fn name(&self) -> &str {
        "ArcadeBeef"
    }

    async fn post_beef(&self, beef: &[u8], txids: &[String]) -> PostBeefResult {
        let parsed = Beef::from_binary(&mut std::io::Cursor::new(beef));
        let mut result = PostBeefResult {
            name: self.name().to_string(),
            status: "success".to_string(),
            error: None,
            txid_results: Vec::with_capacity(txids.len()),
        };

        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                result.status = "error".to_string();
                result.error = Some(format!("Failed to parse BEEF: {error}"));
                result.txid_results = txids.iter().map(|txid| Self::error_result(txid)).collect();
                return result;
            }
        };

        let txid_set: HashSet<&str> = txids.iter().map(String::as_str).collect();
        let prepared: Vec<_> = txids
            .iter()
            .map(|txid| {
                let transaction = parsed.find_atomic_transaction(txid);
                let dependencies = transaction
                    .as_ref()
                    .map(|tx| {
                        tx.inputs
                            .iter()
                            .filter_map(|input| input.source_txid.as_deref())
                            .filter(|source_txid| {
                                *source_txid != txid && txid_set.contains(source_txid)
                            })
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let ef_hex = transaction.and_then(|tx| tx.to_hex_ef().ok());
                (txid, ef_hex, dependencies)
            })
            .collect();
        let index_by_txid: HashMap<&str, usize> = txids
            .iter()
            .enumerate()
            .map(|(index, txid)| (txid.as_str(), index))
            .collect();
        let mut pending: HashSet<usize> = (0..prepared.len()).collect();
        let mut txid_results: Vec<Option<PostTxResultForTxid>> =
            (0..prepared.len()).map(|_| None).collect();

        while !pending.is_empty() {
            let mut ready: Vec<usize> = pending
                .iter()
                .copied()
                .filter(|index| {
                    prepared[*index].2.iter().all(|dependency| {
                        index_by_txid
                            .get(dependency.as_str())
                            .is_none_or(|dependency_index| !pending.contains(dependency_index))
                    })
                })
                .collect();
            ready.sort_unstable();
            if ready.is_empty() {
                ready.push(*pending.iter().min().expect("pending is non-empty"));
            }

            for chunk in ready.chunks(ARCADE_POST_BEEF_CONCURRENCY) {
                let prepared = &prepared;
                let posts = chunk.iter().copied().map(|index| async move {
                    let (txid, ef_hex, _) = &prepared[index];
                    let tx_result = match ef_hex {
                        Some(ef_hex) => self.post_ef(txid, ef_hex.clone()).await,
                        None => Self::error_result(txid),
                    };
                    (index, tx_result)
                });
                for (index, tx_result) in join_all(posts).await {
                    txid_results[index] = Some(tx_result);
                }
            }

            for index in ready {
                pending.remove(&index);
            }
        }

        result.txid_results = txid_results
            .into_iter()
            .enumerate()
            .map(|(index, tx_result)| {
                tx_result.unwrap_or_else(|| Self::error_result(txids[index].as_str()))
            })
            .collect();
        if let Some(failed) = result
            .txid_results
            .iter()
            .find(|tx_result| tx_result.status == "error")
        {
            result.status = "error".to_string();
            result.error = Some(body_error_description(
                &failed.txid,
                parsed.find_txid(&failed.txid).is_some(),
            ));
        }

        result
    }
}

fn body_error_description(txid: &str, found: bool) -> String {
    if found {
        format!("Could not build Extended Format transaction {txid}")
    } else {
        format!("Transaction {txid} not found in BEEF")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsv::transaction::beef::BEEF_V2;
    use bsv::transaction::beef_tx::BeefTx;
    use bsv::transaction::Transaction;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn posts_extended_format_to_tx_with_callback_token() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let length = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..length]).to_string();
            let body = r#"{"txid":"unused","txStatus":"RECEIVED"}"#;
            let response = format!(
                "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            request
        });

        let tx = Transaction::new();
        let txid = tx.id().unwrap();
        let mut parsed_beef = Beef::new(BEEF_V2);
        parsed_beef.txs.push(BeefTx::from_tx(tx, None).unwrap());
        let mut beef = Vec::new();
        parsed_beef.to_binary(&mut beef).unwrap();
        let config = ArcConfig {
            callback_token: Some("wallet-token".to_string()),
            ..Default::default()
        };
        let provider =
            ArcadeProvider::new(&format!("http://{address}"), config, reqwest::Client::new());

        let result = provider.post_beef(&beef, &[txid]).await;
        assert_eq!(result.status, "success");

        let request = server.await.unwrap();
        assert!(request.starts_with("POST /tx HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("x-callbacktoken: wallet-token"));
        assert!(request.contains("\"rawTx\""));
    }
}

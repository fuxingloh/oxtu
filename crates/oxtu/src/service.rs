use std::env;
use std::str::FromStr;

use bigdecimal::BigDecimal;
use bitcoincore_rpc::bitcoin::address::Address;
use jsonrpsee::core::async_trait;
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::{ErrorCode, ErrorObjectOwned};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use oxtu_index::db::ScriptInfo;
use oxtu_index::types::U128Decimal;
use oxtu_index::Index;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ListUnspentQueryOptions {
    pub minconf: Option<u64>,
    pub maxconf: Option<u64>,
    pub count: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Utxo {
    pub txid: String,
    pub vout: u32,
    pub address: String,
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: String,
    #[serde(with = "bigdecimal::serde::json_num")]
    pub amount: BigDecimal,
    pub confirmations: u64,
    pub height: u64,
    pub coinbase: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AddressInfo {
    pub address: String,
    #[serde(with = "bigdecimal::serde::json_num")]
    pub balance: BigDecimal,
    #[serde(with = "bigdecimal::serde::json_num")]
    pub total_sent: BigDecimal,
    #[serde(with = "bigdecimal::serde::json_num")]
    pub total_received: BigDecimal,
    pub tx_count: u64,
}

#[rpc(server, client)]
pub trait Rpc {
    /// RPC Method: listunspent
    /// Implements `bitcoin-core` wallet RPC method `listunspent` without indexing wallet.
    /// Instead of returning all UTXOs from a wallet, this method returns all UTXOs from an address.
    ///
    /// Reference:
    /// https://github.com/bitcoin/bitcoin/blob/538363738e9e30813cf3e76ca4f71c1aaff349e7/src/wallet/rpc/coins.cpp#L497
    #[method(name = "listunspent")]
    async fn listunspent(
        &self,
        address: String,
        query_options: Option<ListUnspentQueryOptions>,
    ) -> Result<Vec<Utxo>, ErrorObjectOwned>;

    #[method(name = "getaddressinfo")]
    async fn getaddressinfo(&self, address: String) -> Result<AddressInfo, ErrorObjectOwned>;

    #[method(name = "_probe")]
    async fn probe(&self, name: String) -> Result<(), ErrorObjectOwned>;
}

pub struct OxtuRpcServer {
    pub(crate) index: Index,
}

static MAX_COUNT: Lazy<usize> = Lazy::new(|| {
    env::var("MAX_COUNT")
        .unwrap_or_else(|_| "100".to_string())
        .parse::<usize>()
        .unwrap()
});

#[async_trait]
impl RpcServer for OxtuRpcServer {
    async fn listunspent(
        &self,
        address: String,
        query_options: Option<ListUnspentQueryOptions>,
    ) -> Result<Vec<Utxo>, ErrorObjectOwned> {
        let address_parsed = Address::from_str(&address).unwrap();
        let script = address_parsed.assume_checked().script_pubkey().to_bytes();
        let block_tip = self.index.db.peek().expect("failed to get block tip");

        let lower_bound = query_options
            .as_ref()
            .and_then(|o| o.maxconf)
            .map(|maxconf| {
                block_tip
                    .height
                    .checked_sub(maxconf)
                    .map(|lower| lower + 1)
                    .unwrap_or_else(|| u64::MAX)
            });

        let upper_bound = query_options
            .as_ref()
            .and_then(|o| o.minconf)
            .map(|minconf| {
                // RocksDB upper bound is always excluded hence + 2
                block_tip
                    .height
                    .checked_sub(minconf)
                    .map(|upper| upper + 2)
                    .unwrap_or_else(|| u64::MIN)
            });

        let utxo_iterator = self
            .index
            .db
            .iterator_script_utxo(&script, lower_bound..upper_bound);
        let count = query_options
            .as_ref()
            .and_then(|o| o.count)
            .filter(|&count| count <= *MAX_COUNT)
            .unwrap_or_else(|| *MAX_COUNT);
        let script_pub_key = hex::encode(script);

        let utxos = utxo_iterator
            .take(count)
            .map(|utxo| Utxo {
                txid: utxo.key.vout.txid.to_hex(),
                vout: utxo.key.vout.n,
                address: address.clone(),
                script_pub_key: script_pub_key.clone(),
                amount: utxo.value.into(),
                confirmations: block_tip.height - utxo.key.height + 1,
                height: utxo.key.height,
                coinbase: utxo.coinbase,
            })
            .collect();

        Ok(utxos)
    }

    async fn getaddressinfo(&self, address: String) -> Result<AddressInfo, ErrorObjectOwned> {
        let address_parsed = Address::from_str(&address).unwrap();
        let script = address_parsed.assume_checked().script_pubkey().to_bytes();
        let info = self.index.db.get_script_info(&script).unwrap_or_else(|| {
            const {
                ScriptInfo {
                    script: Vec::new(),
                    balance: U128Decimal::zero(),
                    total_sent: U128Decimal::zero(),
                    total_received: U128Decimal::zero(),
                    tx_count: 0,
                }
            }
        });

        Ok(AddressInfo {
            address,
            balance: info.balance.into(),
            total_sent: info.total_sent.into(),
            total_received: info.total_received.into(),
            tx_count: info.tx_count,
        })
    }

    async fn probe(&self, name: String) -> Result<(), ErrorObjectOwned> {
        match name.as_str() {
            "liveness" => Ok(()),
            "readiness" => match self.index.status().await {
                Ok(status) if status.initial_indexing => Err(ErrorCode::InternalError.into()),
                Ok(_) => Ok(()),
                Err(_) => Err(ErrorCode::InternalError.into()),
            },
            "startup" => Ok(()),
            _ => Err(ErrorCode::InvalidParams.into()),
        }
    }
}

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::watch;
use tokio::task::spawn;

use types::U256;

use crate::rpc::{RpcClient, RpcOptions};

pub mod db;
pub mod rpc;
pub mod types;

#[must_use]
pub struct Index {
    pub db: Arc<db::Db>,
    rpc_client: Arc<RpcClient>,
}

pub struct IndexStatus {
    pub initial_indexing: bool,
}

struct Progress {
    height: u64,
    prev_hash: U256,
}

impl Progress {
    pub fn genesis() -> Self {
        Self {
            height: 0,
            prev_hash: U256::zero(),
        }
    }

    pub fn for_fork(entry: &db::Block) -> Self {
        Self {
            height: entry.height,
            prev_hash: entry.prev_hash,
        }
    }

    pub fn for_next(entry: &db::Block) -> Self {
        Self {
            height: entry.height + 1,
            prev_hash: entry.hash,
        }
    }
}

impl fmt::Debug for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{ height: {}, prev_hash: {} }}",
            self.height, self.prev_hash
        )
    }
}

impl Index {
    pub fn open(path: &str, rpc: RpcOptions) -> Index {
        let db = db::Db::open(path);

        Self {
            db: Arc::new(db),
            rpc_client: Arc::new(RpcClient::new(rpc)),
        }
    }

    pub fn start(&self) -> IndexHandle {
        enum Synced {
            Connected(Box<rpc::Block>),
            Forked,
            Errored(rpc::Error),
        }

        async fn connect(next: &Progress, rpc_client: &RpcClient) -> Synced {
            let next_hash = match rpc_client.get_blockhash(&next.height).await {
                Ok(hash) => hash,
                Err(error) => return Synced::Errored(error),
            };

            let next_block = match rpc_client.get_block(&next_hash).await {
                Ok(block) => block,
                Err(error) => return Synced::Errored(error),
            };

            if let Some(ref parent_hash) = next_block
                .previousblockhash
                .as_ref()
                .map(|hash| U256::from_hex(hash))
            {
                if parent_hash == &next.prev_hash {
                    return Synced::Connected(next_block);
                }

                Synced::Forked
            } else {
                if next_block.height != 0 {
                    panic!("Block height is not 0, previousblockhash is None")
                }

                Synced::Connected(next_block)
            }
        }

        let db = self.db.clone();
        let rpc_client = self.rpc_client.clone();
        let (stop_tx, mut stop_rx) = watch::channel(());

        spawn(async move {
            let mut next: Progress = db
                .peek()
                .as_ref()
                .map(Progress::for_next)
                .unwrap_or_else(Progress::genesis);

            tracing::info!("Started: {:?}", &next);

            let mut sleep_until = SystemTime::now();
            while !stop_rx.has_changed().unwrap() {
                if SystemTime::now() < sleep_until {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }

                // Every 10,000 blocks, we prune the blocks prior to the last 10,000 blocks
                if next.height % 10_000 == 0 && next.height > 10_000 {
                    db.prune_until(next.height - 10_000);
                }

                match connect(&next, &rpc_client).await {
                    Synced::Connected(rpc_block) => {
                        let hash = U256::from_hex(&rpc_block.hash);
                        db.push(*rpc_block);
                        tracing::info!("Connected: {:?}", &next);
                        next = Progress {
                            height: next.height + 1,
                            prev_hash: hash,
                        };
                    }
                    Synced::Forked => {
                        let popped = db.pop();
                        next = Progress::for_fork(&popped);
                        tracing::info!("Forked: {:?}", &next);
                    }
                    Synced::Errored(error) => {
                        tracing::info!("Errored: {:?}, error: {:?}", &next, error);
                        sleep_until = SystemTime::now() + Duration::from_secs(5);
                    }
                }
            }

            stop_rx.changed().await.unwrap();
            tracing::info!("Stopped index");
        });

        IndexHandle(Arc::new(stop_tx))
    }

    pub async fn status(&self) -> Result<IndexStatus, rpc::Error> {
        let height = self.rpc_client.get_block_count().await?;
        match self.db.peek() {
            None => Ok(IndexStatus {
                initial_indexing: true,
            }),
            Some(block) => Ok(IndexStatus {
                initial_indexing: height > block.height + 100,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IndexHandle(Arc<watch::Sender<()>>);

impl IndexHandle {
    pub fn stop(&self) {
        self.0.send(()).unwrap();
    }

    pub async fn stopped(&self) {
        self.0.closed().await
    }
}

#[cfg(test)]
mod tests {
    use bitcoincore_rpc::bitcoin::address::NetworkChecked;
    use bitcoincore_rpc::bitcoin::{Address, Amount};
    use bitcoincore_rpc::RpcApi;
    use tempfile::tempdir;
    use testcontainers::runners::SyncRunner;
    use tracing_test::traced_test;

    use testcontainers_bitcoind::{Bitcoind, Sync};

    use super::*;

    #[test]
    #[traced_test]
    fn index() -> anyhow::Result<()> {
        let bitcoind = Bitcoind::default().start().unwrap();
        let index = {
            let rpc_url = bitcoind.rpc_url()?;
            let rpc_options = match bitcoind.rpc_auth() {
                Some(bitcoincore_rpc::Auth::UserPass(username, password)) => RpcOptions {
                    url: rpc_url,
                    username: Some(username),
                    password: Some(password),
                },
                _ => RpcOptions {
                    url: rpc_url,
                    username: None,
                    password: None,
                },
            };
            Index::open(tempdir().unwrap().path().to_str().unwrap(), rpc_options)
        };

        let client = bitcoind.client().unwrap();
        client
            .create_wallet("test", None, None, None, None)
            .unwrap();
        let address1: Address<NetworkChecked> =
            client.get_new_address(None, None).unwrap().assume_checked();
        let address2: Address<NetworkChecked> =
            client.get_new_address(None, None).unwrap().assume_checked();
        client.generate_to_address(104, &address1).unwrap();

        for _ in 0..100 {
            client
                .send_to_address(
                    &address2,
                    Amount::from_btc(123.9954).unwrap(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            client
                .send_to_address(
                    &address1,
                    Amount::from_btc(9.12345678).unwrap(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            client.generate_to_address(3, &address2).unwrap();
        }

        client
            .send_to_address(
                &address1,
                Amount::from_btc(10000.87654321).unwrap(),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        client.generate_to_address(1, &address2).unwrap();
        client
            .send_to_address(
                &address2,
                Amount::from_btc(10000.12345678).unwrap(),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        client.generate_to_address(15, &address1).unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let handle = index.start();
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            handle.stop();
            handle.stopped().await;
        });

        let block_0 = index.db.get_block(0).unwrap();
        assert_eq!(
            block_0.hash,
            U256::from_hex("0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206")
        );

        index.db.get_block(420).expect("Block 420 not found");

        let script = address1.script_pubkey().to_bytes();
        tracing::info!("UTXOs for script: {:?}", hex::encode(&script));
        let iter = index.db.iterator_script_utxo(&script, None..None);
        let utxos: Vec<db::Utxo> = iter.take(100).collect();
        for utxo in utxos {
            tracing::info!("{:?}", (hex::encode(utxo.key.script), utxo.value));
        }

        Ok(())
    }
}

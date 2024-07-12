use std::env;
use std::net::SocketAddr;
use std::sync::Arc;

use jsonrpsee::server::middleware::rpc::{RpcServiceBuilder, RpcServiceT};
use jsonrpsee::server::Server;
use tokio::net::ToSocketAddrs;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::watch;
use tracing_subscriber::filter::EnvFilter;

use oxtu_index::rpc::RpcOptions;
use oxtu_index::Index;
use service::{OxtuRpcServer, RpcServer};

mod service;

struct LoggingMiddleware<S>(S);

impl<'a, S: RpcServiceT<'a>> RpcServiceT<'a> for LoggingMiddleware<S> {
    type Future = S::Future;

    fn call(&self, request: jsonrpsee::types::Request<'a>) -> Self::Future {
        tracing::info!("Received request: {:?}", request);
        self.0.call(request)
    }
}

#[derive(Debug, Clone)]
pub struct OxtuHandle {
    addr: SocketAddr,
    stop_handle: Arc<watch::Sender<()>>,
}

impl OxtuHandle {
    pub fn stop(&self) {
        self.stop_handle.send(()).unwrap();
    }

    pub async fn stopped(&self) {
        self.stop_handle.closed().await
    }
}

async fn start_oxtu(addrs: impl ToSocketAddrs, path: &str, rpc_options: RpcOptions) -> OxtuHandle {
    let rpc_middleware = RpcServiceBuilder::new().layer_fn(LoggingMiddleware);
    let server = Server::builder()
        .set_rpc_middleware(rpc_middleware)
        .build(addrs)
        .await
        .expect("server must be created");

    let addr = server
        .local_addr()
        .expect("server must have a local address");

    let index = Index::open(path, rpc_options);

    let (stop_tx, mut stop_rx) = watch::channel(());

    let index_handle = index.start();
    let server_handle = server.start(OxtuRpcServer { index }.into_rpc());

    tokio::spawn(async move {
        stop_rx.changed().await.unwrap();
        index_handle.stop();
        server_handle.stop().unwrap();
        index_handle.stopped().await;
        server_handle.stopped().await;
    });

    OxtuHandle {
        addr,
        stop_handle: Arc::new(stop_tx),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .init();

    let port = env::var("OXTU_PORT").unwrap_or_else(|_| "0".to_string());
    let listen = env::var("OXTU_LISTEN").unwrap_or_else(|_| "127.0.0.1".to_string());
    let addrs = format!("{}:{}", listen, port);
    let path = env::var("DATABASE_PATH").unwrap_or_else(|_| "/oxtu/.oxtu".to_string());
    let rpc_options = RpcOptions {
        url: env::var("BITCOIND_RPC_URL").expect("BITCOIND_RPC_URL must be set"),
        username: env::var("BITCOIND_RPC_USERNAME").ok(),
        password: env::var("BITCOIND_RPC_PASSWORD").ok(),
    };

    let db_path = path + "/data";
    let handle = start_oxtu(addrs, &db_path, rpc_options).await;
    tracing::info!("JSON-RPC server is running on {}", handle.addr);

    let mut sigint = signal(SignalKind::interrupt()).unwrap();
    let mut sigterm = signal(SignalKind::terminate()).unwrap();

    tokio::select! {
       _ = sigint.recv() => tracing::info!("SIGINT: shutting down..."),
       _ = sigterm.recv() => tracing::info!("SIGTERM: shutting down...")
    }

    handle.stop();
    handle.stopped().await;
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bigdecimal::BigDecimal;
    use bitcoincore_rpc::bitcoin::address::NetworkChecked;
    use bitcoincore_rpc::bitcoin::{Address, Amount, BlockHash, Txid};
    use bitcoincore_rpc::RpcApi;
    use jsonrpsee::http_client::HttpClientBuilder;
    use tempfile::tempdir;
    use testcontainers::runners::AsyncRunner;
    use testcontainers::ContainerAsync;
    use tracing_test::traced_test;

    use oxtu_index::rpc::RpcOptions;
    use testcontainers_bitcoind::{Async, Bitcoind};

    use crate::service::{ListUnspentQueryOptions, RpcClient};

    use super::*;

    struct TestSetup {
        _bitcoind: ContainerAsync<Bitcoind>,
        _temp_dir: tempfile::TempDir,
        bitcoind_client: bitcoincore_rpc::Client,
        oxtu_handle: OxtuHandle,
    }

    impl TestSetup {
        fn get_new_address(&self) -> Address<NetworkChecked> {
            self.bitcoind_client
                .get_new_address(None, None)
                .unwrap()
                .assume_checked()
        }

        fn generate(&self, n: u64, address: &Address<NetworkChecked>) -> Vec<BlockHash> {
            self.bitcoind_client
                .generate_to_address(n, address)
                .unwrap()
        }

        fn invalidate_block(&self, hash: &BlockHash) {
            self.bitcoind_client.invalidate_block(hash).unwrap();
        }

        fn send_to_address(&self, address: &Address<NetworkChecked>, amount: Amount) -> Txid {
            self.bitcoind_client
                .send_to_address(address, amount, None, None, None, None, None, None)
                .unwrap()
        }

        fn rpc_client(&self) -> jsonrpsee::http_client::HttpClient {
            let url = format!("http://{}/", self.oxtu_handle.addr);
            HttpClientBuilder::default().build(url).unwrap()
        }

        async fn stop(&self) {
            self.oxtu_handle.stop();
            self.oxtu_handle.stopped().await;
        }
    }

    async fn setup() -> anyhow::Result<TestSetup> {
        let bitcoind = Bitcoind::default().start().await?;
        let temp_dir = tempdir().unwrap();
        let oxtu_handle = {
            let rpc_options = match bitcoind.rpc_auth() {
                Some(bitcoincore_rpc::Auth::UserPass(username, password)) => RpcOptions {
                    url: bitcoind.rpc_url().await?,
                    username: Some(username),
                    password: Some(password),
                },
                _ => RpcOptions {
                    url: bitcoind.rpc_url().await?,
                    username: None,
                    password: None,
                },
            };
            start_oxtu(
                "127.0.0.1:0",
                temp_dir.path().to_str().unwrap(),
                rpc_options,
            )
            .await
        };

        let bitcoind_client = bitcoind.client().await?;

        bitcoind_client
            .create_wallet("test", None, None, None, None)
            .unwrap();

        Ok(TestSetup {
            _bitcoind: bitcoind,
            bitcoind_client,
            _temp_dir: temp_dir,
            oxtu_handle,
        })
    }

    #[tokio::test]
    #[traced_test]
    async fn list_unspent() -> anyhow::Result<()> {
        let test = self::setup().await?;
        let address1 = test.get_new_address();
        let address2 = test.get_new_address();

        test.generate(120, &address1);
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let hashes = test.generate(50, &address2);
        test.invalidate_block(hashes.first().unwrap());
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        for _ in 0..100 {
            test.send_to_address(&address2, Amount::from_btc(321.12345678).unwrap());
            test.send_to_address(&address1, Amount::from_btc(123.87654321).unwrap());
            test.generate(2, &address2);
        }
        test.generate(1, &address1);

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let client = test.rpc_client();
        let result = client
            .listunspent(address1.to_string(), None)
            .await
            .unwrap();

        assert_eq!(result[0].address, address1.to_string());
        assert_eq!(result.last().unwrap().height, 321);

        test.stop().await;
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn list_unspent_range() -> anyhow::Result<()> {
        let test = self::setup().await?;
        let address = test.get_new_address();

        test.generate(201, &address);
        let txid = test.send_to_address(&address, Amount::from_btc(4999.99999999).unwrap());
        test.generate(1, &address);
        // Block 0 to 202 generated

        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

        let client = test.rpc_client();
        let result = client
            .listunspent(
                address.to_string(),
                Some(ListUnspentQueryOptions {
                    minconf: Some(1),
                    maxconf: Some(50),
                    count: Some(100),
                }),
            )
            .await
            .unwrap();

        assert_eq!(result.len(), 51);
        for utxo in result {
            assert_eq!(utxo.address, address.to_string());

            assert!(utxo.confirmations >= 1);
            assert!(utxo.confirmations <= 50);
            // Height: 152 to 202
            assert!(utxo.height >= 152);
            assert!(utxo.height <= 202);

            // Look for 4999.99999999
            if utxo.txid == txid.to_string() {
                assert_eq!(utxo.amount.to_string(), "4999.99999999");
            }
        }

        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn get_address_info() -> anyhow::Result<()> {
        let test = self::setup().await?;
        let address = test.get_new_address();

        test.generate(101, &address);
        test.send_to_address(&address, Amount::from_btc(0.12345678).unwrap());
        test.generate(1, &address);

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let client = test.rpc_client();
        let result = client.getaddressinfo(address.to_string()).await.unwrap();

        assert_eq!(result.address, address.to_string());
        assert_eq!(
            result.balance,
            BigDecimal::from_str("5050.12345819").unwrap()
        );
        assert_eq!(
            result.total_sent,
            BigDecimal::from_str("50.00000000").unwrap()
        );
        assert_eq!(
            result.total_received,
            BigDecimal::from_str("5100.12345819").unwrap()
        );
        assert_eq!(result.tx_count, 104u64);

        test.stop().await;
        Ok(())
    }
}

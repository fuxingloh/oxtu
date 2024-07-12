use std::borrow::Cow;
use std::collections::HashMap;

use testcontainers::core::{error, WaitFor};
use testcontainers::{Image, TestcontainersError};

pub const RPC_PORT: u16 = 8332;

pub const NAME: &str = "docker.io/kylemanna/bitcoind";
pub const TAG: &str = "latest";

/// Module to work with Bitcoind inside of tests.
///
/// Starts an instance of Bitcoind.
/// This module is based on the official [`Bitcoind docker image`].
///
/// [`Bitcoind docker image`]: https://hub.docker.com/kylemanna/bitcoind
#[derive(Debug)]
pub struct Bitcoind {
    cmd: Vec<String>,
    env_vars: HashMap<String, String>,
}

impl Bitcoind {
    /// Sets the RPCUSER & RPCPASSWORD for the Bitcoind instance.
    pub fn with_rpc_auth(mut self, user: &str, password: &str) -> Self {
        self.env_vars.insert("RPCUSER".to_owned(), user.to_owned());
        self.env_vars
            .insert("RPCPASSWORD".to_owned(), password.to_owned());
        self
    }
}

impl Default for Bitcoind {
    fn default() -> Self {
        let mut env_vars = HashMap::new();
        env_vars.insert("REGTEST".to_owned(), "1".to_owned());
        env_vars.insert("DISABLEWALLET".to_owned(), "0".to_owned());
        env_vars.insert("RPCUSER".to_owned(), "user".to_owned());
        env_vars.insert("RPCPASSWORD".to_owned(), "pass".to_owned());

        let cmd = vec![
            "btc_oneshot".to_owned(),
            "-fallbackfee=0.00000200".to_owned(),
            "-rpcbind=:8332".to_owned(),
            "-rpcallowip=0.0.0.0/0".to_owned(),
        ];

        Self { env_vars, cmd }
    }
}

impl Image for Bitcoind {
    fn name(&self) -> &str {
        NAME
    }

    fn tag(&self) -> &str {
        TAG
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("init message: Done loading")]
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        &self.env_vars
    }

    fn cmd(&self) -> impl IntoIterator<Item = impl Into<Cow<'_, str>>> {
        &self.cmd
    }
}

/// Implement the convenient RPC methods for Bitcoind using the SyncRunner.
pub trait Sync {
    fn rpc_auth(&self) -> Option<bitcoincore_rpc::Auth>;

    fn rpc_url(&self) -> error::Result<String>;

    fn client(&self) -> error::Result<bitcoincore_rpc::Client>;
}

impl Sync for testcontainers::Container<Bitcoind> {
    fn rpc_auth(&self) -> Option<bitcoincore_rpc::Auth> {
        let user = self.image().env_vars.get("RPCUSER");
        let pass = self.image().env_vars.get("RPCPASSWORD");

        match (user, pass) {
            (Some(user), Some(pass)) => {
                Some(bitcoincore_rpc::Auth::UserPass(user.clone(), pass.clone()))
            }
            _ => None,
        }
    }

    fn rpc_url(&self) -> error::Result<String> {
        let host = self.get_host()?;
        let port = self.get_host_port_ipv4(RPC_PORT)?;
        Ok(format!("http://{host}:{port}"))
    }

    fn client(&self) -> error::Result<bitcoincore_rpc::Client> {
        let url = self.rpc_url()?;
        let auth = self.rpc_auth().unwrap();
        match bitcoincore_rpc::Client::new(&url, auth) {
            Ok(client) => Ok(client),
            Err(e) => Err(TestcontainersError::Other(e.into())),
        }
    }
}

/// Implement the convenient RPC methods for Bitcoind using the AsyncRunner.
pub trait Async {
    fn rpc_auth(&self) -> Option<bitcoincore_rpc::Auth>;

    fn rpc_url(&self) -> impl std::future::Future<Output = error::Result<String>> + Send;

    fn client(
        &self,
    ) -> impl std::future::Future<Output = error::Result<bitcoincore_rpc::Client>> + Send;
}

impl Async for testcontainers::ContainerAsync<Bitcoind> {
    fn rpc_auth(&self) -> Option<bitcoincore_rpc::Auth> {
        let user = self.image().env_vars.get("RPCUSER");
        let pass = self.image().env_vars.get("RPCPASSWORD");

        match (user, pass) {
            (Some(user), Some(pass)) => {
                Some(bitcoincore_rpc::Auth::UserPass(user.clone(), pass.clone()))
            }
            _ => None,
        }
    }

    async fn rpc_url(&self) -> error::Result<String> {
        let host = self.get_host().await?;
        let port = self.get_host_port_ipv4(RPC_PORT).await?;
        Ok(format!("http://{host}:{port}"))
    }

    async fn client(&self) -> error::Result<bitcoincore_rpc::Client> {
        let url = self.rpc_url().await?;
        let auth = self.rpc_auth().unwrap();
        match bitcoincore_rpc::Client::new(&url, auth) {
            Ok(client) => Ok(client),
            Err(e) => Err(TestcontainersError::Other(e.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    mod sync_container {
        use bitcoincore_rpc::bitcoin::Amount;
        use bitcoincore_rpc::RpcApi;
        use testcontainers::runners::SyncRunner;
        use tracing_test::traced_test;

        use crate::*;

        #[test]
        #[traced_test]
        fn bitcoind_rpc_url() -> anyhow::Result<()> {
            let bitcoind = Bitcoind::default().start()?;

            let url = bitcoind.rpc_url()?;
            let host = bitcoind.get_host()?;
            let port = bitcoind.get_host_port_ipv4(RPC_PORT)?;
            assert_eq!(url, format!("http://{host}:{port}"));
            Ok(())
        }

        #[test]
        #[traced_test]
        fn bitcoind_set_basic_auth() -> anyhow::Result<()> {
            let bitcoind = Bitcoind::default().with_rpc_auth("123", "456").start()?;

            let user = bitcoind.rpc_auth().unwrap();
            assert_eq!(
                user,
                bitcoincore_rpc::Auth::UserPass("123".to_owned(), "456".to_owned())
            );
            Ok(())
        }

        #[test]
        #[traced_test]
        fn rpc_getblockchaininfo() -> anyhow::Result<()> {
            let bitcoind = Bitcoind::default().start()?;
            let client = bitcoind.client()?;

            let info = client.get_blockchain_info()?;
            assert_eq!(info.blocks, 0);
            Ok(())
        }

        #[test]
        #[traced_test]
        fn rpc_generate() -> anyhow::Result<()> {
            let bitcoind = Bitcoind::default().start()?;
            let client = bitcoind.client()?;

            client.create_wallet("test", None, None, None, None)?;
            let address = client.get_new_address(None, None)?.assume_checked();
            let hashes = client.generate_to_address(150, &address)?;
            assert_eq!(hashes.len(), 150);

            let address2 = client.get_new_address(None, None)?.assume_checked();
            let txid = client.send_to_address(
                &address2,
                Amount::from_btc(900.12345678)?,
                None,
                None,
                None,
                None,
                None,
                None,
            )?;
            let generated = client.generate_to_address(1, &address)?;

            let block = client.get_block_info(&generated[0])?;
            assert_eq!(block.hash, generated[0]);
            assert_eq!(block.tx[1], txid);

            let count = client.get_block_count()?;
            assert_eq!(count, 151);

            let hash = client.get_block_hash(151)?;
            assert_eq!(hash, generated[0]);
            Ok(())
        }
    }

    mod async_container {
        use testcontainers::runners::AsyncRunner;
        use tracing_test::traced_test;

        use crate::*;

        #[tokio::test]
        #[traced_test]
        async fn bitcoind_rpc_url() -> anyhow::Result<()> {
            let bitcoind = Bitcoind::default().start().await?;

            let url = bitcoind.rpc_url().await?;
            let host = bitcoind.get_host().await?;
            let port = bitcoind.get_host_port_ipv4(RPC_PORT).await?;
            assert_eq!(url, format!("http://{host}:{port}"));
            Ok(())
        }

        #[tokio::test]
        #[traced_test]
        async fn bitcoind_set_basic_auth() -> anyhow::Result<()> {
            let bitcoind = Bitcoind::default()
                .with_rpc_auth("abc", "def")
                .start()
                .await?;

            let user = bitcoind.rpc_auth().unwrap();
            assert_eq!(
                user,
                bitcoincore_rpc::Auth::UserPass("abc".to_owned(), "def".to_owned())
            );
            Ok(())
        }
    }
}

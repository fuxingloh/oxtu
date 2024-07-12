use base64::Engine;
use bigdecimal::BigDecimal;
use rand::prelude::random;
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

pub struct RpcOptions {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

pub struct RpcClient {
    client: Client,
    url: String,
}

/// Custom RPC client for interacting with Bitcoin Core as we aim to have wide compatibility with
/// different Bitcoin Core versions and Bitcoin-like implementations.
///
/// Data fields not needed for the index are omitted and won't be parsed.
/// Verbose mode=2 (instead of parsing raw hex)
/// is enabled so that we can accommodate to different Bitcoin Core versions where
/// fields may be added or removed.
impl RpcClient {
    pub fn new(options: RpcOptions) -> RpcClient {
        let client = Client::builder()
            .default_headers({
                let authorization = match (options.username, options.password) {
                    (Some(username), None) => {
                        let credentials = format!("{}:", username);
                        let header_value = format!(
                            "Basic {}",
                            base64::prelude::BASE64_STANDARD.encode(credentials)
                        );
                        Ok(Some(HeaderValue::from_str(&header_value).unwrap()))
                    }
                    (Some(username), Some(password)) => {
                        let credentials = format!("{}:{}", username, password);
                        let header_value = format!(
                            "Basic {}",
                            base64::prelude::BASE64_STANDARD.encode(credentials)
                        );
                        Ok(Some(HeaderValue::from_str(&header_value).unwrap()))
                    }
                    (None, Some(_)) => Err("Username is required"),
                    (None, None) => Ok(None),
                };

                let mut headers = HeaderMap::new();
                headers.insert("Content-Type", HeaderValue::from_static("application/json"));
                if let Some(value) = authorization.expect("Failed to create authorization header") {
                    headers.insert(AUTHORIZATION, value);
                }
                headers
            })
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            url: options.url,
        }
    }

    async fn request<T: DeserializeOwned>(&self, method: &str, params: &Value) -> Result<T, Error> {
        let id: u64 = random();

        let resp = self
            .client
            .post(&self.url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params
            }))
            .send()
            .await?
            .json::<RpcResponse<T>>()
            .await?;

        if let Some(error) = resp.error {
            return Err(Error::Rpc(error));
        }

        Ok(resp.result.unwrap())
    }

    pub async fn get_block(&self, hash: &str) -> Result<Box<Block>, Error> {
        let block: Box<Block> = self.request("getblock", &json!([hash, 2])).await?;
        Ok(block)
    }

    pub async fn get_blockhash(&self, height: &u64) -> Result<String, Error> {
        let hash: String = self.request("getblockhash", &json!([height])).await?;
        Ok(hash)
    }

    pub async fn get_block_count(&self) -> Result<u64, Error> {
        let count: u64 = self.request("getblockcount", &json!([])).await?;
        Ok(count)
    }
}

#[derive(Debug)]
pub enum Error {
    Reqwest(reqwest::Error),
    Rpc(RpcError),
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Error {
        Error::Reqwest(err)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcResponse<R> {
    pub result: Option<R>,
    pub error: Option<RpcError>,
    pub id: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Block {
    pub hash: String,
    pub previousblockhash: Option<String>,
    pub height: u64,
    pub tx: Vec<Tx>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Tx {
    pub txid: String,
    pub hash: String,
    pub version: u32,
    pub size: u32,
    pub vsize: u32,
    pub weight: u32,
    pub locktime: u32,
    pub vin: Vec<Vin>,
    pub vout: Vec<Vout>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Vin {
    pub txid: Option<String>,
    pub vout: Option<u32>,
    #[serde(rename = "scriptSig")]
    pub script_sig: Option<ScriptSig>,
    pub sequence: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Vout {
    pub n: u32,
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: ScriptPubKey,
    #[serde(with = "bigdecimal::serde::json_num")]
    pub value: BigDecimal,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ScriptSig {
    pub hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ScriptPubKey {
    pub hex: String,
}

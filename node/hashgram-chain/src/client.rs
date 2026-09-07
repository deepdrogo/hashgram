//! Queries and transactions over the REST gateway.
//!
//! The gateway (`hashgramd`'s `api` server, `:1317`) exposes every module
//! query as JSON and accepts protobuf transactions base64-encoded. Using it
//! rather than gRPC keeps the client to one HTTP dependency and works
//! through any proxy an operator puts in front of a node.
//!
//! # Signing
//!
//! `SIGN_MODE_DIRECT`: the body and auth info are encoded, wrapped in a
//! `SignDoc` with the chain id and account number, and signed with the
//! account key. The chain id in the sign doc is what stops a devnet
//! transaction being replayed on mainnet; it is taken from the client's
//! configured identity, never from what a node reports.

use std::time::Duration;

use cosmrs::tx::{self, Fee, SignDoc, SignerInfo};
use cosmrs::{Any, Coin};
use serde::Deserialize;

use crate::wallet::{Wallet, DENOM};

/// Why a call failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Transport.
    #[error("chain unreachable: {0}")]
    Http(#[from] reqwest::Error),
    /// The gateway answered with an error body.
    #[error("chain returned {status}: {message}")]
    Gateway {
        /// HTTP status.
        status: u16,
        /// Body.
        message: String,
    },
    /// The account does not exist yet (never funded).
    #[error("account {0} does not exist on chain; it has never received funds")]
    NoAccount(String),
    /// Transaction construction or signing.
    #[error("transaction: {0}")]
    Tx(String),
    /// The transaction was rejected.
    #[error("transaction rejected (code {code}): {log}")]
    Rejected {
        /// ABCI code.
        code: u32,
        /// Raw log.
        log: String,
    },
    /// Unexpected response shape.
    #[error("unexpected response: {0}")]
    Shape(String),
}

/// A broadcast result.
#[derive(Debug, Clone)]
pub struct TxResult {
    /// Hex transaction hash.
    pub txhash: String,
    /// ABCI code, 0 on success.
    pub code: u32,
    /// Height, once included (0 for sync broadcast).
    pub height: u64,
    /// Gas used.
    pub gas_used: u64,
    /// Raw log.
    pub raw_log: String,
}

/// The client.
#[derive(Clone)]
pub struct Client {
    base: String,
    chain_id: String,
    http: reqwest::Client,
    /// Fee per unit of gas, in uhash. Multiplied by the simulated gas.
    pub gas_price_uhash: f64,
    /// Multiplier applied to simulated gas.
    pub gas_adjustment: f64,
}

#[derive(Deserialize)]
struct BalanceResponse {
    balance: Option<CoinJson>,
}

#[derive(Deserialize)]
struct CoinJson {
    denom: String,
    amount: String,
}

#[derive(Deserialize)]
struct AccountResponse {
    account: serde_json::Value,
}

#[derive(Deserialize)]
struct SimulateResponse {
    gas_info: Option<GasInfo>,
}

#[derive(Deserialize)]
struct GasInfo {
    #[serde(default)]
    gas_used: String,
}

#[derive(Deserialize)]
struct BroadcastResponse {
    tx_response: Option<TxResponseJson>,
}

#[derive(Deserialize)]
struct TxResponseJson {
    #[serde(default)]
    txhash: String,
    #[serde(default)]
    code: u32,
    #[serde(default)]
    height: String,
    #[serde(default)]
    gas_used: String,
    #[serde(default)]
    raw_log: String,
}

#[derive(Deserialize)]
struct GetTxResponse {
    tx_response: Option<TxResponseJson>,
}

#[derive(Deserialize)]
struct StatusResponse {
    #[serde(default)]
    default_node_info: NodeInfo,
}

#[derive(Deserialize, Default)]
struct NodeInfo {
    #[serde(default)]
    network: String,
}

#[derive(Deserialize)]
struct LatestBlock {
    block: Option<BlockJson>,
}
#[derive(Deserialize)]
struct BlockJson {
    header: HeaderJson,
}
#[derive(Deserialize)]
struct HeaderJson {
    height: String,
}

impl Client {
    /// A client for `base` (e.g. `http://127.0.0.1:1317`) signing for
    /// `chain_id`.
    pub fn new(base: &str, chain_id: &str) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            chain_id: chain_id.to_owned(),
            http,
            gas_price_uhash: 0.0025,
            gas_adjustment: 1.5,
        })
    }

    /// The configured chain id.
    #[must_use]
    pub fn chain_id(&self) -> &str {
        &self.chain_id
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let url = format!("{}/{}", self.base, path.trim_start_matches('/'));
        let resp = self.http.get(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(ClientError::Gateway {
                status: status.as_u16(),
                message,
            });
        }
        Ok(resp.json().await?)
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, ClientError> {
        let url = format!("{}/{}", self.base, path.trim_start_matches('/'));
        let resp = self.http.post(&url).json(body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(ClientError::Gateway {
                status: status.as_u16(),
                message,
            });
        }
        Ok(resp.json().await?)
    }

    /// Any GET returning JSON, for module queries the typed methods do not
    /// cover. The path is relative to the gateway root.
    pub async fn query(&self, path: &str) -> Result<serde_json::Value, ClientError> {
        self.get(path).await
    }

    /// The chain id the node reports. Compare with [`Self::chain_id`].
    pub async fn node_chain_id(&self) -> Result<String, ClientError> {
        let s: StatusResponse = self.get("cosmos/base/tendermint/v1beta1/node_info").await?;
        Ok(s.default_node_info.network)
    }

    /// Latest block height.
    pub async fn height(&self) -> Result<u64, ClientError> {
        let b: LatestBlock = self
            .get("cosmos/base/tendermint/v1beta1/blocks/latest")
            .await?;
        b.block
            .map(|b| b.header.height.parse().unwrap_or(0))
            .ok_or_else(|| ClientError::Shape("no block".into()))
    }

    /// Balance in uhash.
    pub async fn balance(&self, address: &str) -> Result<u128, ClientError> {
        let r: BalanceResponse = self
            .get(&format!(
                "cosmos/bank/v1beta1/balances/{address}/by_denom?denom={DENOM}"
            ))
            .await?;
        Ok(r.balance
            .filter(|c| c.denom == DENOM)
            .and_then(|c| c.amount.parse().ok())
            .unwrap_or(0))
    }

    /// Account number and sequence.
    pub async fn account(&self, address: &str) -> Result<(u64, u64), ClientError> {
        let r: AccountResponse = match self
            .get(&format!("cosmos/auth/v1beta1/accounts/{address}"))
            .await
        {
            Ok(r) => r,
            Err(ClientError::Gateway { status: 404, .. }) => {
                return Err(ClientError::NoAccount(address.to_owned()))
            }
            Err(e) => return Err(e),
        };
        // Base accounts and vesting accounts nest the fields differently.
        let acct = &r.account;
        let base = acct
            .get("base_vesting_account")
            .and_then(|v| v.get("base_account"))
            .or_else(|| acct.get("base_account"))
            .unwrap_or(acct);
        let num = base
            .get("account_number")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| ClientError::Shape("account_number missing".into()))?;
        let seq = base
            .get("sequence")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Ok((num, seq))
    }

    /// Looks up a transaction by hash.
    pub async fn tx(&self, hash: &str) -> Result<Option<TxResult>, ClientError> {
        match self
            .get::<GetTxResponse>(&format!("cosmos/tx/v1beta1/txs/{hash}"))
            .await
        {
            Ok(r) => Ok(r.tx_response.map(to_result)),
            Err(ClientError::Gateway { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Builds, simulates, signs and broadcasts a transaction, then waits
    /// for inclusion (up to ~30 s).
    pub async fn sign_and_broadcast(
        &self,
        wallet: &Wallet,
        msgs: Vec<Any>,
        memo: &str,
    ) -> Result<TxResult, ClientError> {
        let address = wallet.address().to_string();
        let (account_number, sequence) = self.account(&address).await?;
        let chain_id: cosmrs::tendermint::chain::Id = self
            .chain_id
            .parse()
            .map_err(|e| ClientError::Tx(format!("chain id: {e}")))?;
        let body = tx::Body::new(msgs, memo, 0u32);

        // Simulate with an empty signature to learn gas.
        let sim_fee = Fee::from_amount_and_gas(coin(0)?, 0u64);
        let sim_auth =
            SignerInfo::single_direct(Some(wallet.public_key()), sequence).auth_info(sim_fee);
        let sim_raw = tx::Raw::from(cosmrs::proto::cosmos::tx::v1beta1::TxRaw {
            body_bytes: body
                .clone()
                .into_bytes()
                .map_err(|e| ClientError::Tx(e.to_string()))?,
            auth_info_bytes: sim_auth
                .into_bytes()
                .map_err(|e| ClientError::Tx(e.to_string()))?,
            signatures: vec![vec![0u8; 64]],
        });
        let sim_bytes = sim_raw
            .to_bytes()
            .map_err(|e| ClientError::Tx(e.to_string()))?;
        let sim: SimulateResponse = self
            .post(
                "cosmos/tx/v1beta1/simulate",
                &serde_json::json!({ "tx_bytes": b64(&sim_bytes) }),
            )
            .await?;
        let gas_used: u64 = sim
            .gas_info
            .and_then(|g| g.gas_used.parse().ok())
            .unwrap_or(200_000);
        let gas_limit = ((gas_used as f64) * self.gas_adjustment).ceil() as u64 + 20_000;
        let fee_amount = ((gas_limit as f64) * self.gas_price_uhash).ceil() as u128;

        let fee = Fee::from_amount_and_gas(coin(fee_amount)?, gas_limit);
        let auth_info =
            SignerInfo::single_direct(Some(wallet.public_key()), sequence).auth_info(fee);
        let sign_doc = SignDoc::new(&body, &auth_info, &chain_id, account_number)
            .map_err(|e| ClientError::Tx(e.to_string()))?;
        let raw = sign_doc
            .sign(wallet.signing_key())
            .map_err(|e| ClientError::Tx(e.to_string()))?;
        let bytes = raw.to_bytes().map_err(|e| ClientError::Tx(e.to_string()))?;

        let resp: BroadcastResponse = self
            .post(
                "cosmos/tx/v1beta1/txs",
                &serde_json::json!({ "tx_bytes": b64(&bytes), "mode": "BROADCAST_MODE_SYNC" }),
            )
            .await?;
        let r = resp
            .tx_response
            .ok_or_else(|| ClientError::Shape("no tx_response".into()))?;
        if r.code != 0 {
            return Err(ClientError::Rejected {
                code: r.code,
                log: r.raw_log,
            });
        }
        // Wait for inclusion.
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Some(t) = self.tx(&r.txhash).await? {
                if t.code != 0 {
                    return Err(ClientError::Rejected {
                        code: t.code,
                        log: t.raw_log,
                    });
                }
                return Ok(t);
            }
        }
        Ok(to_result(r))
    }
}

fn to_result(r: TxResponseJson) -> TxResult {
    TxResult {
        txhash: r.txhash,
        code: r.code,
        height: r.height.parse().unwrap_or(0),
        gas_used: r.gas_used.parse().unwrap_or(0),
        raw_log: r.raw_log,
    }
}

fn coin(amount: u128) -> Result<Coin, ClientError> {
    Coin::new(amount, DENOM).map_err(|e| ClientError::Tx(e.to_string()))
}

/// Standard base64 with padding.
fn b64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(
                    T.get(((n >> (18 - 6 * i)) & 63) as usize)
                        .copied()
                        .unwrap_or(b'A') as char,
                );
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_sign_doc_binds_the_chain_id() {
        // Two sign docs over the same body for different chains must differ,
        // which is what stops a devnet transaction replaying on mainnet.
        let w = Wallet::from_mnemonic(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            "",
            0,
        )
        .unwrap();
        let body = tx::Body::new(
            vec![crate::msgs::bank_send(w.address().as_ref(), "hash1x", 1)],
            "",
            0u32,
        );
        let fee = Fee::from_amount_and_gas(coin(1).unwrap(), 100_000u64);
        let auth = SignerInfo::single_direct(Some(w.public_key()), 0).auth_info(fee);
        let a = SignDoc::new(&body, &auth, &"hashgram-1".parse().unwrap(), 7).unwrap();
        let b = SignDoc::new(&body, &auth, &"hashgram-devnet-1".parse().unwrap(), 7).unwrap();
        let ra = a.sign(w.signing_key()).unwrap().to_bytes().unwrap();
        let rb = b.sign(w.signing_key()).unwrap().to_bytes().unwrap();
        assert_ne!(ra, rb);
    }
}

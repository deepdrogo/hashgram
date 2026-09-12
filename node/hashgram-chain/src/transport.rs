//! How a [`crate::Client`] reaches the chain.
//!
//! Two transports exist. The original one is HTTP to a REST gateway
//! (`http://127.0.0.1:1317` on a node, or an HTTPS endpoint an operator
//! publishes). The second is the P2P chain relay: allow-listed reads and
//! broadcast carried over `/hashgram/rpc/1` by the same nodes a client
//! already talks to, implemented in `hashgram-sdk` on top of its `Link` and
//! plugged in here through [`ChainTransport`]. The client code above the
//! transport — balances, sequences, signing, broadcast, polling — is the
//! same either way.
//!
//! A transport reports what it can and cannot do. The relay cannot run
//! `simulate` (it executes transactions against the gateway, so nodes do not
//! forward it); the client then estimates gas instead. The relay also knows
//! which peers served an answer and whether they agreed; HTTP knows nothing
//! of the kind and reports `None`.

use std::sync::Arc;
use std::time::Duration;

use crate::client::ClientError;

/// An answer from the chain, verbatim: HTTP status, body bytes and the
/// block height the gateway reported (0 when unknown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportResponse {
    /// HTTP status.
    pub status: u16,
    /// Body as served.
    pub body: Vec<u8>,
    /// `grpc-metadata-x-cosmos-block-height`, 0 when absent.
    pub height: u64,
}

impl TransportResponse {
    /// `true` for 2xx.
    #[must_use]
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Who answered a read and whether they agreed. Produced by the P2P relay;
/// what the UI turns into "verified by 2 nodes".
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Verification {
    /// Peer ids that served the answer that was returned.
    pub peers: Vec<String>,
    /// Their operator addresses (empty string when a peer claimed none).
    pub operators: Vec<String>,
    /// Heights each answer was served at.
    pub heights: Vec<u64>,
    /// `true` when at least two answers from different operators matched.
    pub agreed: bool,
    /// `true` when only one operator's nodes were reachable, so agreement
    /// could not be independent.
    pub single_operator: bool,
    /// Peers whose answer disagreed and were set aside.
    pub disputed: Vec<String>,
}

impl Verification {
    /// Number of nodes whose answers matched the returned one.
    #[must_use]
    pub fn verified_by(&self) -> usize {
        self.peers.len()
    }
}

/// A way to reach the chain.
#[async_trait::async_trait]
pub trait ChainTransport: Send + Sync {
    /// GET `path` (no leading slash, no query string) with `query` (no `?`).
    async fn get(&self, path: &str, query: &str) -> Result<TransportResponse, ClientError>;

    /// Broadcast a signed transaction in sync mode
    /// (`POST /cosmos/tx/v1beta1/txs`).
    async fn broadcast(&self, tx_bytes: &[u8]) -> Result<TransportResponse, ClientError>;

    /// `POST /cosmos/tx/v1beta1/simulate`, when the transport allows it.
    /// The default says it does not.
    async fn simulate(&self, _tx_bytes: &[u8]) -> Result<TransportResponse, ClientError> {
        Err(ClientError::Unsupported(
            "this transport does not simulate transactions".into(),
        ))
    }

    /// Whether [`Self::simulate`] works.
    fn can_simulate(&self) -> bool {
        false
    }

    /// Who served the most recent read and whether they agreed; `None`
    /// when the transport has no such notion (HTTP).
    fn verification(&self) -> Option<Verification> {
        None
    }

    /// A short human description ("http://127.0.0.1:1317", "p2p relay").
    fn describe(&self) -> String;
}

/// HTTP to a REST gateway.
pub struct HttpTransport {
    base: String,
    http: reqwest::Client,
}

impl HttpTransport {
    /// Builds a transport for `base` (trailing slashes ignored).
    pub fn new(base: &str) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            http,
        })
    }

    async fn read(resp: reqwest::Response) -> Result<TransportResponse, ClientError> {
        let status = resp.status().as_u16();
        let height = resp
            .headers()
            .get("grpc-metadata-x-cosmos-block-height")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let body = resp.bytes().await?.to_vec();
        Ok(TransportResponse {
            status,
            body,
            height,
        })
    }

    async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<TransportResponse, ClientError> {
        let url = format!("{}/{}", self.base, path);
        let resp = self.http.post(&url).json(body).send().await?;
        Self::read(resp).await
    }
}

#[async_trait::async_trait]
impl ChainTransport for HttpTransport {
    async fn get(&self, path: &str, query: &str) -> Result<TransportResponse, ClientError> {
        let mut url = format!("{}/{}", self.base, path);
        if !query.is_empty() {
            url.push('?');
            url.push_str(query);
        }
        let resp = self.http.get(&url).send().await?;
        Self::read(resp).await
    }

    async fn broadcast(&self, tx_bytes: &[u8]) -> Result<TransportResponse, ClientError> {
        self.post_json(
            "cosmos/tx/v1beta1/txs",
            &serde_json::json!({ "tx_bytes": crate::client::b64(tx_bytes), "mode": "BROADCAST_MODE_SYNC" }),
        )
        .await
    }

    async fn simulate(&self, tx_bytes: &[u8]) -> Result<TransportResponse, ClientError> {
        self.post_json(
            "cosmos/tx/v1beta1/simulate",
            &serde_json::json!({ "tx_bytes": crate::client::b64(tx_bytes) }),
        )
        .await
    }

    fn can_simulate(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        self.base.clone()
    }
}

/// A shared transport handle.
pub type SharedTransport = Arc<dyn ChainTransport>;

/// Splits `path?query` into its two halves; a path without `?` has an
/// empty query.
#[must_use]
pub fn split_path(path: &str) -> (&str, &str) {
    let p = path.trim_start_matches('/');
    match p.split_once('?') {
        Some((a, b)) => (a, b),
        None => (p, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_path_separates_query() {
        assert_eq!(
            split_path("/cosmos/bank/v1beta1/supply/by_denom?denom=uhash"),
            ("cosmos/bank/v1beta1/supply/by_denom", "denom=uhash")
        );
        assert_eq!(
            split_path("hashgram/founder/v1/params"),
            ("hashgram/founder/v1/params", "")
        );
        assert_eq!(split_path("a?b=1?c"), ("a", "b=1?c"));
    }

    #[test]
    fn verification_counts_matching_peers() {
        let v = Verification {
            peers: vec!["a".into(), "b".into()],
            agreed: true,
            ..Default::default()
        };
        assert_eq!(v.verified_by(), 2);
        assert!(!v.single_operator);
    }
}

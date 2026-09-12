//! Network: what the client can see about the network it is on, and the
//! public read model (leaderboards, validators, stats) served by an
//! indexer when one is configured.
//!
//! The indexer is a convenience, never an authority: everything it says is
//! also derivable from the chain, and the client labels indexer-sourced
//! numbers as such. Leaderboards expose public addresses; a username is
//! attached only when that address registered it on chain (public by
//! construction), which the indexer reports as `verified: true`.

use serde_json::Value;

use crate::app::HashgramOne;
use crate::SdkError;

/// A connected peer for display.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PeerView {
    /// Peer id.
    pub peer: String,
    /// Roles claimed.
    pub roles: Vec<String>,
    /// Operator address claimed.
    pub operator: String,
}

/// Network overview.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Overview {
    /// Network id.
    pub network_id: String,
    /// Chain id.
    pub chain_id: String,
    /// Genesis hash.
    pub genesis_hash: String,
    /// Connected verified peers.
    pub peers: Vec<PeerView>,
    /// Peers rejected (wrong network etc.).
    pub rejected: Vec<(String, String)>,
    /// Latest chain height seen.
    pub height: Option<u64>,
    /// Chain read verification summary.
    pub verification: Option<String>,
}

/// The Network API.
pub struct Network<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> Network<'a> {
    /// Overview.
    pub async fn overview(&mut self) -> Result<Overview, SdkError> {
        let peers = self
            .one
            .link
            .peers()
            .await
            .into_iter()
            .map(|p| PeerView {
                peer: p.peer.to_string(),
                roles: p.roles,
                operator: p.operator,
            })
            .collect();
        let rejected = self
            .one
            .link
            .rejected()
            .await
            .into_iter()
            .map(|r| (r.peer.to_string(), r.reason))
            .collect();
        let height = self.one.chain.height().await.ok();
        let verification = self.one.chain.verification().map(|v| {
            if v.single_operator {
                "single operator".to_owned()
            } else if v.agreed {
                format!(
                    "{} nodes / {} operators agree",
                    v.peers.len(),
                    v.operators.len()
                )
            } else {
                "unverified".to_owned()
            }
        });
        Ok(Overview {
            network_id: self.one.network.network_id.clone(),
            chain_id: self.one.network.chain_id.clone(),
            genesis_hash: self.one.network.genesis_hash.clone(),
            peers,
            rejected,
            height,
            verification,
        })
    }

    /// Validators from the chain (staking module), sorted by tokens.
    pub async fn validators(&mut self) -> Result<Vec<Value>, SdkError> {
        let v = self
            .one
            .chain
            .query("cosmos/staking/v1beta1/validators?pagination.limit=200")
            .await?;
        let mut vals: Vec<Value> = v
            .get("validators")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        vals.sort_by_key(|x| {
            std::cmp::Reverse(
                x.get("tokens")
                    .and_then(|t| t.as_str())
                    .and_then(|t| t.parse::<u128>().ok())
                    .unwrap_or(0),
            )
        });
        Ok(vals)
    }

    /// Supply and reserve figures from the chain.
    pub async fn supply(&mut self) -> Result<Value, SdkError> {
        let supply = self
            .one
            .chain
            .query("cosmos/bank/v1beta1/supply/by_denom?denom=uhash")
            .await?;
        let reserve = self
            .one
            .chain
            .query("hashgram/serviceproof/v1/reserve")
            .await
            .unwrap_or(Value::Null);
        let founder = self
            .one
            .chain
            .query("hashgram/founder/v1/revenue")
            .await
            .unwrap_or(Value::Null);
        Ok(serde_json::json!({
            "supply": supply,
            "service_reserve": reserve,
            "founder_revenue": founder,
        }))
    }

    /// Reads a public indexer endpoint (`/v1/...`). Returns `None` when no
    /// indexer URL is configured. The desktop shows these as "from indexer".
    pub async fn indexer(
        &mut self,
        base_url: Option<&str>,
        path: &str,
    ) -> Result<Option<Value>, SdkError> {
        let Some(base) = base_url else {
            return Ok(None);
        };
        if !path.starts_with("/v1/") || path.contains("..") {
            return Err(SdkError::Invalid("indexer path".into()));
        }
        let url = format!("{}{}", base.trim_end_matches('/'), path);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| SdkError::Invalid(e.to_string()))?;
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| SdkError::NotFound(format!("indexer: {e}")))?;
        if !resp.status().is_success() {
            return Err(SdkError::NotFound(format!(
                "indexer returned {}",
                resp.status()
            )));
        }
        let body = resp
            .bytes()
            .await
            .map_err(|e| SdkError::Corrupt(e.to_string()))?;
        if body.len() > 4 * 1024 * 1024 {
            return Err(SdkError::Corrupt("indexer answer too large".into()));
        }
        Ok(Some(
            serde_json::from_slice(&body).map_err(|e| SdkError::Corrupt(e.to_string()))?,
        ))
    }

    /// Top holders (indexer). See `docs/INDEXER.md`.
    pub async fn top_holders(
        &mut self,
        indexer: Option<&str>,
        limit: u32,
    ) -> Result<Option<Value>, SdkError> {
        self.indexer(indexer, &format!("/v1/leaderboards/holders?limit={limit}"))
            .await
    }
    /// Top validators (indexer).
    pub async fn top_validators(
        &mut self,
        indexer: Option<&str>,
        limit: u32,
    ) -> Result<Option<Value>, SdkError> {
        self.indexer(
            indexer,
            &format!("/v1/leaderboards/validators?limit={limit}"),
        )
        .await
    }
    /// Top providers (indexer).
    pub async fn top_providers(
        &mut self,
        indexer: Option<&str>,
        limit: u32,
    ) -> Result<Option<Value>, SdkError> {
        self.indexer(
            indexer,
            &format!("/v1/leaderboards/providers?limit={limit}"),
        )
        .await
    }
    /// Network stats (indexer).
    pub async fn stats(&mut self, indexer: Option<&str>) -> Result<Option<Value>, SdkError> {
        self.indexer(indexer, "/v1/network/stats").await
    }
}

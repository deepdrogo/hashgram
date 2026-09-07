//! The node's read-only view of the chain.
//!
//! The P2P daemon never signs a chain transaction with anything but its own
//! service-submission key (see `serviceproof`), and never holds a consensus
//! or user key. What it needs from the chain is answers: is this device
//! authorised for this author, is this provider registered, what epoch is
//! it. Those come over the co-located `hashgramd`'s REST gateway on
//! localhost. If the chain node is down the answers are "unavailable", and
//! every caller treats that as "do nothing", never as "yes".

use std::time::Duration;

use serde::Deserialize;

use crate::social::{Authz, DeviceAuthority};

/// A REST client for the co-located chain node.
#[derive(Clone)]
pub struct ChainClient {
    base: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct ResolveDeviceKey {
    #[serde(default)]
    found: bool,
    #[serde(default)]
    root_address: String,
    #[serde(default)]
    device: Device,
}

#[derive(Deserialize, Default)]
struct Device {
    #[serde(default)]
    revoked: bool,
}

#[derive(Deserialize)]
struct NodeInfoResponse {
    #[serde(default)]
    default_node_info: DefaultNodeInfo,
}

#[derive(Deserialize, Default)]
struct DefaultNodeInfo {
    #[serde(default)]
    network: String,
}

impl ChainClient {
    /// Points at a REST base such as `http://127.0.0.1:1317`.
    pub fn new(base: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(2))
            .build()?;
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            http,
        })
    }

    /// The chain id the co-located node reports, for a startup cross-check
    /// against the pinned identity.
    pub async fn chain_id(&self) -> anyhow::Result<String> {
        let url = format!("{}/cosmos/base/tendermint/v1beta1/node_info", self.base);
        let r: NodeInfoResponse = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(r.default_node_info.network)
    }

    /// Generic GET returning JSON, for the local API's pass-through queries.
    pub async fn get_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}/{}", self.base, path.trim_start_matches('/'));
        Ok(self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

/// Standard base64 for the gateway's bytes query parameter.
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

#[async_trait::async_trait]
impl DeviceAuthority for ChainClient {
    async fn check(&self, author: &str, device_pubkey: &[u8]) -> Authz {
        let url = format!(
            "{}/hashgram/identity/v1/resolve_device_key?device_pubkey={}",
            self.base,
            urlencode(&b64(device_pubkey))
        );
        let resp = match self.http.get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(error = %e, "chain unavailable for device lookup");
                return Authz::Unavailable;
            }
        };
        if resp.status().is_server_error() {
            return Authz::Unavailable;
        }
        if !resp.status().is_success() {
            // 404 / 400 from the gateway: the key is not known.
            return Authz::Refused;
        }
        match resp.json::<ResolveDeviceKey>().await {
            Ok(r) if r.found && !r.device.revoked && r.root_address == author => Authz::Authorised,
            Ok(_) => Authz::Refused,
            Err(_) => Authz::Unavailable,
        }
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_urlencode_produce_gateway_safe_strings() {
        let raw = [0xfbu8, 0xff, 0xbf, 0x00];
        let enc = b64(&raw);
        assert_eq!(enc, "+/+/AA==");
        assert_eq!(urlencode(&enc), "%2B%2F%2B%2FAA%3D%3D");
    }
}

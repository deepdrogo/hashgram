//! Public social events: build, sign, publish, fetch.
//!
//! Events are signed with the device key and carry a per-author sequence and
//! hash chain. The sequence lives in the vault so two sessions on one device
//! do not collide; two *devices* of one account each keep their own chain
//! (the author's sequence space is per device in practice, and indexers
//! order by timestamp).

use hashgram_net::NetworkIdentity;
use hashgram_proto::limits::WIRE_VERSION;
use hashgram_proto::pb;
use hashgram_proto::{signing, validate, Ed25519Signer};
use prost::Message;

use crate::account::Account;
use crate::link::Link;
use crate::SdkError;

/// Vault key for the last sequence and event id.
pub const VAULT_SOCIAL_KEY: &str = "social_chain";

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Chain {
    next_sequence: u64,
    previous: String,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Social publishing for a device.
pub struct Social {
    device: Ed25519Signer,
    address: String,
    chain: Chain,
}

impl Social {
    /// Loads chain state from the vault.
    pub fn open(account: &Account) -> Result<Self, SdkError> {
        let chain = account
            .contents
            .extra
            .get(VAULT_SOCIAL_KEY)
            .and_then(|h| hex::decode(h).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Ok(Self {
            device: account.device()?,
            address: account.address().to_owned(),
            chain,
        })
    }

    /// Writes chain state back.
    pub fn persist(&self, account: &mut Account) {
        let raw = serde_json::to_vec(&self.chain).unwrap_or_default();
        account
            .contents
            .extra
            .insert(VAULT_SOCIAL_KEY.into(), hex::encode(raw));
    }

    /// Builds and signs an event of `kind` with a typed payload.
    pub fn build<M: Message>(
        &mut self,
        network: &NetworkIdentity,
        kind: &str,
        payload: &M,
        media: Vec<pb::MediaReference>,
    ) -> Result<pb::SocialEvent, SdkError> {
        let mut ev = pb::SocialEvent {
            network_id: network.network_id.clone(),
            version: WIRE_VERSION,
            r#type: kind.to_owned(),
            author: self.address.clone(),
            timestamp: now(),
            sequence: self.chain.next_sequence,
            previous_event: if self.chain.next_sequence == 0 {
                Vec::new()
            } else {
                hex::decode(&self.chain.previous).unwrap_or_default()
            },
            payload: payload.encode_to_vec(),
            media,
            ..Default::default()
        };
        signing::sign_social_event(network, &self.device, &mut ev)?;
        validate::social_event(&ev, now()).map_err(|e| SdkError::Invalid(e.to_string()))?;
        Ok(ev)
    }

    /// Publishes an event through a connected node and advances the chain.
    pub async fn publish(&mut self, link: &Link, ev: pb::SocialEvent) -> Result<Vec<u8>, SdkError> {
        let id = ev.id.clone();
        let (_, body) = link
            .request_role_any(pb::request::Body::EventPublish(pb::EventPublish {
                event: Some(ev),
            }))
            .await?;
        match body {
            pb::response::Body::EventPublish(r) if r.accepted => {
                self.chain.next_sequence += 1;
                self.chain.previous = hex::encode(&id);
                Ok(id)
            }
            pb::response::Body::EventPublish(r) => Err(SdkError::Invalid(r.reason)),
            _ => Err(SdkError::Link(crate::link::LinkError::Unexpected)),
        }
    }

    /// Fetches an author's events from a connected node, verifying each.
    pub async fn fetch_author(
        &self,
        link: &Link,
        network: &NetworkIdentity,
        author: &str,
        from_sequence: u64,
        limit: u32,
    ) -> Result<Vec<pb::SocialEvent>, SdkError> {
        let (_, body) = link
            .request_role_any(pb::request::Body::EventFetch(pb::EventFetch {
                author: author.to_owned(),
                from_sequence,
                limit,
                ..Default::default()
            }))
            .await?;
        match body {
            pb::response::Body::EventFetch(r) => Ok(r
                .events
                .into_iter()
                .filter(|e| signing::verify_social_event(network, e).is_ok())
                .collect()),
            _ => Err(SdkError::Link(crate::link::LinkError::Unexpected)),
        }
    }

    /// Fetches events by id.
    pub async fn fetch_ids(
        &self,
        link: &Link,
        network: &NetworkIdentity,
        ids: Vec<Vec<u8>>,
    ) -> Result<Vec<pb::SocialEvent>, SdkError> {
        let (_, body) = link
            .request_role_any(pb::request::Body::EventFetch(pb::EventFetch {
                ids,
                ..Default::default()
            }))
            .await?;
        match body {
            pb::response::Body::EventFetch(r) => Ok(r
                .events
                .into_iter()
                .filter(|e| signing::verify_social_event(network, e).is_ok())
                .collect()),
            _ => Err(SdkError::Link(crate::link::LinkError::Unexpected)),
        }
    }
}

/// Renders an event's typed payload as JSON for display.
#[must_use]
pub fn payload_json(ev: &pb::SocialEvent) -> serde_json::Value {
    fn j<M: Message + Default + serde::Serialize>(b: &[u8]) -> serde_json::Value {
        M::decode(b)
            .ok()
            .and_then(|m| serde_json::to_value(m).ok())
            .unwrap_or(serde_json::Value::Null)
    }
    match ev.r#type.as_str() {
        "PROFILE_UPDATE" => j::<pb::ProfileUpdate>(&ev.payload),
        "FOLLOW" => j::<pb::Follow>(&ev.payload),
        "UNFOLLOW" => j::<pb::Unfollow>(&ev.payload),
        "POST_CREATE" => j::<pb::PostCreate>(&ev.payload),
        "POST_EDIT" => j::<pb::PostEdit>(&ev.payload),
        "POST_DELETE" => j::<pb::PostDelete>(&ev.payload),
        "COMMENT_CREATE" => j::<pb::CommentCreate>(&ev.payload),
        "REACTION" => j::<pb::Reaction>(&ev.payload),
        "REPOST" => j::<pb::Repost>(&ev.payload),
        "CHANNEL_CREATE" => j::<pb::ChannelCreate>(&ev.payload),
        "REEL_CREATE" => j::<pb::ReelCreate>(&ev.payload),
        "STORY_CREATE" => j::<pb::StoryCreate>(&ev.payload),
        _ => serde_json::Value::Null,
    }
}

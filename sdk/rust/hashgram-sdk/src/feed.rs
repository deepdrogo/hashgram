//! Feed: the public social layer with a typed API and honest pagination.
//!
//! Everything public is a signed `SocialEvent` (see `crate::social`). This
//! module adds: post/comment/react/repost/follow helpers, a chronological
//! *following* feed assembled client-side from the authors the user
//! follows (each author's chain is fetched from store nodes and verified),
//! a local cache of fetched events in the store, and the merge of private
//! Circle posts into one timeline for the UI.
//!
//! There is no ranking algorithm. Friends and Following are chronological;
//! Explore is whatever public authors the client chooses to fetch (the
//! indexer's `/v1/feed/chronological` and `/v1/feed/hashtag/{tag}` serve
//! that when an indexer is configured — see `network::Network::indexer`).

use std::collections::{BTreeMap, BTreeSet};

use hashgram_proto::pb;
use prost::Message;

use crate::app::HashgramOne;
use crate::social::{payload_json, Social};
use crate::SdkError;

const NS_EVENTS: &str = "feed/events";
const NS_FOLLOWS: &str = "feed/follows";

/// A feed item for display.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FeedItem {
    /// Hex event id.
    pub id: String,
    /// Event type.
    pub kind: String,
    /// Author address.
    pub author: String,
    /// Timestamp (s).
    pub timestamp: u64,
    /// Decoded payload.
    pub payload: serde_json::Value,
    /// Media references (cid hex, mime, size).
    pub media: Vec<(String, String, u64)>,
    /// "public" or "circle:<hex gid>".
    pub visibility: String,
}

fn item(ev: &pb::SocialEvent) -> FeedItem {
    FeedItem {
        id: hex::encode(&ev.id),
        kind: ev.r#type.clone(),
        author: ev.author.clone(),
        timestamp: ev.timestamp,
        payload: payload_json(ev),
        media: ev
            .media
            .iter()
            .map(|m| (hex::encode(&m.cid), m.mime.clone(), m.size))
            .collect(),
        visibility: "public".into(),
    }
}

/// A post with its comments and reaction tallies.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PostThread {
    /// The post.
    pub post: FeedItem,
    /// Comments oldest first.
    pub comments: Vec<FeedItem>,
    /// reaction → count.
    pub reactions: BTreeMap<String, u32>,
}

/// The Feed API.
pub struct Feed<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> Feed<'a> {
    fn social(&self) -> Result<Social, SdkError> {
        Social::open(&self.one.account)
    }

    async fn publish(
        &mut self,
        kind: &str,
        payload: &impl Message,
        media: Vec<pb::MediaReference>,
    ) -> Result<String, SdkError> {
        let mut s = self.social()?;
        let ev = s.build(&self.one.network, kind, payload, media)?;
        let id = s.publish(&self.one.link, ev.clone()).await?;
        s.persist(&mut self.one.account);
        self.one.store.put(NS_EVENTS, &ev.id, &ev)?;
        Ok(hex::encode(id))
    }

    /// Creates a public post. `media` are already-uploaded public blobs.
    pub async fn post(
        &mut self,
        text: &str,
        hashtags: Vec<String>,
        media: Vec<pb::MediaReference>,
        sensitive: bool,
    ) -> Result<String, SdkError> {
        self.publish(
            "POST_CREATE",
            &pb::PostCreate {
                text: text.to_owned(),
                hashtags,
                sensitive,
                ..Default::default()
            },
            media,
        )
        .await
    }

    /// Comments on a post.
    pub async fn comment(&mut self, post_id_hex: &str, text: &str) -> Result<String, SdkError> {
        let post = hex::decode(post_id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        self.publish(
            "COMMENT_CREATE",
            &pb::CommentCreate {
                post,
                text: text.to_owned(),
                ..Default::default()
            },
            vec![],
        )
        .await
    }

    /// Reacts (empty reaction removes).
    pub async fn react(&mut self, target_hex: &str, reaction: &str) -> Result<String, SdkError> {
        let target = hex::decode(target_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        self.publish(
            "REACTION",
            &pb::Reaction {
                target,
                reaction: reaction.to_owned(),
            },
            vec![],
        )
        .await
    }

    /// Reposts.
    pub async fn repost(&mut self, post_id_hex: &str, comment: &str) -> Result<String, SdkError> {
        let post = hex::decode(post_id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        self.publish(
            "REPOST",
            &pb::Repost {
                post,
                comment: comment.to_owned(),
            },
            vec![],
        )
        .await
    }

    /// Edits our own post.
    pub async fn edit_post(&mut self, post_id_hex: &str, text: &str) -> Result<String, SdkError> {
        let post = hex::decode(post_id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        self.publish(
            "POST_EDIT",
            &pb::PostEdit {
                post,
                text: text.to_owned(),
                ..Default::default()
            },
            vec![],
        )
        .await
    }

    /// Deletes our own post (tombstone event).
    pub async fn delete_post(&mut self, post_id_hex: &str) -> Result<String, SdkError> {
        let post = hex::decode(post_id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        self.publish("POST_DELETE", &pb::PostDelete { post }, vec![])
            .await
    }

    /// Updates our public profile.
    pub async fn update_profile(
        &mut self,
        display_name: &str,
        bio: &str,
        avatar_cid_hex: &str,
    ) -> Result<String, SdkError> {
        let avatar_cid = hex::decode(avatar_cid_hex).unwrap_or_default();
        self.publish(
            "PROFILE_UPDATE",
            &pb::ProfileUpdate {
                display_name: display_name.to_owned(),
                bio: bio.to_owned(),
                avatar_cid,
                ..Default::default()
            },
            vec![],
        )
        .await
    }

    /// Follow / unfollow (public).
    pub async fn set_follow(&mut self, address: &str, on: bool) -> Result<String, SdkError> {
        let id = if on {
            self.publish(
                "FOLLOW",
                &pb::Follow {
                    target: address.to_owned(),
                },
                vec![],
            )
            .await?
        } else {
            self.publish(
                "UNFOLLOW",
                &pb::Unfollow {
                    target: address.to_owned(),
                },
                vec![],
            )
            .await?
        };
        let mut follows = self.follows();
        if on {
            follows.insert(address.to_owned());
        } else {
            follows.remove(address);
        }
        self.one.store.put(NS_FOLLOWS, b"set", &follows)?;
        Ok(id)
    }

    /// Addresses we follow (local mirror).
    pub fn follows(&self) -> BTreeSet<String> {
        self.one
            .store
            .get(NS_FOLLOWS, b"set")
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// Fetches and caches an author's recent events (verified).
    pub async fn refresh_author(&mut self, author: &str, limit: u32) -> Result<usize, SdkError> {
        let s = self.social()?;
        let cursor_key = format!("cursor/{author}");
        let from: u64 = self
            .one
            .store
            .get(NS_EVENTS, cursor_key.as_bytes())?
            .unwrap_or(0);
        let events = s
            .fetch_author(
                &self.one.link,
                &self.one.network,
                author,
                from,
                limit.min(200),
            )
            .await?;
        let mut max_seq = from;
        for ev in &events {
            self.one.store.put(NS_EVENTS, &ev.id, ev)?;
            max_seq = max_seq.max(ev.sequence + 1);
        }
        self.one
            .store
            .put(NS_EVENTS, cursor_key.as_bytes(), &max_seq)?;
        Ok(events.len())
    }

    /// Refreshes everyone we follow plus ourselves. Returns new events.
    pub async fn refresh(&mut self) -> Result<usize, SdkError> {
        let mut authors = self.follows();
        authors.insert(self.one.account.address().to_owned());
        let mut n = 0;
        for a in authors {
            n += self.refresh_author(&a, 100).await.unwrap_or(0);
        }
        Ok(n)
    }

    /// Chronological page over cached events from the given authors (or
    /// everyone cached when `authors` is empty). `before` is a timestamp
    /// (s), 0 = now.
    pub fn timeline(
        &self,
        authors: &BTreeSet<String>,
        kinds: &[&str],
        before: u64,
        limit: usize,
    ) -> Result<Vec<FeedItem>, SdkError> {
        let mut deleted: BTreeSet<Vec<u8>> = BTreeSet::new();
        let mut items: Vec<FeedItem> = Vec::new();
        let events: Vec<(Vec<u8>, pb::SocialEvent)> = self
            .one
            .store
            .scan::<pb::SocialEvent>(NS_EVENTS)?
            .into_iter()
            .filter(|(k, _)| !k.starts_with(b"cursor/"))
            .collect();
        for (_, ev) in &events {
            if ev.r#type == "POST_DELETE" {
                if let Ok(d) = pb::PostDelete::decode(ev.payload.as_slice()) {
                    deleted.insert(d.post);
                }
            }
        }
        for (_, ev) in events {
            if !authors.is_empty() && !authors.contains(&ev.author) {
                continue;
            }
            if !kinds.is_empty() && !kinds.contains(&ev.r#type.as_str()) {
                continue;
            }
            if before != 0 && ev.timestamp >= before {
                continue;
            }
            if deleted.contains(&ev.id) {
                continue;
            }
            items.push(item(&ev));
        }
        items.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| b.id.cmp(&a.id)));
        items.truncate(limit);
        Ok(items)
    }

    /// Following feed: posts and reposts from followed authors.
    pub fn following(&self, before: u64, limit: usize) -> Result<Vec<FeedItem>, SdkError> {
        let mut authors = self.follows();
        authors.insert(self.one.account.address().to_owned());
        self.timeline(&authors, &["POST_CREATE", "REPOST"], before, limit)
    }

    /// Friends feed: posts from contacts (People friends).
    pub fn friends(&self, before: u64, limit: usize) -> Result<Vec<FeedItem>, SdkError> {
        let authors: BTreeSet<String> = self
            .one
            .people_state
            .contacts
            .with_flag(hashgram_app::people::FRIEND)
            .into_iter()
            .map(|r| r.address.clone())
            .collect();
        self.timeline(&authors, &["POST_CREATE", "REPOST"], before, limit)
    }

    /// One author's posts.
    pub fn author(
        &self,
        address: &str,
        before: u64,
        limit: usize,
    ) -> Result<Vec<FeedItem>, SdkError> {
        let mut a = BTreeSet::new();
        a.insert(address.to_owned());
        self.timeline(&a, &["POST_CREATE", "REPOST"], before, limit)
    }

    /// A post with its comments and reaction counts.
    pub fn thread(&self, post_id_hex: &str) -> Result<Option<PostThread>, SdkError> {
        let pid = hex::decode(post_id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let post: Option<pb::SocialEvent> = self.one.store.get(NS_EVENTS, &pid)?;
        let Some(post) = post else { return Ok(None) };
        let mut comments = Vec::new();
        let mut reactions: BTreeMap<String, u32> = BTreeMap::new();
        let mut reacted: BTreeMap<String, String> = BTreeMap::new();
        let events: Vec<(Vec<u8>, pb::SocialEvent)> = self.one.store.scan(NS_EVENTS)?;
        let mut evs: Vec<pb::SocialEvent> = events
            .into_iter()
            .filter(|(k, _)| !k.starts_with(b"cursor/"))
            .map(|(_, e)| e)
            .collect();
        evs.sort_by_key(|e| e.timestamp);
        for ev in evs {
            match ev.r#type.as_str() {
                "COMMENT_CREATE" => {
                    if let Ok(c) = pb::CommentCreate::decode(ev.payload.as_slice()) {
                        if c.post == pid {
                            comments.push(item(&ev));
                        }
                    }
                }
                "REACTION" => {
                    if let Ok(r) = pb::Reaction::decode(ev.payload.as_slice()) {
                        if r.target == pid {
                            if let Some(prev) = reacted.remove(&ev.author) {
                                if let Some(n) = reactions.get_mut(&prev) {
                                    *n = n.saturating_sub(1);
                                }
                            }
                            if !r.reaction.is_empty() {
                                *reactions.entry(r.reaction.clone()).or_insert(0) += 1;
                                reacted.insert(ev.author.clone(), r.reaction);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        reactions.retain(|_, n| *n > 0);
        Ok(Some(PostThread {
            post: item(&post),
            comments,
            reactions,
        }))
    }

    /// Fetches specific events by id from the network (verified) and caches
    /// them — used to open a post referenced from mail or a Space.
    pub async fn fetch(&mut self, ids_hex: &[String]) -> Result<Vec<FeedItem>, SdkError> {
        let ids: Vec<Vec<u8>> = ids_hex.iter().filter_map(|h| hex::decode(h).ok()).collect();
        let s = self.social()?;
        let events = s.fetch_ids(&self.one.link, &self.one.network, ids).await?;
        let mut out = Vec::new();
        for ev in events {
            self.one.store.put(NS_EVENTS, &ev.id, &ev)?;
            out.push(item(&ev));
        }
        Ok(out)
    }

    /// Uploads a public media blob for a post and returns its reference.
    pub async fn upload_media(
        &mut self,
        bytes: &[u8],
        mime: &str,
        kind: &str,
    ) -> Result<pb::MediaReference, SdkError> {
        let device = self.one.account.device()?;
        let up = crate::blob::upload(
            &self.one.link,
            &self.one.network,
            &device,
            bytes,
            mime,
            false,
            2,
        )
        .await?;
        Ok(pb::MediaReference {
            cid: hex::decode(&up.cid).unwrap_or_default(),
            mime: mime.to_owned(),
            size: bytes.len() as u64,
            kind: kind.to_owned(),
            content_hash: hex::decode(&up.plaintext_hash).unwrap_or_default(),
            ..Default::default()
        })
    }
}

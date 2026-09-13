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
const NS_WALLS: &str = "feed/walls";

/// Most nodes tried for one Explore page before giving up.
const EXPLORE_ATTEMPTS: usize = 3;

/// Shown with every page or digest assembled from this device's own cache
/// because no connected node could answer.
pub const LOCAL_FALLBACK_NOTE: &str = "Computed on this device from what it already holds: the connected nodes run an older Hashgram node release that cannot answer Explore queries yet. Ask their operators to update; results will fill in on their own.";

/// Per-node memory of whether it answers Explore queries. A node running a
/// release from before the digest existed answers `invalid: empty request`
/// (it cannot decode the request body); it is asked once, then treated as
/// legacy until the link reconnects.
#[derive(Debug, Default)]
pub struct ExploreCaps {
    caps: std::collections::HashMap<crate::PeerId, bool>,
}

impl ExploreCaps {
    fn get(&self, p: &crate::PeerId) -> Option<bool> {
        self.caps.get(p).copied()
    }
    fn set(&mut self, p: crate::PeerId, ok: bool) {
        // Bounded: a client never talks to more than a handful of nodes.
        if self.caps.len() > 256 {
            self.caps.clear();
        }
        self.caps.insert(p, ok);
    }
}

/// Whether an error means "this node predates the request type".
fn legacy_node_error(e: &SdkError) -> bool {
    match e {
        SdkError::Link(crate::link::LinkError::Refused { code, message }) => {
            code == "invalid" && message.contains("empty request")
        }
        SdkError::Link(crate::link::LinkError::Unexpected) => true,
        _ => false,
    }
}

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
    /// Wall (channel) the post is on, hex id; empty for none.
    pub channel: String,
    /// Post this is a reply to, hex id; empty for none.
    pub reply_to: String,
}

fn item(ev: &pb::SocialEvent) -> FeedItem {
    let (channel, reply_to) = if ev.r#type == "POST_CREATE" {
        match pb::PostCreate::decode(ev.payload.as_slice()) {
            Ok(p) => (hex::encode(&p.channel), hex::encode(&p.reply_to)),
            Err(_) => (String::new(), String::new()),
        }
    } else {
        (String::new(), String::new())
    };
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
        channel,
        reply_to,
    }
}

/// One page of a remote (Explore / wall / hashtag) timeline.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ExplorePage {
    /// Posts, newest first.
    pub items: Vec<FeedItem>,
    /// `before` for the next page; 0 when the node has nothing older.
    pub next_before: u64,
    /// Peer id of the node that answered.
    pub source_peer: String,
    /// Operator address that node claimed, if any.
    pub source_operator: String,
    /// Measured round-trip to that node, ms, once known.
    pub source_rtt_ms: Option<u64>,
    /// Empty when a node answered; otherwise why this came from the local
    /// cache instead (see [`LOCAL_FALLBACK_NOTE`]).
    pub note: String,
}

/// A remote timeline query.
#[derive(Debug, Clone, Default)]
pub struct ExploreQuery {
    /// Timestamp bound (exclusive), 0 = now.
    pub before: u64,
    /// Page size, clamped to 1..=100.
    pub limit: u32,
    /// Only this hashtag (with or without `#`).
    pub hashtag: String,
    /// Only this wall, hex id.
    pub channel: String,
    /// Event types; empty = posts and reels.
    pub types: Vec<String>,
}

/// A wall (channel): a topic anybody can open and, when `open_posting`,
/// anybody can write on.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WallInfo {
    /// Hex id of the CHANNEL_CREATE event.
    pub id: String,
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Creator address.
    pub creator: String,
    /// Whether anyone may post.
    pub open_posting: bool,
    /// Creation time (s).
    pub created_at: u64,
    /// Posts the answering node counted (digest only).
    pub posts: u32,
    /// Distinct posters the node counted (digest only).
    pub authors: u32,
    /// Newest post time (digest only).
    pub last_post: u64,
    /// Whether this wall is pinned locally.
    pub pinned: bool,
}

/// One author in a digest.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AuthorActivity {
    /// Address.
    pub author: String,
    /// Posts in the window.
    pub posts: u32,
    /// Comments written in the window.
    pub comments: u32,
    /// Reactions their content received.
    pub reactions_received: u32,
    /// Comments their content received.
    pub comments_received: u32,
    /// Last event time.
    pub last_active: u64,
}

/// One hashtag in a digest.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HashtagActivity {
    /// The tag, lower-case, no `#`.
    pub tag: String,
    /// Posts.
    pub posts: u32,
    /// Distinct authors.
    pub authors: u32,
    /// Last use.
    pub last_used: u64,
}

/// What one node sees happening.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Digest {
    /// Window looked back over (s).
    pub window_secs: u64,
    /// Events in the window.
    pub events: u64,
    /// Distinct active authors in the window.
    pub authors: u64,
    /// Events the node holds in total.
    pub total_events: u64,
    /// Distinct authors the node holds in total.
    pub total_authors: u64,
    /// Most active authors.
    pub top_authors: Vec<AuthorActivity>,
    /// Busiest hashtags.
    pub top_hashtags: Vec<HashtagActivity>,
    /// Walls, most recently active first.
    pub walls: Vec<WallInfo>,
    /// When the node computed it.
    pub computed_at: u64,
    /// Node that answered.
    pub source_peer: String,
    /// Its operator, if claimed.
    pub source_operator: String,
    /// Round-trip to it, ms.
    pub source_rtt_ms: Option<u64>,
    /// Empty when a node answered; otherwise why this came from the local
    /// cache instead (see [`LOCAL_FALLBACK_NOTE`]).
    pub note: String,
}

/// My own public activity, as counted from my event chain.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MyActivity {
    /// Posts (POST_CREATE + REEL_CREATE), excluding deleted.
    pub posts: u32,
    /// Comments written.
    pub comments: u32,
    /// Reactions given.
    pub reactions: u32,
    /// Reposts.
    pub reposts: u32,
    /// Walls created.
    pub walls_created: u32,
    /// Accounts followed (public FOLLOW events still in effect).
    pub following: u32,
    /// Events in total.
    pub events: u32,
    /// Distinct walls posted on.
    pub walls_posted: Vec<String>,
    /// First event time (0 if none).
    pub first_event: u64,
    /// Last event time (0 if none).
    pub last_event: u64,
    /// Engagement received, as far as the local cache knows.
    pub reactions_received: u32,
    /// Comments received, as far as the local cache knows.
    pub comments_received: u32,
    /// A transparent activity score: see [`activity_score`].
    pub score: u64,
}

/// A plain, explainable activity score. Not a rank of worth: it counts
/// public contribution so a profile can show a number that means
/// something and that anyone can recompute from the public log.
///
/// posts ×10 + comments ×3 + reposts ×2 + walls created ×15 + reactions
/// given ×1 + reactions received ×2 + comments received ×4.
#[must_use]
pub fn activity_score(a: &MyActivity) -> u64 {
    u64::from(a.posts) * 10
        + u64::from(a.comments) * 3
        + u64::from(a.reposts) * 2
        + u64::from(a.walls_created) * 15
        + u64::from(a.reactions)
        + u64::from(a.reactions_received) * 2
        + u64::from(a.comments_received) * 4
}

fn wall_from_event(ev: &pb::SocialEvent) -> Option<WallInfo> {
    if ev.r#type != "CHANNEL_CREATE" {
        return None;
    }
    let c = pb::ChannelCreate::decode(ev.payload.as_slice()).ok()?;
    Some(WallInfo {
        id: hex::encode(&ev.id),
        name: c.name,
        description: c.description,
        creator: ev.author.clone(),
        open_posting: c.open_posting,
        created_at: ev.timestamp,
        ..Default::default()
    })
}

fn hex32(field: &str, h: &str) -> Result<Vec<u8>, SdkError> {
    let b = hex::decode(h).map_err(|e| SdkError::Invalid(format!("{field}: {e}")))?;
    if b.len() != 32 {
        return Err(SdkError::Invalid(format!("{field}: not a 32-byte id")));
    }
    Ok(b)
}

/// Validates a wall name: 2..=64 chars, letters/digits/space/`_-.`.
pub fn validate_wall_name(name: &str) -> Result<(), SdkError> {
    let n = name.trim();
    if n.chars().count() < 2 || n.len() > 64 {
        return Err(SdkError::Invalid(
            "wall name must be 2 to 64 characters".into(),
        ));
    }
    if !n
        .chars()
        .all(|c| c.is_alphanumeric() || c == ' ' || c == '_' || c == '-' || c == '.')
    {
        return Err(SdkError::Invalid(
            "wall name may contain letters, digits, spaces, '_', '-' and '.'".into(),
        ));
    }
    Ok(())
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
        self.post_on(text, hashtags, media, sensitive, "").await
    }

    /// Creates a public post, optionally on a wall (`channel_hex`). Posting
    /// on a closed wall you did not create is refused here, before anything
    /// is signed; nodes and other clients hide such posts too.
    pub async fn post_on(
        &mut self,
        text: &str,
        hashtags: Vec<String>,
        media: Vec<pb::MediaReference>,
        sensitive: bool,
        channel_hex: &str,
    ) -> Result<String, SdkError> {
        let channel = if channel_hex.is_empty() {
            Vec::new()
        } else {
            let wall = self.wall_info(channel_hex).await?;
            if !wall.open_posting && wall.creator != self.one.account.address() {
                return Err(SdkError::Invalid(
                    "only its creator may post on this wall".into(),
                ));
            }
            hex32("channel", channel_hex)?
        };
        self.publish(
            "POST_CREATE",
            &pb::PostCreate {
                text: text.to_owned(),
                hashtags,
                sensitive,
                channel,
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

    // -- Explore: pages from the nearest nodes --------------------------------

    /// One page of a public timeline from the nearest node that answers.
    /// Nothing is bulk-downloaded: a page is `limit` posts, verified, and
    /// the cursor for the next one. Pages are not cached in the store; open
    /// a post with [`Self::thread_fetch`] to keep it.
    pub async fn explore(&mut self, q: &ExploreQuery) -> Result<ExplorePage, SdkError> {
        let s = self.social()?;
        let limit = q.limit.clamp(1, 100);
        let types = if q.types.is_empty() {
            vec!["POST_CREATE".to_owned(), "REEL_CREATE".to_owned()]
        } else {
            q.types.clone()
        };
        let channel = if q.channel.is_empty() {
            Vec::new()
        } else {
            hex32("channel", &q.channel)?
        };
        let query = pb::EventFetch {
            limit,
            before_timestamp: q.before,
            types,
            hashtag: q.hashtag.trim().trim_start_matches('#').to_lowercase(),
            channel,
            ..Default::default()
        };
        let (capable, legacy) = self.explore_peers(&s).await?;
        if capable.is_empty() {
            // Every reachable node is a legacy release: it would answer a
            // timeline query with an empty page, which is not "nothing is
            // happening". Say so and serve the local cache instead.
            debug_assert!(legacy > 0);
            return self.local_explore(q, &query.types);
        }
        let mut last: Option<SdkError> = None;
        for rp in capable.iter().take(EXPLORE_ATTEMPTS) {
            match s
                .fetch_from(&self.one.link, &self.one.network, rp.peer.peer, query.clone())
                .await
            {
                Ok(r) => {
                    let mut deleted: BTreeSet<Vec<u8>> = BTreeSet::new();
                    for ev in &r.events {
                        if ev.r#type == "POST_DELETE" {
                            if let Ok(d) = pb::PostDelete::decode(ev.payload.as_slice()) {
                                deleted.insert(d.post);
                            }
                        }
                    }
                    let blocked = self.blocked_authors();
                    let items = r
                        .events
                        .iter()
                        .filter(|e| !deleted.contains(&e.id) && !blocked.contains(&e.author))
                        .map(item)
                        .collect();
                    return Ok(ExplorePage {
                        items,
                        next_before: r.next_before,
                        source_peer: rp.peer.peer.to_string(),
                        source_operator: rp.peer.operator.clone(),
                        source_rtt_ms: rp.rtt_ms,
                        note: String::new(),
                    });
                }
                Err(e) => {
                    if legacy_node_error(&e) {
                        self.one.explore_caps.set(rp.peer.peer, false);
                    }
                    last = Some(e);
                }
            }
        }
        match last {
            Some(e) if legacy_node_error(&e) => self.local_explore(q, &query.types),
            Some(e) => Err(e),
            None => Err(SdkError::Link(crate::link::LinkError::NoPeer("any"))),
        }
    }

    /// Nearest nodes that answer Explore queries, and how many legacy nodes
    /// were skipped. Unknown nodes are probed once with a tiny digest.
    /// `Err(NoPeer)` when nothing is connected at all.
    async fn explore_peers(
        &mut self,
        s: &Social,
    ) -> Result<(Vec<crate::link::RankedPeer>, usize), SdkError> {
        let ranked = self.one.link.peers_ranked().await;
        if ranked.is_empty() {
            return Err(SdkError::Link(crate::link::LinkError::NoPeer("any")));
        }
        let mut capable = Vec::new();
        let mut legacy = 0usize;
        for rp in ranked.into_iter().take(EXPLORE_ATTEMPTS + 2) {
            let known = self.one.explore_caps.get(&rp.peer.peer);
            let ok = match known {
                Some(v) => v,
                None => match s.digest_from(&self.one.link, rp.peer.peer, 1, 1).await {
                    Ok(_) => {
                        self.one.explore_caps.set(rp.peer.peer, true);
                        true
                    }
                    Err(e) if legacy_node_error(&e) => {
                        tracing::info!(peer = %rp.peer.peer, "node predates Explore; skipping it");
                        self.one.explore_caps.set(rp.peer.peer, false);
                        false
                    }
                    // A timeout or transport error says nothing about the
                    // release; try again next time.
                    Err(_) => continue,
                },
            };
            if ok {
                capable.push(rp);
            } else {
                legacy += 1;
            }
        }
        Ok((capable, legacy))
    }

    /// An Explore page from the local cache: what this device has already
    /// verified (own posts, followed authors, friends, opened threads).
    fn local_explore(&self, q: &ExploreQuery, types: &[String]) -> Result<ExplorePage, SdkError> {
        let kinds: Vec<&str> = types.iter().map(String::as_str).collect();
        let limit = q.limit.clamp(1, 100) as usize;
        let tag = q.hashtag.trim().trim_start_matches('#').to_lowercase();
        let channel = q.channel.to_lowercase();
        // Over-fetch, then filter by tag / wall which `timeline` does not know.
        let all = self.timeline(&BTreeSet::new(), &kinds, q.before, 5_000)?;
        let mut items: Vec<FeedItem> = all
            .into_iter()
            .filter(|it| channel.is_empty() || it.channel == channel)
            .filter(|it| {
                if tag.is_empty() {
                    return true;
                }
                it.payload
                    .get("hashtags")
                    .and_then(|h| h.as_array())
                    .is_some_and(|arr| {
                        arr.iter().any(|t| {
                            t.as_str()
                                .map(|s| s.trim_start_matches('#').to_lowercase() == tag)
                                .unwrap_or(false)
                        })
                    })
            })
            .collect();
        let more = items.len() > limit;
        items.truncate(limit);
        let next_before = if more {
            items.last().map(|i| i.timestamp).unwrap_or(0)
        } else {
            0
        };
        Ok(ExplorePage {
            items,
            next_before,
            source_peer: String::new(),
            source_operator: String::new(),
            source_rtt_ms: None,
            note: LOCAL_FALLBACK_NOTE.to_owned(),
        })
    }

    /// A digest over the local cache, shaped like a node's answer.
    fn local_digest(&self, window_secs: u64, limit: u32) -> Result<Digest, SdkError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let since = now.saturating_sub(window_secs.max(1));
        let limit = limit.clamp(1, 50) as usize;
        let blocked = self.blocked_authors();
        let pinned = self.pinned_ids();
        let events: Vec<pb::SocialEvent> = self
            .one
            .store
            .scan::<pb::SocialEvent>(NS_EVENTS)?
            .into_iter()
            .filter(|(k, _)| !k.starts_with(b"cursor/"))
            .map(|(_, e)| e)
            .filter(|e| !blocked.contains(&e.author))
            .collect();
        let mut deleted: BTreeSet<Vec<u8>> = BTreeSet::new();
        let mut author_of: BTreeMap<Vec<u8>, String> = BTreeMap::new();
        for ev in &events {
            author_of.insert(ev.id.clone(), ev.author.clone());
            if ev.r#type == "POST_DELETE" {
                if let Ok(d) = pb::PostDelete::decode(ev.payload.as_slice()) {
                    deleted.insert(d.post);
                }
            }
        }
        let mut total_authors: BTreeSet<&str> = BTreeSet::new();
        let mut in_window = 0u64;
        let mut active: BTreeSet<&str> = BTreeSet::new();
        let mut authors: BTreeMap<String, AuthorActivity> = BTreeMap::new();
        let mut tags: BTreeMap<String, (u32, BTreeSet<String>, u64)> = BTreeMap::new();
        let mut walls: BTreeMap<String, WallInfo> = BTreeMap::new();
        let mut wall_posts: BTreeMap<String, (u32, BTreeSet<String>, u64)> = BTreeMap::new();
        for ev in &events {
            total_authors.insert(&ev.author);
            if let Some(w) = wall_from_event(ev) {
                walls.insert(w.id.clone(), w);
            }
            if ev.timestamp < since || deleted.contains(&ev.id) {
                continue;
            }
            in_window += 1;
            active.insert(&ev.author);
            let a = authors.entry(ev.author.clone()).or_insert_with(|| AuthorActivity {
                author: ev.author.clone(),
                ..Default::default()
            });
            a.last_active = a.last_active.max(ev.timestamp);
            match ev.r#type.as_str() {
                "POST_CREATE" | "REEL_CREATE" => {
                    a.posts += 1;
                    if let Ok(p) = pb::PostCreate::decode(ev.payload.as_slice()) {
                        for t in p.hashtags {
                            let t = t.trim_start_matches('#').to_lowercase();
                            if t.is_empty() {
                                continue;
                            }
                            let e = tags.entry(t).or_insert((0, BTreeSet::new(), 0));
                            e.0 += 1;
                            e.1.insert(ev.author.clone());
                            e.2 = e.2.max(ev.timestamp);
                        }
                        if !p.channel.is_empty() {
                            let e = wall_posts
                                .entry(hex::encode(&p.channel))
                                .or_insert((0, BTreeSet::new(), 0));
                            e.0 += 1;
                            e.1.insert(ev.author.clone());
                            e.2 = e.2.max(ev.timestamp);
                        }
                    }
                }
                "COMMENT_CREATE" => {
                    a.comments += 1;
                    if let Ok(c) = pb::CommentCreate::decode(ev.payload.as_slice()) {
                        if let Some(owner) = author_of.get(&c.post).cloned() {
                            if owner != ev.author {
                                authors
                                    .entry(owner.clone())
                                    .or_insert_with(|| AuthorActivity { author: owner, ..Default::default() })
                                    .comments_received += 1;
                            }
                        }
                    }
                }
                "REACTION" => {
                    if let Ok(r) = pb::Reaction::decode(ev.payload.as_slice()) {
                        if let Some(owner) = author_of.get(&r.target).cloned() {
                            if owner != ev.author {
                                authors
                                    .entry(owner.clone())
                                    .or_insert_with(|| AuthorActivity { author: owner, ..Default::default() })
                                    .reactions_received += 1;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        // Walls we know by description only (digest cache) count too.
        for (k, w) in self.one.store.scan::<WallInfo>(NS_WALLS)? {
            if k.starts_with(b"info/") && !walls.contains_key(&w.id) && !blocked.contains(&w.creator) {
                walls.insert(w.id.clone(), w);
            }
        }
        let mut walls: Vec<WallInfo> = walls
            .into_values()
            .map(|mut w| {
                if let Some((posts, people, last)) = wall_posts.get(&w.id) {
                    w.posts = *posts;
                    w.authors = people.len() as u32;
                    w.last_post = *last;
                }
                w.pinned = pinned.contains(&w.id);
                w
            })
            .collect();
        walls.sort_by(|a, b| b.last_post.max(b.created_at).cmp(&a.last_post.max(a.created_at)));
        walls.truncate(limit);
        let mut top_authors: Vec<AuthorActivity> = authors
            .into_values()
            .filter(|a| a.posts + a.comments + a.reactions_received + a.comments_received > 0)
            .collect();
        top_authors.sort_by(|a, b| {
            (b.posts, b.comments, b.reactions_received).cmp(&(a.posts, a.comments, a.reactions_received))
        });
        top_authors.truncate(limit);
        let mut top_hashtags: Vec<HashtagActivity> = tags
            .into_iter()
            .map(|(tag, (posts, people, last))| HashtagActivity {
                tag,
                posts,
                authors: people.len() as u32,
                last_used: last,
            })
            .collect();
        top_hashtags.sort_by(|a, b| (b.posts, b.authors).cmp(&(a.posts, a.authors)));
        top_hashtags.truncate(limit);
        Ok(Digest {
            window_secs,
            events: in_window,
            authors: active.len() as u64,
            total_events: events.len() as u64,
            total_authors: total_authors.len() as u64,
            top_authors,
            top_hashtags,
            walls,
            computed_at: now,
            source_peer: String::new(),
            source_operator: String::new(),
            source_rtt_ms: None,
            note: LOCAL_FALLBACK_NOTE.to_owned(),
        })
    }

    /// Addresses we have blocked; their public posts are hidden locally.
    fn blocked_authors(&self) -> BTreeSet<String> {
        self.one
            .people_state
            .contacts
            .with_flag(hashgram_app::people::BLOCKED)
            .into_iter()
            .map(|r| r.address.clone())
            .collect()
    }

    /// What is active, according to the nearest node that answers.
    pub async fn digest(&mut self, window_secs: u64, limit: u32) -> Result<Digest, SdkError> {
        let s = self.social()?;
        let (capable, _legacy) = self.explore_peers(&s).await?;
        if capable.is_empty() {
            return self.local_digest(window_secs, limit);
        }
        let pinned = self.pinned_ids();
        let blocked = self.blocked_authors();
        let mut last: Option<SdkError> = None;
        for rp in capable.iter().take(EXPLORE_ATTEMPTS) {
            match s
                .digest_from(&self.one.link, rp.peer.peer, window_secs, limit)
                .await
            {
                Ok(d) => {
                    let walls: Vec<WallInfo> = d
                        .channels
                        .into_iter()
                        .filter(|c| !blocked.contains(&c.creator))
                        .map(|c| {
                            let id = hex::encode(&c.id);
                            WallInfo {
                                pinned: pinned.contains(&id),
                                id,
                                name: c.name,
                                description: c.description,
                                creator: c.creator,
                                open_posting: c.open_posting,
                                created_at: c.created_at,
                                posts: c.posts,
                                authors: c.authors,
                                last_post: c.last_post,
                            }
                        })
                        .collect();
                    // Remember what the network told us about walls so a
                    // wall page can open without another round-trip.
                    for w in &walls {
                        let _ = self
                            .one
                            .store
                            .put(NS_WALLS, format!("info/{}", w.id).as_bytes(), w);
                    }
                    return Ok(Digest {
                        window_secs: d.window_secs,
                        events: d.events,
                        authors: d.authors,
                        total_events: d.total_events,
                        total_authors: d.total_authors,
                        top_authors: d
                            .top_authors
                            .into_iter()
                            .filter(|a| !blocked.contains(&a.author))
                            .map(|a| AuthorActivity {
                                author: a.author,
                                posts: a.posts,
                                comments: a.comments,
                                reactions_received: a.reactions_received,
                                comments_received: a.comments_received,
                                last_active: a.last_active,
                            })
                            .collect(),
                        top_hashtags: d
                            .top_hashtags
                            .into_iter()
                            .map(|t| HashtagActivity {
                                tag: t.tag,
                                posts: t.posts,
                                authors: t.authors,
                                last_used: t.last_used,
                            })
                            .collect(),
                        walls,
                        computed_at: d.computed_at,
                        source_peer: rp.peer.peer.to_string(),
                        source_operator: rp.peer.operator.clone(),
                        source_rtt_ms: rp.rtt_ms,
                        note: String::new(),
                    });
                }
                Err(e) => {
                    if legacy_node_error(&e) {
                        self.one.explore_caps.set(rp.peer.peer, false);
                    }
                    last = Some(e);
                }
            }
        }
        match last {
            Some(e) if legacy_node_error(&e) => self.local_digest(window_secs, limit),
            Some(e) => Err(e),
            None => Err(SdkError::Link(crate::link::LinkError::NoPeer("any"))),
        }
    }

    /// Opens a post from the network: the post itself plus every comment,
    /// reaction and repost a node holds for it, verified and cached, then
    /// assembled like [`Self::thread`]. Works for posts found in Explore
    /// that this device never cached.
    pub async fn thread_fetch(&mut self, post_id_hex: &str) -> Result<Option<PostThread>, SdkError> {
        let pid = hex32("post", post_id_hex)?;
        let s = self.social()?;
        let cached: Option<pb::SocialEvent> = self.one.store.get(NS_EVENTS, &pid)?;
        if cached.is_none() {
            let got = s
                .fetch_ids(&self.one.link, &self.one.network, vec![pid.clone()])
                .await?;
            for ev in got {
                self.one.store.put(NS_EVENTS, &ev.id, &ev)?;
            }
        }
        let under = s
            .fetch(
                &self.one.link,
                &self.one.network,
                pb::EventFetch {
                    target: pid.clone(),
                    limit: 200,
                    ..Default::default()
                },
            )
            .await;
        if let Ok(r) = under {
            for ev in r.events {
                self.one.store.put(NS_EVENTS, &ev.id, &ev)?;
            }
        }
        self.thread(post_id_hex)
    }

    // -- Walls ----------------------------------------------------------------

    /// Opens a wall: a topic everyone can see and, when `open_posting`,
    /// everyone can write on. Returns its hex id. The wall is pinned
    /// locally.
    pub async fn create_wall(
        &mut self,
        name: &str,
        description: &str,
        open_posting: bool,
    ) -> Result<WallInfo, SdkError> {
        validate_wall_name(name)?;
        if description.len() > 2_000 {
            return Err(SdkError::Invalid("description too long".into()));
        }
        let id = self
            .publish(
                "CHANNEL_CREATE",
                &pb::ChannelCreate {
                    name: name.trim().to_owned(),
                    description: description.trim().to_owned(),
                    open_posting,
                    ..Default::default()
                },
                vec![],
            )
            .await?;
        let ev: Option<pb::SocialEvent> = self
            .one
            .store
            .get(NS_EVENTS, &hex::decode(&id).unwrap_or_default())?;
        let mut info = ev
            .as_ref()
            .and_then(wall_from_event)
            .ok_or_else(|| SdkError::Store("wall event not cached".into()))?;
        info.pinned = true;
        self.one
            .store
            .put(NS_WALLS, format!("info/{id}").as_bytes(), &info)?;
        self.pin_wall(&id, true)?;
        Ok(info)
    }

    /// A wall's description, from the local cache or the network.
    pub async fn wall_info(&mut self, id_hex: &str) -> Result<WallInfo, SdkError> {
        let id = hex32("wall", id_hex)?;
        let key = format!("info/{id_hex}");
        let pinned = self.pinned_ids().contains(id_hex);
        if let Some(mut w) = self.one.store.get::<WallInfo>(NS_WALLS, key.as_bytes())? {
            w.pinned = pinned;
            return Ok(w);
        }
        let ev: Option<pb::SocialEvent> = self.one.store.get(NS_EVENTS, &id)?;
        let ev = match ev {
            Some(e) => Some(e),
            None => {
                let s = self.social()?;
                s.fetch_ids(&self.one.link, &self.one.network, vec![id])
                    .await?
                    .into_iter()
                    .next()
            }
        };
        let mut info = ev
            .as_ref()
            .and_then(wall_from_event)
            .ok_or_else(|| SdkError::NotFound(format!("wall {id_hex}")))?;
        info.pinned = pinned;
        self.one.store.put(NS_WALLS, key.as_bytes(), &info)?;
        Ok(info)
    }

    /// One page of a wall's posts from the nearest node.
    pub async fn wall_page(
        &mut self,
        id_hex: &str,
        before: u64,
        limit: u32,
    ) -> Result<ExplorePage, SdkError> {
        self.explore(&ExploreQuery {
            before,
            limit,
            channel: id_hex.to_owned(),
            types: vec!["POST_CREATE".to_owned()],
            ..Default::default()
        })
        .await
    }

    fn pinned_ids(&self) -> BTreeSet<String> {
        self.one
            .store
            .get(NS_WALLS, b"pinned")
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// Pins or unpins a wall in the Hashwall sidebar.
    pub fn pin_wall(&mut self, id_hex: &str, on: bool) -> Result<(), SdkError> {
        hex32("wall", id_hex)?;
        let mut set = self.pinned_ids();
        if on {
            set.insert(id_hex.to_owned());
        } else {
            set.remove(id_hex);
        }
        self.one.store.put(NS_WALLS, b"pinned", &set)?;
        Ok(())
    }

    /// Pinned walls, newest pin last, from the local cache.
    pub fn pinned_walls(&self) -> Vec<WallInfo> {
        self.pinned_ids()
            .into_iter()
            .filter_map(|id| {
                let mut w: WallInfo = self
                    .one
                    .store
                    .get(NS_WALLS, format!("info/{id}").as_bytes())
                    .ok()
                    .flatten()?;
                w.pinned = true;
                Some(w)
            })
            .collect()
    }

    // -- Me -------------------------------------------------------------------

    /// My public activity, counted from my cached event chain (refresh with
    /// [`Self::refresh_author`] first for the network's view).
    pub fn my_activity(&self) -> Result<MyActivity, SdkError> {
        let me = self.one.account.address().to_owned();
        let events: Vec<pb::SocialEvent> = self
            .one
            .store
            .scan::<pb::SocialEvent>(NS_EVENTS)?
            .into_iter()
            .filter(|(k, _)| !k.starts_with(b"cursor/"))
            .map(|(_, e)| e)
            .collect();
        let mut a = MyActivity::default();
        let mut deleted: BTreeSet<Vec<u8>> = BTreeSet::new();
        let mut follows: BTreeSet<String> = BTreeSet::new();
        let mut my_ids: BTreeSet<Vec<u8>> = BTreeSet::new();
        let mut walls: BTreeSet<String> = BTreeSet::new();
        for ev in events.iter().filter(|e| e.author == me) {
            if ev.r#type == "POST_DELETE" {
                if let Ok(d) = pb::PostDelete::decode(ev.payload.as_slice()) {
                    deleted.insert(d.post);
                }
            }
            my_ids.insert(ev.id.clone());
        }
        for ev in &events {
            if ev.author == me {
                a.events += 1;
                a.first_event = if a.first_event == 0 {
                    ev.timestamp
                } else {
                    a.first_event.min(ev.timestamp)
                };
                a.last_event = a.last_event.max(ev.timestamp);
                match ev.r#type.as_str() {
                    "POST_CREATE" | "REEL_CREATE" if !deleted.contains(&ev.id) => {
                        a.posts += 1;
                        if let Ok(p) = pb::PostCreate::decode(ev.payload.as_slice()) {
                            if !p.channel.is_empty() {
                                walls.insert(hex::encode(&p.channel));
                            }
                        }
                    }
                    "COMMENT_CREATE" => a.comments += 1,
                    "REACTION" => {
                        if let Ok(r) = pb::Reaction::decode(ev.payload.as_slice()) {
                            if !r.reaction.is_empty() {
                                a.reactions += 1;
                            }
                        }
                    }
                    "REPOST" => a.reposts += 1,
                    "CHANNEL_CREATE" => a.walls_created += 1,
                    "FOLLOW" => {
                        if let Ok(f) = pb::Follow::decode(ev.payload.as_slice()) {
                            follows.insert(f.target);
                        }
                    }
                    "UNFOLLOW" => {
                        if let Ok(f) = pb::Unfollow::decode(ev.payload.as_slice()) {
                            follows.remove(&f.target);
                        }
                    }
                    _ => {}
                }
            } else {
                match ev.r#type.as_str() {
                    "COMMENT_CREATE" => {
                        if let Ok(c) = pb::CommentCreate::decode(ev.payload.as_slice()) {
                            if my_ids.contains(&c.post) {
                                a.comments_received += 1;
                            }
                        }
                    }
                    "REACTION" => {
                        if let Ok(r) = pb::Reaction::decode(ev.payload.as_slice()) {
                            if my_ids.contains(&r.target) && !r.reaction.is_empty() {
                                a.reactions_received += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        a.following = follows.len() as u32;
        a.walls_posted = walls.into_iter().collect();
        a.score = activity_score(&a);
        Ok(a)
    }

    /// My cached events of every type, newest first — the "everything I
    /// did" list on the profile page.
    pub fn my_events(&self, before: u64, limit: usize) -> Result<Vec<FeedItem>, SdkError> {
        let mut a = BTreeSet::new();
        a.insert(self.one.account.address().to_owned());
        self.timeline(&a, &[], before, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_names_are_bounded_and_plain() {
        assert!(validate_wall_name("ArchevnebiSaqartveloshi").is_ok());
        assert!(validate_wall_name("არჩევნები 2026").is_ok());
        assert!(validate_wall_name("a").is_err());
        assert!(validate_wall_name("bad<name>").is_err());
        assert!(validate_wall_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn activity_score_is_a_plain_sum() {
        let a = MyActivity {
            posts: 2,
            comments: 1,
            reposts: 1,
            walls_created: 1,
            reactions: 5,
            reactions_received: 3,
            comments_received: 2,
            ..Default::default()
        };
        assert_eq!(activity_score(&a), 20 + 3 + 2 + 15 + 5 + 6 + 8);
    }

    #[test]
    fn items_expose_wall_and_reply_as_hex() {
        let ev = pb::SocialEvent {
            r#type: "POST_CREATE".into(),
            payload: pb::PostCreate {
                text: "x".into(),
                channel: vec![1; 32],
                ..Default::default()
            }
            .encode_to_vec(),
            ..Default::default()
        };
        let it = item(&ev);
        assert_eq!(it.channel, "01".repeat(32));
        assert_eq!(it.reply_to, "");
    }
}

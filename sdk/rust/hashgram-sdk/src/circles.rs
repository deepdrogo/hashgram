//! Circles: private groups (Family, Close Friends, Team…) whose posts are
//! `CircleEvent`s inside one MLS group.
//!
//! Membership *is* the MLS roster. Adding a member adds every device of
//! that address in a new epoch, so they cannot decrypt anything sent
//! before (history is not shared unless a member deliberately re-shares).
//! Removing a member commits a removal; from the next epoch they receive
//! nothing. The timeline projection is `hashgram_app::circle::Timeline`,
//! rebuilt from the events kept in the local store.

use hashgram_app::circle as c;
use hashgram_app::pb as app;

use crate::app::{group_kind, HashgramOne};
use crate::messaging::Received;
use crate::store::LocalStore;
use crate::SdkError;

const NS_INFO: &str = "circles/info";
const NS_EVENTS: &str = "circles/events";

/// A circle as listed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CircleInfo {
    /// Hex group id.
    pub id: String,
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Members (addresses).
    pub members: Vec<String>,
    /// Created / joined.
    pub since: u64,
    /// Whether we created it.
    pub owner: bool,
}

/// Circles state.
#[derive(Debug, Default)]
pub struct CirclesState {
    timelines: std::collections::HashMap<String, c::Timeline>,
}

impl CirclesState {
    pub(crate) fn load(_store: &LocalStore) -> Result<Self, SdkError> {
        Ok(Self::default())
    }
}

/// The Circles API.
pub struct Circles<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> Circles<'a> {
    /// Creates a circle with initial members.
    pub async fn create(
        &mut self,
        name: &str,
        description: &str,
        members: &[String],
    ) -> Result<String, SdkError> {
        if name.is_empty() || name.len() > 128 {
            return Err(SdkError::Invalid("circle name length".into()));
        }
        let gid = self
            .one
            .messaging
            .create_conversation(
                &self.one.link,
                &self.one.chain,
                &self.one.network,
                name,
                members,
            )
            .await?;
        let hexid = hex::encode(&gid);
        self.one.set_group_kind(&hexid, group_kind::CIRCLE)?;
        let info = CircleInfo {
            id: hexid.clone(),
            name: name.to_owned(),
            description: description.to_owned(),
            members: members.to_vec(),
            since: hashgram_app::ids::now_secs(),
            owner: true,
        };
        self.one.store.put(NS_INFO, hexid.as_bytes(), &info)?;
        let ev = c::build(app::circle_event::Body::Info(app::CircleInfo {
            name: name.to_owned(),
            description: description.to_owned(),
            avatar: None,
            history_visible_to_new_members: false,
        }))?;
        self.one
            .send_app(&gid, app::app_message::Body::CircleEvent(ev.clone()))
            .await?;
        let me = self.one.account.address().to_owned();
        self.record(&hexid, &me, &ev)?;
        Ok(hexid)
    }

    /// Adds a member (every device of the address).
    pub async fn add_member(&mut self, circle_hex: &str, address: &str) -> Result<(), SdkError> {
        let gid = self.gid(circle_hex)?;
        self.one
            .messaging
            .add_participant(
                &self.one.link,
                &self.one.chain,
                &self.one.network,
                &gid,
                address,
            )
            .await?;
        if let Some(mut info) = self
            .one
            .store
            .get::<CircleInfo>(NS_INFO, circle_hex.as_bytes())?
        {
            if !info.members.iter().any(|m| m == address) {
                info.members.push(address.to_owned());
            }
            self.one.store.put(NS_INFO, circle_hex.as_bytes(), &info)?;
        }
        Ok(())
    }

    /// Removes a member.
    pub async fn remove_member(
        &mut self,
        circle_hex: &str,
        address: &str,
    ) -> Result<usize, SdkError> {
        let gid = self.gid(circle_hex)?;
        let n = self
            .one
            .messaging
            .remove_participant(&self.one.link, &self.one.network, &gid, address)
            .await?;
        if let Some(mut info) = self
            .one
            .store
            .get::<CircleInfo>(NS_INFO, circle_hex.as_bytes())?
        {
            info.members.retain(|m| m != address);
            self.one.store.put(NS_INFO, circle_hex.as_bytes(), &info)?;
        }
        Ok(n)
    }

    /// Leaves a circle (removes our own devices).
    pub async fn leave(&mut self, circle_hex: &str) -> Result<(), SdkError> {
        let me = self.one.account.address().to_owned();
        self.remove_member(circle_hex, &me).await?;
        self.one.store.delete(NS_INFO, circle_hex.as_bytes())?;
        Ok(())
    }

    fn gid(&self, circle_hex: &str) -> Result<Vec<u8>, SdkError> {
        if self.one.group_kind(circle_hex).as_deref() != Some(group_kind::CIRCLE) {
            return Err(SdkError::NotFound(format!("circle {circle_hex}")));
        }
        hex::decode(circle_hex).map_err(|e| SdkError::Invalid(e.to_string()))
    }

    /// Posts to a circle.
    pub async fn post(
        &mut self,
        circle_hex: &str,
        text: &str,
        media: Vec<app::BlobRef>,
        drive_refs: Vec<app::DriveCapability>,
        poll: Option<app::Poll>,
    ) -> Result<String, SdkError> {
        let gid = self.gid(circle_hex)?;
        let ev = c::build(app::circle_event::Body::Post(app::CirclePost {
            text: text.to_owned(),
            media,
            drive_refs,
            poll,
        }))?;
        self.send(&gid, circle_hex, ev).await
    }

    /// Comments.
    pub async fn comment(
        &mut self,
        circle_hex: &str,
        post_hex: &str,
        text: &str,
    ) -> Result<String, SdkError> {
        let gid = self.gid(circle_hex)?;
        let ev = c::build(app::circle_event::Body::Comment(app::CircleComment {
            post_id: hex::decode(post_hex).map_err(|e| SdkError::Invalid(e.to_string()))?,
            reply_to: Vec::new(),
            text: text.to_owned(),
        }))?;
        self.send(&gid, circle_hex, ev).await
    }

    /// Reacts.
    pub async fn react(
        &mut self,
        circle_hex: &str,
        target_hex: &str,
        reaction: &str,
    ) -> Result<String, SdkError> {
        let gid = self.gid(circle_hex)?;
        let ev = c::build(app::circle_event::Body::Reaction(app::CircleReaction {
            target_id: hex::decode(target_hex).map_err(|e| SdkError::Invalid(e.to_string()))?,
            reaction: reaction.to_owned(),
        }))?;
        self.send(&gid, circle_hex, ev).await
    }

    /// Votes.
    pub async fn vote(
        &mut self,
        circle_hex: &str,
        post_hex: &str,
        options: Vec<u32>,
    ) -> Result<String, SdkError> {
        let gid = self.gid(circle_hex)?;
        let ev = c::build(app::circle_event::Body::Vote(app::CircleVote {
            post_id: hex::decode(post_hex).map_err(|e| SdkError::Invalid(e.to_string()))?,
            option_indexes: options,
        }))?;
        self.send(&gid, circle_hex, ev).await
    }

    /// Deletes our own post/comment.
    pub async fn delete(&mut self, circle_hex: &str, target_hex: &str) -> Result<String, SdkError> {
        let gid = self.gid(circle_hex)?;
        let ev = c::build(app::circle_event::Body::Delete(app::CircleDelete {
            target_id: hex::decode(target_hex).map_err(|e| SdkError::Invalid(e.to_string()))?,
        }))?;
        self.send(&gid, circle_hex, ev).await
    }

    /// Updates info.
    pub async fn set_info(
        &mut self,
        circle_hex: &str,
        name: &str,
        description: &str,
    ) -> Result<String, SdkError> {
        let gid = self.gid(circle_hex)?;
        let ev = c::build(app::circle_event::Body::Info(app::CircleInfo {
            name: name.to_owned(),
            description: description.to_owned(),
            avatar: None,
            history_visible_to_new_members: false,
        }))?;
        if let Some(mut info) = self
            .one
            .store
            .get::<CircleInfo>(NS_INFO, circle_hex.as_bytes())?
        {
            info.name = name.to_owned();
            info.description = description.to_owned();
            self.one.store.put(NS_INFO, circle_hex.as_bytes(), &info)?;
        }
        self.send(&gid, circle_hex, ev).await
    }

    async fn send(
        &mut self,
        gid: &[u8],
        circle_hex: &str,
        ev: app::CircleEvent,
    ) -> Result<String, SdkError> {
        self.one
            .send_app(gid, app::app_message::Body::CircleEvent(ev.clone()))
            .await?;
        let me = self.one.account.address().to_owned();
        self.record(circle_hex, &me, &ev)?;
        Ok(hex::encode(&ev.event_id))
    }

    fn record(
        &mut self,
        circle_hex: &str,
        author: &str,
        ev: &app::CircleEvent,
    ) -> Result<(), SdkError> {
        let key = format!(
            "{circle_hex}/{:016x}/{}",
            ev.at_ms,
            hex::encode(&ev.event_id)
        );
        self.one
            .store
            .put(NS_EVENTS, key.as_bytes(), &(author.to_owned(), ev.clone()))?;
        let tl = self
            .one
            .circles_state
            .timelines
            .entry(circle_hex.to_owned())
            .or_default();
        tl.apply(author, ev)?;
        Ok(())
    }

    /// Handles a Circle event from the network.
    pub(crate) fn handle_incoming(
        &mut self,
        r: &Received,
        appmsg: &app::AppMessage,
    ) -> Result<bool, SdkError> {
        let Some(app::app_message::Body::CircleEvent(ev)) = &appmsg.body else {
            return Ok(false);
        };
        // A circle event in a non-circle group: a member of a conversation
        // is trying to render a "circle"; we tag the group as a circle the
        // first time we see one from someone else only if we don't already
        // know the group as a conversation.
        match self.one.group_kind(&r.group_id).as_deref() {
            Some(group_kind::CIRCLE) => {}
            Some(group_kind::CONVERSATION) | Some(group_kind::SELF) | Some(group_kind::SPACE) => {
                return Ok(false)
            }
            _ => {
                self.one.set_group_kind(&r.group_id, group_kind::CIRCLE)?;
                let (name, _meta_members) = self
                    .one
                    .messaging
                    .conversations()
                    .into_iter()
                    .find(|(g, _, _)| *g == r.group_id)
                    .map(|(_, m, members)| (m.name, members))
                    .unwrap_or_default();
                let members = self
                    .one
                    .messaging
                    .conversations()
                    .into_iter()
                    .find(|(g, _, _)| *g == r.group_id)
                    .map(|(_, _, m)| m)
                    .unwrap_or_default();
                self.one.store.put(
                    NS_INFO,
                    r.group_id.as_bytes(),
                    &CircleInfo {
                        id: r.group_id.clone(),
                        name,
                        description: String::new(),
                        members,
                        since: hashgram_app::ids::now_secs(),
                        owner: false,
                    },
                )?;
            }
        }
        if let Some(app::circle_event::Body::Info(i)) = &ev.body {
            if let Some(mut info) = self
                .one
                .store
                .get::<CircleInfo>(NS_INFO, r.group_id.as_bytes())?
            {
                info.name = i.name.clone();
                info.description = i.description.clone();
                self.one.store.put(NS_INFO, r.group_id.as_bytes(), &info)?;
            }
        }
        self.record(&r.group_id, &r.sender, ev)?;
        Ok(true)
    }

    /// Lists circles.
    pub fn list(&self) -> Result<Vec<CircleInfo>, SdkError> {
        let mut v: Vec<CircleInfo> = self
            .one
            .store
            .scan::<CircleInfo>(NS_INFO)?
            .into_iter()
            .map(|(_, i)| i)
            .collect();
        // Refresh members from MLS.
        for c in &mut v {
            if let Some((_, _, m)) = self
                .one
                .messaging
                .conversations()
                .into_iter()
                .find(|(g, _, _)| *g == c.id)
            {
                c.members = m;
            }
        }
        v.sort_by_key(|c| c.name.to_lowercase());
        Ok(v)
    }

    fn timeline(&mut self, circle_hex: &str) -> Result<&c::Timeline, SdkError> {
        if !self.one.circles_state.timelines.contains_key(circle_hex) {
            let mut tl = c::Timeline::default();
            let prefix = format!("{circle_hex}/");
            for (k, (author, ev)) in self
                .one
                .store
                .scan::<(String, app::CircleEvent)>(NS_EVENTS)?
            {
                if k.starts_with(prefix.as_bytes()) {
                    let _ = tl.apply(&author, &ev);
                }
            }
            self.one
                .circles_state
                .timelines
                .insert(circle_hex.to_owned(), tl);
        }
        self.one
            .circles_state
            .timelines
            .get(circle_hex)
            .ok_or_else(|| SdkError::NotFound("circle".into()))
    }

    /// Posts page, newest first.
    pub fn posts(
        &mut self,
        circle_hex: &str,
        before_ms: u64,
        limit: usize,
    ) -> Result<Vec<c::Item>, SdkError> {
        Ok(self.timeline(circle_hex)?.posts(before_ms, limit))
    }

    /// Comments of a post.
    pub fn comments(&mut self, circle_hex: &str, post_hex: &str) -> Result<Vec<c::Item>, SdkError> {
        Ok(self.timeline(circle_hex)?.comments(post_hex))
    }

    /// Merged private timeline across all circles (for the Feed's
    /// "friends + circles" view), newest first.
    pub fn merged(
        &mut self,
        before_ms: u64,
        limit: usize,
    ) -> Result<Vec<(String, c::Item)>, SdkError> {
        let ids: Vec<String> = self.list()?.into_iter().map(|c| c.id).collect();
        let mut all = Vec::new();
        for id in ids {
            for it in self.timeline(&id)?.posts(before_ms, limit) {
                all.push((id.clone(), it));
            }
        }
        all.sort_by_key(|(_, it)| std::cmp::Reverse(it.at_ms));
        all.truncate(limit);
        Ok(all)
    }
}

//! Circles: private groups whose content is `CircleEvent`s inside one MLS
//! group. Membership is the MLS roster; there is no separate role log (a
//! Circle is flat: every member may post, comment, react and vote; the
//! creator and anyone they add may add members — the SDK exposes this as
//! "anyone in the circle can invite", which is the family/close-friends
//! model. Spaces are the structured alternative).
//!
//! What this module guarantees: bounds, dedup by event id, and a
//! deterministic timeline projection with reactions and votes folded in.

use std::collections::{BTreeMap, BTreeSet};

use crate::ids::{random_id, require_id, require_str, require_str_max};
use crate::pb;
use crate::version::{check_body, CIRCLE_VERSION};
use crate::AppError;

/// Post text bytes.
pub const MAX_TEXT: usize = 64 * 1024;
/// Media per post.
pub const MAX_MEDIA: usize = 20;
/// Poll options.
pub const MAX_POLL_OPTIONS: usize = 12;
/// Timeline rows kept in memory.
pub const MAX_TIMELINE: usize = 50_000;

/// Validates an event.
pub fn validate(e: &pb::CircleEvent) -> Result<(), AppError> {
    check_body("CircleEvent", e.version, CIRCLE_VERSION)?;
    require_id("event_id", &e.event_id)?;
    use pb::circle_event::Body as B;
    match &e.body {
        Some(B::Info(i)) => {
            require_str_max("name", &i.name, 128)?;
            require_str_max("description", &i.description, 4096)?;
        }
        Some(B::Post(p)) => {
            require_str_max("text", &p.text, MAX_TEXT)?;
            if p.media.len() > MAX_MEDIA {
                return Err(AppError::Invalid("too many media".into()));
            }
            for c in &p.drive_refs {
                crate::drive::validate_capability(c)?;
            }
            if let Some(poll) = &p.poll {
                require_str("poll.question", &poll.question, 512)?;
                if poll.options.len() < 2 || poll.options.len() > MAX_POLL_OPTIONS {
                    return Err(AppError::Invalid("poll needs 2..=12 options".into()));
                }
                for o in &poll.options {
                    require_str("poll option", o, 128)?;
                }
            }
            if p.text.is_empty()
                && p.media.is_empty()
                && p.drive_refs.is_empty()
                && p.poll.is_none()
            {
                return Err(AppError::Invalid("empty post".into()));
            }
        }
        Some(B::Comment(c)) => {
            require_id("post_id", &c.post_id)?;
            crate::ids::require_id_or_empty("reply_to", &c.reply_to)?;
            require_str("text", &c.text, MAX_TEXT)?;
        }
        Some(B::Reaction(r)) => {
            require_id("target_id", &r.target_id)?;
            require_str_max("reaction", &r.reaction, 32)?;
        }
        Some(B::Vote(v)) => {
            require_id("post_id", &v.post_id)?;
            if v.option_indexes.len() > MAX_POLL_OPTIONS {
                return Err(AppError::Invalid("too many votes".into()));
            }
        }
        Some(B::Delete(d)) => require_id("target_id", &d.target_id)?,
        None => return Err(AppError::Unsupported("unknown circle event body".into())),
    }
    Ok(())
}

/// Builds an event.
pub fn build(body: pb::circle_event::Body) -> Result<pb::CircleEvent, AppError> {
    let e = pb::CircleEvent {
        version: CIRCLE_VERSION,
        event_id: random_id()?,
        at_ms: crate::ids::now_ms(),
        body: Some(body),
    };
    validate(&e)?;
    Ok(e)
}

/// A projected post or comment.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Item {
    /// Hex id.
    pub id: String,
    /// "post" | "comment".
    pub kind: &'static str,
    /// Author address (from MLS).
    pub author: String,
    /// Time.
    pub at_ms: u64,
    /// Text.
    pub text: String,
    /// Parent post for comments (hex).
    pub post_id: String,
    /// Reply target for threaded comments (hex).
    pub reply_to: String,
    /// Media.
    pub media: Vec<pb::BlobRef>,
    /// Drive refs.
    pub drive_refs: Vec<pb::DriveCapability>,
    /// Poll, if any.
    pub poll: Option<pb::Poll>,
    /// reaction → count.
    pub reactions: BTreeMap<String, u32>,
    /// option index → votes.
    pub votes: BTreeMap<u32, u32>,
    /// Deleted by author (tombstone).
    pub deleted: bool,
}

/// The circle timeline projection.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Timeline {
    /// Name / description.
    pub info: pb::CircleInfo,
    /// Items in arrival order.
    pub items: Vec<Item>,
    /// (target, author) → reaction, so one reaction per author per target.
    reactions: BTreeMap<(String, String), String>,
    /// (post, author) → options.
    votes: BTreeMap<(String, String), Vec<u32>>,
    seen: BTreeSet<String>,
}

impl Timeline {
    /// Applies an event from `author` (the MLS sender address).
    pub fn apply(&mut self, author: &str, e: &pb::CircleEvent) -> Result<(), AppError> {
        validate(e)?;
        let id = hex::encode(&e.event_id);
        if !self.seen.insert(id.clone()) {
            return Ok(()); // duplicate delivery (several devices/stores)
        }
        use pb::circle_event::Body as B;
        match e.body.as_ref() {
            Some(B::Info(i)) => self.info = i.clone(),
            Some(B::Post(p)) => self.push(Item {
                id,
                kind: "post",
                author: author.into(),
                at_ms: e.at_ms,
                text: p.text.clone(),
                post_id: String::new(),
                reply_to: String::new(),
                media: p.media.clone(),
                drive_refs: p.drive_refs.clone(),
                poll: p.poll.clone(),
                reactions: BTreeMap::new(),
                votes: BTreeMap::new(),
                deleted: false,
            }),
            Some(B::Comment(c)) => self.push(Item {
                id,
                kind: "comment",
                author: author.into(),
                at_ms: e.at_ms,
                text: c.text.clone(),
                post_id: hex::encode(&c.post_id),
                reply_to: hex::encode(&c.reply_to),
                media: Vec::new(),
                drive_refs: Vec::new(),
                poll: None,
                reactions: BTreeMap::new(),
                votes: BTreeMap::new(),
                deleted: false,
            }),
            Some(B::Reaction(r)) => {
                let target = hex::encode(&r.target_id);
                let key = (target.clone(), author.to_owned());
                if let Some(prev) = self.reactions.remove(&key) {
                    if let Some(item) = self.items.iter_mut().find(|i| i.id == target) {
                        if let Some(n) = item.reactions.get_mut(&prev) {
                            *n = n.saturating_sub(1);
                            if *n == 0 {
                                item.reactions.remove(&prev);
                            }
                        }
                    }
                }
                if !r.reaction.is_empty() {
                    if let Some(item) = self.items.iter_mut().find(|i| i.id == target) {
                        *item.reactions.entry(r.reaction.clone()).or_insert(0) += 1;
                        self.reactions.insert(key, r.reaction.clone());
                    }
                }
            }
            Some(B::Vote(v)) => {
                let post = hex::encode(&v.post_id);
                let key = (post.clone(), author.to_owned());
                let Some(item) = self
                    .items
                    .iter_mut()
                    .find(|i| i.id == post && i.kind == "post")
                else {
                    return Ok(());
                };
                let Some(poll) = &item.poll else {
                    return Ok(());
                };
                let n = poll.options.len() as u32;
                let multi = poll.multiple_choice;
                if poll.closes_at_ms != 0 && e.at_ms > poll.closes_at_ms {
                    return Ok(());
                }
                if let Some(prev) = self.votes.remove(&key) {
                    for o in prev {
                        if let Some(c) = item.votes.get_mut(&o) {
                            *c = c.saturating_sub(1);
                        }
                    }
                }
                let mut chosen: Vec<u32> = v
                    .option_indexes
                    .iter()
                    .copied()
                    .filter(|o| *o < n)
                    .collect();
                chosen.sort_unstable();
                chosen.dedup();
                if !multi {
                    chosen.truncate(1);
                }
                for o in &chosen {
                    *item.votes.entry(*o).or_insert(0) += 1;
                }
                self.votes.insert(key, chosen);
            }
            Some(B::Delete(d)) => {
                let target = hex::encode(&d.target_id);
                if let Some(item) = self.items.iter_mut().find(|i| i.id == target) {
                    if item.author == author {
                        item.deleted = true;
                        item.text.clear();
                        item.media.clear();
                        item.drive_refs.clear();
                    }
                }
            }
            None => {}
        }
        Ok(())
    }

    fn push(&mut self, item: Item) {
        self.items.push(item);
        if self.items.len() > MAX_TIMELINE {
            let excess = self.items.len() - MAX_TIMELINE;
            self.items.drain(..excess);
        }
    }

    /// Posts newest first (comments excluded), before `before_ms` (0 = now).
    #[must_use]
    pub fn posts(&self, before_ms: u64, limit: usize) -> Vec<Item> {
        let mut v: Vec<Item> = self
            .items
            .iter()
            .filter(|i| i.kind == "post" && (before_ms == 0 || i.at_ms < before_ms))
            .cloned()
            .collect();
        v.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| b.id.cmp(&a.id)));
        v.truncate(limit);
        v
    }

    /// Comments of a post, oldest first.
    #[must_use]
    pub fn comments(&self, post_id_hex: &str) -> Vec<Item> {
        let mut v: Vec<Item> = self
            .items
            .iter()
            .filter(|i| i.kind == "comment" && i.post_id == post_id_hex)
            .cloned()
            .collect();
        v.sort_by(|a, b| a.at_ms.cmp(&b.at_ms).then_with(|| a.id.cmp(&b.id)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_projection() {
        let mut t = Timeline::default();
        let post = build(pb::circle_event::Body::Post(pb::CirclePost {
            text: "hello".into(),
            poll: Some(pb::Poll {
                question: "pizza?".into(),
                options: vec!["yes".into(), "no".into()],
                ..Default::default()
            }),
            ..Default::default()
        }))
        .unwrap();
        t.apply("hash1a", &post).unwrap();
        t.apply("hash1a", &post).unwrap(); // duplicate ignored
        assert_eq!(t.items.len(), 1);
        let pid = post.event_id.clone();
        let react = |r: &str| {
            build(pb::circle_event::Body::Reaction(pb::CircleReaction {
                target_id: pid.clone(),
                reaction: r.into(),
            }))
            .unwrap()
        };
        t.apply("hash1b", &react("❤")).unwrap();
        t.apply("hash1b", &react("👍")).unwrap(); // replaces
        t.apply("hash1c", &react("👍")).unwrap();
        let p = &t.items[0];
        assert_eq!(p.reactions.get("👍"), Some(&2));
        assert!(!p.reactions.contains_key("❤"));
        let vote = |o: Vec<u32>| {
            build(pb::circle_event::Body::Vote(pb::CircleVote {
                post_id: pid.clone(),
                option_indexes: o,
            }))
            .unwrap()
        };
        t.apply("hash1b", &vote(vec![0])).unwrap();
        t.apply("hash1b", &vote(vec![1, 1, 9])).unwrap(); // re-vote, dedup, out of range dropped, single choice
        t.apply("hash1c", &vote(vec![1])).unwrap();
        let p = &t.items[0];
        assert_eq!(p.votes.get(&0).copied().unwrap_or(0), 0);
        assert_eq!(p.votes.get(&1), Some(&2));
        let c = build(pb::circle_event::Body::Comment(pb::CircleComment {
            post_id: pid.clone(),
            text: "nice".into(),
            ..Default::default()
        }))
        .unwrap();
        t.apply("hash1c", &c).unwrap();
        assert_eq!(t.comments(&hex::encode(&pid)).len(), 1);
        // Only the author may delete.
        let del = build(pb::circle_event::Body::Delete(pb::CircleDelete {
            target_id: pid.clone(),
        }))
        .unwrap();
        t.apply("hash1b", &del).unwrap();
        assert!(!t.items[0].deleted);
        let del2 = build(pb::circle_event::Body::Delete(pb::CircleDelete {
            target_id: pid.clone(),
        }))
        .unwrap();
        t.apply("hash1a", &del2).unwrap();
        assert!(t.items[0].deleted);
        assert_eq!(t.posts(0, 10).len(), 1);
    }

    #[test]
    fn validation() {
        assert!(build(pb::circle_event::Body::Post(pb::CirclePost::default())).is_err());
        assert!(build(pb::circle_event::Body::Post(pb::CirclePost {
            poll: Some(pb::Poll {
                question: "q".into(),
                options: vec!["one".into()],
                ..Default::default()
            }),
            ..Default::default()
        }))
        .is_err());
    }
}

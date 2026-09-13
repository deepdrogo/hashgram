//! Spaces: a signed, hash-chained event log replayed by every member into
//! one role table, plus the rules that decide who may do what.
//!
//! A Space is an MLS group (confidentiality, membership of devices) plus
//! this log (authorisation, ordering, auditability). MLS alone says *who is
//! in the room*; the log says *who is allowed to change the room*. Every
//! member applies the same rules to the same events, so a member who sends
//! an event they may not send has it rejected by everyone else and it never
//! enters anyone's state.
//!
//! Ordering: events are applied in `(at_ms, event_id)` order. Because
//! clocks drift, a reader also keeps events it could not apply yet (e.g.
//! a post from someone whose `MemberAdd` has not arrived) in a bounded
//! pending set and retries after every new event.

use std::collections::{BTreeMap, BTreeSet};

use hashgram_net::{CanonicalBuf, NetworkIdentity};
use hashgram_proto::keys::Ed25519Signer;
use prost::Message;

use crate::ids::{
    now_ms, random_id, require_address, require_id, require_id_or_empty, require_str,
    require_str_max,
};
use crate::pb;
use crate::signing::{self, AppPurpose};
use crate::version::{check_body, SPACE_VERSION};
use crate::AppError;

/// Bounds.
pub const MAX_MEMBERS: usize = 10_000;
/// Name bytes.
pub const MAX_NAME: usize = 128;
/// Description / announcement text bytes.
pub const MAX_TEXT: usize = 64 * 1024;
/// Pending (not yet applicable) events kept.
pub const MAX_PENDING: usize = 4096;
/// Posts/comments/announcements kept in state (older are dropped from the
/// in-memory state but remain in the local store).
pub const MAX_CONTENT: usize = 50_000;

/// Role ordering helpers.
fn role(r: i32) -> pb::SpaceRole {
    pb::SpaceRole::try_from(r).unwrap_or(pb::SpaceRole::None)
}

fn rank(r: pb::SpaceRole) -> u8 {
    match r {
        pb::SpaceRole::None => 0,
        pb::SpaceRole::Guest => 1,
        pb::SpaceRole::Member => 2,
        pb::SpaceRole::Admin => 3,
        pb::SpaceRole::Owner => 4,
    }
}

/// The canonical bytes a Space event signature commits to: every field but
/// the signature, in tag order.
pub fn canonical_payload(e: &pb::SpaceEvent) -> Result<Vec<u8>, AppError> {
    let body = body_bytes(e);
    Ok(CanonicalBuf::new(256)
        .u32(e.version)
        .bytes("space_id", &e.space_id)
        .bytes("event_id", &e.event_id)
        .string("actor", &e.actor)
        .bytes("device_pubkey", &e.device_pubkey)
        .u64(e.sequence)
        .bytes("parent_id", &e.parent_id)
        .u64(e.at_ms)
        .u32(body_tag(e))
        .bytes("body", &body)
        .finish()?)
}

fn body_tag(e: &pb::SpaceEvent) -> u32 {
    use pb::space_event::Body as B;
    match &e.body {
        Some(B::Create(_)) => 10,
        Some(B::Info(_)) => 11,
        Some(B::MemberAdd(_)) => 12,
        Some(B::MemberRemove(_)) => 13,
        Some(B::RoleChange(_)) => 14,
        Some(B::Announcement(_)) => 15,
        Some(B::DriveShare(_)) => 16,
        Some(B::DriveUnshare(_)) => 17,
        Some(B::Post(_)) => 18,
        Some(B::Comment(_)) => 19,
        None => 0,
    }
}

fn body_bytes(e: &pb::SpaceEvent) -> Vec<u8> {
    use pb::space_event::Body as B;
    match &e.body {
        Some(B::Create(b)) => b.encode_to_vec(),
        Some(B::Info(b)) => b.encode_to_vec(),
        Some(B::MemberAdd(b)) => b.encode_to_vec(),
        Some(B::MemberRemove(b)) => b.encode_to_vec(),
        Some(B::RoleChange(b)) => b.encode_to_vec(),
        Some(B::Announcement(b)) => b.encode_to_vec(),
        Some(B::DriveShare(b)) => b.encode_to_vec(),
        Some(B::DriveUnshare(b)) => b.encode_to_vec(),
        Some(B::Post(b)) => b.encode_to_vec(),
        Some(B::Comment(b)) => b.encode_to_vec(),
        None => Vec::new(),
    }
}

/// Signs an event in place.
pub fn sign(
    network: &NetworkIdentity,
    device: &Ed25519Signer,
    e: &mut pb::SpaceEvent,
) -> Result<(), AppError> {
    e.device_pubkey = device.public_key().to_vec();
    let payload = canonical_payload(e)?;
    e.signature = signing::sign(network, AppPurpose::SpaceEvent, device, &payload).to_vec();
    Ok(())
}

/// Verifies the signature and structure (not the authorisation; that is
/// [`State::apply`]).
pub fn verify(network: &NetworkIdentity, e: &pb::SpaceEvent) -> Result<(), AppError> {
    validate(e)?;
    let payload = canonical_payload(e)?;
    signing::verify(
        network,
        AppPurpose::SpaceEvent,
        &e.device_pubkey,
        &payload,
        &e.signature,
    )
    .map_err(|err| AppError::Unauthorised(format!("space event signature: {err}")))
}

/// Structural validation.
pub fn validate(e: &pb::SpaceEvent) -> Result<(), AppError> {
    check_body("SpaceEvent", e.version, SPACE_VERSION)?;
    require_id("space_id", &e.space_id)?;
    require_id("event_id", &e.event_id)?;
    require_address("actor", &e.actor)?;
    if e.device_pubkey.len() != 32 {
        return Err(AppError::Invalid("device_pubkey length".into()));
    }
    require_id_or_empty("parent_id", &e.parent_id)?;
    if e.signature.len() != 64 {
        return Err(AppError::Invalid("signature length".into()));
    }
    use pb::space_event::Body as B;
    match &e.body {
        Some(B::Create(b)) => {
            require_str("name", &b.name, MAX_NAME)?;
            require_str_max("description", &b.description, MAX_TEXT)?;
            require_address("owner", &b.owner)?;
            if e.space_id != e.event_id {
                return Err(AppError::Invalid(
                    "Create must have space_id == event_id".into(),
                ));
            }
        }
        Some(B::Info(b)) => {
            require_str_max("name", &b.name, MAX_NAME)?;
            require_str_max("description", &b.description, MAX_TEXT)?;
        }
        Some(B::MemberAdd(b)) => {
            require_address("address", &b.address)?;
            if matches!(role(b.role), pb::SpaceRole::None | pb::SpaceRole::Owner) {
                return Err(AppError::Invalid(
                    "MemberAdd role must be Guest, Member or Admin".into(),
                ));
            }
        }
        Some(B::MemberRemove(b)) => {
            require_address("address", &b.address)?;
            require_str_max("reason", &b.reason, 512)?;
        }
        Some(B::RoleChange(b)) => {
            require_address("address", &b.address)?;
            if role(b.role) == pb::SpaceRole::None {
                return Err(AppError::Invalid(
                    "RoleChange to None; use MemberRemove".into(),
                ));
            }
        }
        Some(B::Announcement(b)) => {
            require_str("title", &b.title, MAX_NAME)?;
            require_str_max("text", &b.text, MAX_TEXT)?;
            for c in &b.attachments {
                crate::drive::validate_capability(c)?;
            }
        }
        Some(B::DriveShare(b)) => {
            let c = b
                .capability
                .as_ref()
                .ok_or_else(|| AppError::Invalid("capability missing".into()))?;
            crate::drive::validate_capability(c)?;
            require_str_max("path", &b.path, 1024)?;
        }
        Some(B::DriveUnshare(b)) => require_id("share_id", &b.share_id)?,
        Some(B::Post(b)) => {
            require_str_max("text", &b.text, MAX_TEXT)?;
            if b.text.is_empty() && b.media.is_empty() && b.drive_refs.is_empty() {
                return Err(AppError::Invalid("empty post".into()));
            }
            for c in &b.drive_refs {
                crate::drive::validate_capability(c)?;
            }
        }
        Some(B::Comment(b)) => {
            require_id("post_id", &b.post_id)?;
            require_str("text", &b.text, MAX_TEXT)?;
        }
        None => return Err(AppError::Unsupported("unknown space event body".into())),
    }
    Ok(())
}

/// Builds an unsigned event for the caller to sign.
pub fn build(
    space_id: &[u8],
    actor: &str,
    sequence: u64,
    parent_id: &[u8],
    body: pb::space_event::Body,
) -> Result<pb::SpaceEvent, AppError> {
    let event_id = if matches!(body, pb::space_event::Body::Create(_)) {
        space_id.to_vec()
    } else {
        random_id()?
    };
    Ok(pb::SpaceEvent {
        version: SPACE_VERSION,
        space_id: space_id.to_vec(),
        event_id,
        actor: actor.to_owned(),
        device_pubkey: Vec::new(),
        sequence,
        parent_id: parent_id.to_vec(),
        at_ms: now_ms(),
        body: Some(body),
        signature: Vec::new(),
    })
}

/// A member row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Member {
    /// Address.
    pub address: String,
    /// Role.
    pub role: i32,
    /// When they joined.
    pub since_ms: u64,
}

/// A content row (post, comment or announcement).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Content {
    /// Hex event id.
    pub id: String,
    /// "post" | "comment" | "announcement".
    pub kind: &'static str,
    /// Actor.
    pub actor: String,
    /// Time.
    pub at_ms: u64,
    /// Title (announcements).
    pub title: String,
    /// Text.
    pub text: String,
    /// Post id for comments (hex).
    pub post_id: String,
    /// Drive capabilities attached.
    pub drive_refs: Vec<pb::DriveCapability>,
    /// Media attached.
    pub media: Vec<pb::BlobRef>,
}

/// A shared drive entry inside the Space.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SharedEntry {
    /// Capability.
    pub capability: pb::DriveCapability,
    /// Path inside the Space drive.
    pub path: String,
    /// Who shared it.
    pub by: String,
    /// When.
    pub at_ms: u64,
}

/// Why an event was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejected {
    /// Signature or structure.
    #[error("{0}")]
    Invalid(AppError),
    /// The actor may not do this.
    #[error("actor {actor} ({role:?}) may not {action}")]
    Forbidden {
        /// Actor.
        actor: String,
        /// Their role.
        role: pb::SpaceRole,
        /// What they tried.
        action: &'static str,
    },
    /// Replay or out-of-order sequence.
    #[error("sequence {got} is not above {last} for {actor}")]
    Sequence {
        /// Actor.
        actor: String,
        /// Sequence received.
        got: u64,
        /// Last accepted.
        last: u64,
    },
    /// Already applied.
    #[error("duplicate event")]
    Duplicate,
    /// Depends on state not yet seen (kept pending).
    #[error("not applicable yet: {0}")]
    Pending(&'static str),
    /// For the wrong space.
    #[error("event is for another space")]
    WrongSpace,
}

/// The replayed state of one Space.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct State {
    /// Space id, hex.
    pub space_id: String,
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Avatar.
    pub avatar: Option<pb::BlobRef>,
    /// Members by address.
    pub members: BTreeMap<String, Member>,
    /// Content newest last.
    pub content: Vec<Content>,
    /// Shared drive entries by share id (hex).
    pub drive: BTreeMap<String, SharedEntry>,
    /// Last accepted sequence per actor.
    pub sequences: BTreeMap<String, u64>,
    /// Applied event ids (hex).
    pub applied: BTreeSet<String>,
    /// Last applied event id (for `parent_id`).
    pub head: String,
    /// Created.
    pub created_at_ms: u64,
    #[serde(skip)]
    pending: Vec<pb::SpaceEvent>,
}

impl State {
    /// Empty state for a space id.
    #[must_use]
    pub fn new(space_id: &[u8]) -> Self {
        Self {
            space_id: hex::encode(space_id),
            ..Default::default()
        }
    }

    /// Role of an address.
    #[must_use]
    pub fn role_of(&self, address: &str) -> pb::SpaceRole {
        self.members
            .get(address)
            .map(|m| role(m.role))
            .unwrap_or(pb::SpaceRole::None)
    }

    /// Whether the space has been created (saw its Create event).
    #[must_use]
    pub fn exists(&self) -> bool {
        self.created_at_ms > 0
    }

    /// Next sequence for an actor.
    #[must_use]
    pub fn next_sequence(&self, actor: &str) -> u64 {
        self.sequences.get(actor).map(|s| s + 1).unwrap_or(1)
    }

    /// Head event id bytes.
    #[must_use]
    pub fn head_id(&self) -> Vec<u8> {
        hex::decode(&self.head).unwrap_or_default()
    }

    /// Verifies and applies a received event, then retries pending ones.
    /// The MLS sender address must be passed so an actor cannot claim to be
    /// someone else: `e.actor` must equal it.
    pub fn apply(
        &mut self,
        network: &NetworkIdentity,
        e: &pb::SpaceEvent,
        mls_sender: &str,
    ) -> Result<(), Rejected> {
        if hex::encode(&e.space_id) != self.space_id {
            return Err(Rejected::WrongSpace);
        }
        verify(network, e).map_err(Rejected::Invalid)?;
        if e.actor != mls_sender {
            return Err(Rejected::Forbidden {
                actor: e.actor.clone(),
                role: pb::SpaceRole::None,
                action: "act as another address",
            });
        }
        let r = self.apply_verified(e);
        if matches!(r, Err(Rejected::Pending(_))) && self.pending.len() < MAX_PENDING {
            self.pending.push(e.clone());
        }
        if r.is_ok() {
            self.retry_pending();
        }
        r
    }

    fn retry_pending(&mut self) {
        let mut progress = true;
        while progress {
            progress = false;
            let mut pending = std::mem::take(&mut self.pending);
            // Canonical order: store nodes return a page in arrival order
            // and two events of one second sort by id, so an actor's later
            // event can reach us before an earlier one. Retrying in
            // (at_ms, sequence, id) order keeps per-actor sequences
            // monotonic instead of rejecting the earlier event as a replay.
            pending.sort_by(|a, b| {
                (a.at_ms, a.sequence, &a.event_id).cmp(&(b.at_ms, b.sequence, &b.event_id))
            });
            for e in pending {
                match self.apply_verified(&e) {
                    Ok(()) => progress = true,
                    Err(Rejected::Pending(_)) => self.pending.push(e),
                    Err(_) => {} // now definitively rejected
                }
            }
        }
    }

    fn apply_verified(&mut self, e: &pb::SpaceEvent) -> Result<(), Rejected> {
        use pb::space_event::Body as B;
        let id_hex = hex::encode(&e.event_id);
        if self.applied.contains(&id_hex) {
            return Err(Rejected::Duplicate);
        }
        let last = self.sequences.get(&e.actor).copied().unwrap_or(0);
        if e.sequence <= last {
            return Err(Rejected::Sequence {
                actor: e.actor.clone(),
                got: e.sequence,
                last,
            });
        }
        // A gap means an earlier event of this actor has not reached us
        // yet (delivery is per store node and unordered across a page).
        // Applying the later one now would make the earlier one look like
        // a replay forever; keep it pending until the gap fills. The
        // Create is the exception: it is the first event and has no
        // predecessor we could be missing.
        if e.sequence > last + 1 && !matches!(e.body, Some(B::Create(_))) {
            return Err(Rejected::Pending("earlier events from this actor not seen yet"));
        }
        let actor_role = self.role_of(&e.actor);
        let forbid = |action: &'static str| Rejected::Forbidden {
            actor: e.actor.clone(),
            role: actor_role,
            action,
        };
        match &e.body {
            Some(B::Create(b)) => {
                if self.exists() {
                    return Err(forbid("create an existing space"));
                }
                if b.owner != e.actor {
                    return Err(forbid("create a space owned by someone else"));
                }
                self.name = b.name.clone();
                self.description = b.description.clone();
                self.created_at_ms = e.at_ms;
                self.members.insert(
                    e.actor.clone(),
                    Member {
                        address: e.actor.clone(),
                        role: pb::SpaceRole::Owner as i32,
                        since_ms: e.at_ms,
                    },
                );
            }
            _ if !self.exists() => return Err(Rejected::Pending("space not created yet")),
            Some(B::Info(b)) => {
                if rank(actor_role) < rank(pb::SpaceRole::Admin) {
                    return Err(forbid("update space info"));
                }
                if !b.name.is_empty() {
                    self.name = b.name.clone();
                }
                self.description = b.description.clone();
                if b.avatar.is_some() {
                    self.avatar = b.avatar.clone();
                }
            }
            Some(B::MemberAdd(b)) => {
                if rank(actor_role) < rank(pb::SpaceRole::Admin) {
                    return Err(forbid("add members"));
                }
                let new_role = role(b.role);
                if rank(new_role) > rank(actor_role)
                    || (new_role == pb::SpaceRole::Admin && actor_role != pb::SpaceRole::Owner)
                {
                    return Err(forbid("add a member above own rank / add admins"));
                }
                if self.members.contains_key(&b.address) {
                    return Err(forbid("add an existing member"));
                }
                if self.members.len() >= MAX_MEMBERS {
                    return Err(Rejected::Invalid(AppError::Invalid("space is full".into())));
                }
                self.members.insert(
                    b.address.clone(),
                    Member {
                        address: b.address.clone(),
                        role: b.role,
                        since_ms: e.at_ms,
                    },
                );
            }
            Some(B::MemberRemove(b)) => {
                let target = self.role_of(&b.address);
                if target == pb::SpaceRole::None {
                    return Err(Rejected::Pending("remove of an unknown member"));
                }
                let self_leave = b.address == e.actor && actor_role != pb::SpaceRole::Owner;
                if !self_leave
                    && (rank(actor_role) < rank(pb::SpaceRole::Admin)
                        || rank(target) >= rank(actor_role))
                {
                    return Err(forbid("remove this member"));
                }
                self.members.remove(&b.address);
            }
            Some(B::RoleChange(b)) => {
                let target = self.role_of(&b.address);
                if target == pb::SpaceRole::None {
                    return Err(Rejected::Pending("role change of an unknown member"));
                }
                let new_role = role(b.role);
                let allowed = match actor_role {
                    pb::SpaceRole::Owner => {
                        // Owner may promote anyone, including transferring
                        // ownership (then the previous owner becomes admin).
                        true
                    }
                    pb::SpaceRole::Admin => {
                        rank(target) < rank(pb::SpaceRole::Admin)
                            && rank(new_role) <= rank(pb::SpaceRole::Member)
                    }
                    _ => false,
                };
                if !allowed || b.address == e.actor && actor_role != pb::SpaceRole::Owner {
                    return Err(forbid("change this role"));
                }
                if new_role == pb::SpaceRole::Owner {
                    if let Some(prev) = self.members.get_mut(&e.actor) {
                        prev.role = pb::SpaceRole::Admin as i32;
                    }
                }
                if let Some(m) = self.members.get_mut(&b.address) {
                    m.role = b.role;
                }
            }
            Some(B::Announcement(b)) => {
                if rank(actor_role) < rank(pb::SpaceRole::Admin) {
                    return Err(forbid("announce"));
                }
                self.push_content(Content {
                    id: id_hex.clone(),
                    kind: "announcement",
                    actor: e.actor.clone(),
                    at_ms: e.at_ms,
                    title: b.title.clone(),
                    text: b.text.clone(),
                    post_id: String::new(),
                    drive_refs: b.attachments.clone(),
                    media: Vec::new(),
                });
            }
            Some(B::DriveShare(b)) => {
                if rank(actor_role) < rank(pb::SpaceRole::Member) {
                    return Err(forbid("share to the space drive"));
                }
                let cap = b.capability.clone().unwrap_or_default();
                self.drive.insert(
                    hex::encode(&cap.share_id),
                    SharedEntry {
                        capability: cap,
                        path: b.path.clone(),
                        by: e.actor.clone(),
                        at_ms: e.at_ms,
                    },
                );
            }
            Some(B::DriveUnshare(b)) => {
                let key = hex::encode(&b.share_id);
                let Some(entry) = self.drive.get(&key) else {
                    return Err(Rejected::Pending("unshare of an unknown share"));
                };
                if entry.by != e.actor && rank(actor_role) < rank(pb::SpaceRole::Admin) {
                    return Err(forbid("unshare someone else's file"));
                }
                self.drive.remove(&key);
            }
            Some(B::Post(b)) => {
                if rank(actor_role) < rank(pb::SpaceRole::Member) {
                    return Err(forbid("post"));
                }
                self.push_content(Content {
                    id: id_hex.clone(),
                    kind: "post",
                    actor: e.actor.clone(),
                    at_ms: e.at_ms,
                    title: String::new(),
                    text: b.text.clone(),
                    post_id: String::new(),
                    drive_refs: b.drive_refs.clone(),
                    media: b.media.clone(),
                });
            }
            Some(B::Comment(b)) => {
                if rank(actor_role) < rank(pb::SpaceRole::Member) {
                    return Err(forbid("comment"));
                }
                let pid = hex::encode(&b.post_id);
                if !self
                    .content
                    .iter()
                    .any(|c| c.id == pid && c.kind != "comment")
                {
                    return Err(Rejected::Pending("comment on an unknown post"));
                }
                self.push_content(Content {
                    id: id_hex.clone(),
                    kind: "comment",
                    actor: e.actor.clone(),
                    at_ms: e.at_ms,
                    title: String::new(),
                    text: b.text.clone(),
                    post_id: pid,
                    drive_refs: Vec::new(),
                    media: Vec::new(),
                });
            }
            None => return Err(Rejected::Invalid(AppError::Unsupported("body".into()))),
        }
        self.sequences.insert(e.actor.clone(), e.sequence);
        self.applied.insert(id_hex.clone());
        self.head = id_hex;
        Ok(())
    }

    fn push_content(&mut self, c: Content) {
        self.content.push(c);
        if self.content.len() > MAX_CONTENT {
            let excess = self.content.len() - MAX_CONTENT;
            self.content.drain(..excess);
        }
    }

    /// Members sorted by rank then address.
    #[must_use]
    pub fn members_sorted(&self) -> Vec<Member> {
        let mut v: Vec<Member> = self.members.values().cloned().collect();
        v.sort_by(|a, b| {
            rank(role(b.role))
                .cmp(&rank(role(a.role)))
                .then_with(|| a.address.cmp(&b.address))
        });
        v
    }

    /// Content page, newest first, before `before_ms` (0 = now).
    #[must_use]
    pub fn content_page(&self, before_ms: u64, limit: usize) -> Vec<Content> {
        self.content
            .iter()
            .rev()
            .filter(|c| before_ms == 0 || c.at_ms < before_ms)
            .take(limit)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "hash1e0tl2hff03hu4g3sawcjqa2p9tc4uh24e4vfl5";
    const ALICE: &str = "hash178njz3gft77798ssh6lrqxh2hskw5dm68f2uh5";
    const BOB: &str = "hash1znsxwr000000000000000000000000000000000";

    fn net() -> NetworkIdentity {
        NetworkIdentity::devnet("0".repeat(64))
    }

    struct Actor {
        addr: &'static str,
        key: Ed25519Signer,
    }

    fn actor(addr: &'static str, seed: u8) -> Actor {
        Actor {
            addr,
            key: Ed25519Signer::from_secret([seed; 32]),
        }
    }

    fn send(st: &mut State, a: &Actor, body: pb::space_event::Body) -> Result<(), Rejected> {
        let sid = hex::decode(&st.space_id).unwrap();
        let mut e = build(&sid, a.addr, st.next_sequence(a.addr), &st.head_id(), body).unwrap();
        sign(&net(), &a.key, &mut e).unwrap();
        st.apply(&net(), &e, a.addr)
    }

    fn create(owner: &Actor) -> State {
        let sid = random_id().unwrap();
        let mut st = State::new(&sid);
        send(
            &mut st,
            owner,
            pb::space_event::Body::Create(pb::SpaceCreate {
                name: "Team".into(),
                description: String::new(),
                owner: owner.addr.into(),
            }),
        )
        .unwrap();
        st
    }

    #[test]
    fn roles_are_enforced() {
        let owner = actor(OWNER, 1);
        let alice = actor(ALICE, 2);
        let bob = actor(BOB, 3);
        let mut st = create(&owner);
        assert_eq!(st.role_of(OWNER), pb::SpaceRole::Owner);
        // Non-member cannot post.
        assert!(matches!(
            send(
                &mut st,
                &alice,
                pb::space_event::Body::Post(pb::SpacePost {
                    text: "hi".into(),
                    ..Default::default()
                })
            ),
            Err(Rejected::Forbidden { .. })
        ));
        // Owner adds alice as admin, bob as guest.
        send(
            &mut st,
            &owner,
            pb::space_event::Body::MemberAdd(pb::SpaceMemberAdd {
                address: ALICE.into(),
                role: pb::SpaceRole::Admin as i32,
            }),
        )
        .unwrap();
        send(
            &mut st,
            &owner,
            pb::space_event::Body::MemberAdd(pb::SpaceMemberAdd {
                address: BOB.into(),
                role: pb::SpaceRole::Guest as i32,
            }),
        )
        .unwrap();
        // Guest cannot post; member can.
        assert!(matches!(
            send(
                &mut st,
                &bob,
                pb::space_event::Body::Post(pb::SpacePost {
                    text: "hi".into(),
                    ..Default::default()
                })
            ),
            Err(Rejected::Forbidden { .. })
        ));
        send(
            &mut st,
            &alice,
            pb::space_event::Body::RoleChange(pb::SpaceRoleChange {
                address: BOB.into(),
                role: pb::SpaceRole::Member as i32,
            }),
        )
        .unwrap();
        send(
            &mut st,
            &bob,
            pb::space_event::Body::Post(pb::SpacePost {
                text: "hi".into(),
                ..Default::default()
            }),
        )
        .unwrap();
        assert_eq!(st.content.len(), 1);
        // Admin cannot add another admin or remove the owner.
        assert!(matches!(
            send(
                &mut st,
                &alice,
                pb::space_event::Body::MemberAdd(pb::SpaceMemberAdd {
                    address: "hash1newadm00000000000000000000000000000000".into(),
                    role: pb::SpaceRole::Admin as i32
                })
            ),
            Err(Rejected::Forbidden { .. })
        ));
        assert!(matches!(
            send(
                &mut st,
                &alice,
                pb::space_event::Body::MemberRemove(pb::SpaceMemberRemove {
                    address: OWNER.into(),
                    reason: String::new()
                })
            ),
            Err(Rejected::Forbidden { .. })
        ));
        // Admin can announce; member cannot.
        send(
            &mut st,
            &alice,
            pb::space_event::Body::Announcement(pb::SpaceAnnouncement {
                title: "T".into(),
                text: "x".into(),
                attachments: vec![],
            }),
        )
        .unwrap();
        assert!(matches!(
            send(
                &mut st,
                &bob,
                pb::space_event::Body::Announcement(pb::SpaceAnnouncement {
                    title: "T".into(),
                    text: "x".into(),
                    attachments: vec![]
                })
            ),
            Err(Rejected::Forbidden { .. })
        ));
        // Bob leaves on his own.
        send(
            &mut st,
            &bob,
            pb::space_event::Body::MemberRemove(pb::SpaceMemberRemove {
                address: BOB.into(),
                reason: String::new(),
            }),
        )
        .unwrap();
        assert_eq!(st.role_of(BOB), pb::SpaceRole::None);
        // Ownership transfer.
        send(
            &mut st,
            &owner,
            pb::space_event::Body::RoleChange(pb::SpaceRoleChange {
                address: ALICE.into(),
                role: pb::SpaceRole::Owner as i32,
            }),
        )
        .unwrap();
        assert_eq!(st.role_of(ALICE), pb::SpaceRole::Owner);
        assert_eq!(st.role_of(OWNER), pb::SpaceRole::Admin);
    }

    #[test]
    fn replay_impersonation_and_tamper_rejected() {
        let owner = actor(OWNER, 1);
        let alice = actor(ALICE, 2);
        let mut st = create(&owner);
        let sid = hex::decode(&st.space_id).unwrap();
        let mut e = build(
            &sid,
            OWNER,
            st.next_sequence(OWNER),
            &st.head_id(),
            pb::space_event::Body::Info(pb::SpaceInfoUpdate {
                name: "New".into(),
                ..Default::default()
            }),
        )
        .unwrap();
        sign(&net(), &owner.key, &mut e).unwrap();
        st.apply(&net(), &e, OWNER).unwrap();
        // Replay.
        assert!(matches!(
            st.apply(&net(), &e, OWNER),
            Err(Rejected::Duplicate)
        ));
        // Same sequence, different id.
        let mut e2 = e.clone();
        e2.event_id = random_id().unwrap();
        sign(&net(), &owner.key, &mut e2).unwrap();
        assert!(matches!(
            st.apply(&net(), &e2, OWNER),
            Err(Rejected::Sequence { .. })
        ));
        // Alice signs an event claiming to be the owner: MLS sender mismatch.
        let mut e3 = build(
            &sid,
            OWNER,
            10,
            &[],
            pb::space_event::Body::Info(pb::SpaceInfoUpdate {
                name: "Evil".into(),
                ..Default::default()
            }),
        )
        .unwrap();
        sign(&net(), &alice.key, &mut e3).unwrap();
        assert!(matches!(
            st.apply(&net(), &e3, ALICE),
            Err(Rejected::Forbidden { .. })
        ));
        // Tampered body.
        let mut e4 = e.clone();
        e4.event_id = random_id().unwrap();
        e4.sequence = 10;
        sign(&net(), &owner.key, &mut e4).unwrap();
        if let Some(pb::space_event::Body::Info(i)) = &mut e4.body {
            i.name = "Tampered".into();
        }
        assert!(matches!(
            st.apply(&net(), &e4, OWNER),
            Err(Rejected::Invalid(_))
        ));
        assert_eq!(st.name, "New");
    }

    #[test]
    fn out_of_order_events_become_applicable() {
        let owner = actor(OWNER, 1);
        let alice = actor(ALICE, 2);
        let mut st = create(&owner);
        let sid = hex::decode(&st.space_id).unwrap();
        // Alice's post arrives before her MemberAdd.
        let mut post = build(
            &sid,
            ALICE,
            1,
            &[],
            pb::space_event::Body::Post(pb::SpacePost {
                text: "early".into(),
                ..Default::default()
            }),
        )
        .unwrap();
        sign(&net(), &alice.key, &mut post).unwrap();
        // Forbidden now (she is not a member) — but it is not Pending: a
        // non-member posting is a definitive rejection at this state.
        assert!(matches!(
            st.apply(&net(), &post, ALICE),
            Err(Rejected::Forbidden { .. })
        ));
        // A comment on an unknown post is pending and resolves later.
        send(
            &mut st,
            &owner,
            pb::space_event::Body::MemberAdd(pb::SpaceMemberAdd {
                address: ALICE.into(),
                role: pb::SpaceRole::Member as i32,
            }),
        )
        .unwrap();
        let post_id = random_id().unwrap();
        let mut comment = build(
            &sid,
            ALICE,
            1,
            &[],
            pb::space_event::Body::Comment(pb::SpaceComment {
                post_id: post_id.clone(),
                text: "nice".into(),
            }),
        )
        .unwrap();
        sign(&net(), &alice.key, &mut comment).unwrap();
        assert!(matches!(
            st.apply(&net(), &comment, ALICE),
            Err(Rejected::Pending(_))
        ));
        let mut p = build(
            &sid,
            OWNER,
            st.next_sequence(OWNER),
            &[],
            pb::space_event::Body::Post(pb::SpacePost {
                text: "the post".into(),
                ..Default::default()
            }),
        )
        .unwrap();
        p.event_id = post_id;
        sign(&net(), &owner.key, &mut p).unwrap();
        st.apply(&net(), &p, OWNER).unwrap();
        assert_eq!(
            st.content.len(),
            2,
            "pending comment applied after its post"
        );
    }

    #[test]
    fn out_of_order_delivery_converges_to_the_same_state() {
        // The owner emits Create(1), MemberAdd(2), Announcement(3) in order;
        // a new member's store page delivers them as 3, 2, 1 (same second,
        // sorted by random id). Every reader must still end up with the
        // member added and the announcement applied.
        let owner = actor(OWNER, 1);
        let mut source = create(&owner);
        let sid = hex::decode(&source.space_id).unwrap();
        let mut events = Vec::new();
        let mk = |st: &mut State, body: pb::space_event::Body| {
            let mut e = build(&sid, OWNER, st.next_sequence(OWNER), &st.head_id(), body).unwrap();
            sign(&net(), &owner.key, &mut e).unwrap();
            st.apply(&net(), &e, OWNER).unwrap();
            e
        };
        // Reconstruct the Create the source applied (it is the first event).
        let create_ev = {
            let mut e = build(
                &sid,
                OWNER,
                1,
                &[],
                pb::space_event::Body::Create(pb::SpaceCreate {
                    name: "Team".into(),
                    description: String::new(),
                    owner: OWNER.into(),
                }),
            )
            .unwrap();
            e.event_id = sid.clone();
            sign(&net(), &owner.key, &mut e).unwrap();
            e
        };
        events.push(mk(
            &mut source,
            pb::space_event::Body::MemberAdd(pb::SpaceMemberAdd {
                address: ALICE.into(),
                role: pb::SpaceRole::Member as i32,
            }),
        ));
        events.push(mk(
            &mut source,
            pb::space_event::Body::Announcement(pb::SpaceAnnouncement {
                title: "Kick-off".into(),
                text: "Monday".into(),
                attachments: Vec::new(),
            }),
        ));
        // Reader gets 3, 2, then 1.
        let mut reader = State::new(&sid);
        assert!(matches!(
            reader.apply(&net(), &events[1], OWNER),
            Err(Rejected::Pending(_))
        ));
        assert!(matches!(
            reader.apply(&net(), &events[0], OWNER),
            Err(Rejected::Pending(_))
        ));
        reader.apply(&net(), &create_ev, OWNER).unwrap();
        assert_eq!(reader.role_of(ALICE), pb::SpaceRole::Member);
        assert_eq!(reader.content.len(), 1);
        assert_eq!(reader.sequences.get(OWNER), Some(&3));
        // And with the Create first but 3 before 2: the gap keeps 3 pending.
        let mut reader2 = State::new(&sid);
        reader2.apply(&net(), &create_ev, OWNER).unwrap();
        assert!(matches!(
            reader2.apply(&net(), &events[1], OWNER),
            Err(Rejected::Pending(_))
        ));
        reader2.apply(&net(), &events[0], OWNER).unwrap();
        assert_eq!(reader2.role_of(ALICE), pb::SpaceRole::Member);
        assert_eq!(reader2.content.len(), 1, "announcement applied after the gap filled");
    }
}

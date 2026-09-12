//! People: the contact state machine and its transitions.
//!
//! Contact state is local to the user (and synced to their own devices via
//! `ContactsSnapshot`); nothing about who is friends with whom is public
//! unless the user follows someone (a public social event). The states
//! are:
//!
//! ```text
//!  None ──request_sent──▶ PendingOut ──accepted──▶ Friend
//!  None ◀──request_recv── PendingIn  ──accept───▶ Friend
//!                          PendingIn  ──reject───▶ None
//!  Friend ──remove──▶ None
//!  any ──block──▶ Blocked (sticky until unblock; drops requests and mail)
//!  Friend ──trust──▶ Friend + Trusted
//!  any ──mute──▶ + Muted (no notifications; mail to Requests)
//! ```

use std::collections::BTreeMap;

use crate::ids::{now_ms, require_address, require_str_max};
use crate::pb;
use crate::version::{check_body, PEOPLE_VERSION};
use crate::AppError;

/// State flags.
pub const FRIEND: &str = "friend";
/// Outgoing request pending.
pub const PENDING_OUT: &str = "pending_out";
/// Incoming request pending.
pub const PENDING_IN: &str = "pending_in";
/// Blocked.
pub const BLOCKED: &str = "blocked";
/// Muted.
pub const MUTED: &str = "muted";
/// Trusted.
pub const TRUSTED: &str = "trusted";
/// Public follow (mirror of the social event, for the People UI).
pub const FOLLOWING: &str = "following";

/// Request message bytes.
pub const MAX_REQUEST_MESSAGE: usize = 1024;

/// Validates a contact request.
pub fn validate_request(r: &pb::ContactRequest) -> Result<(), AppError> {
    check_body("ContactRequest", r.version, PEOPLE_VERSION)?;
    require_str_max("display_name", &r.display_name, 128)?;
    require_str_max("username", &r.username, 64)?;
    require_str_max("message", &r.message, MAX_REQUEST_MESSAGE)?;
    Ok(())
}

/// Validates a profile card.
pub fn validate_card(c: &pb::ProfileCard) -> Result<(), AppError> {
    check_body("ProfileCard", c.version, PEOPLE_VERSION)?;
    require_str_max("display_name", &c.display_name, 128)?;
    require_str_max("bio", &c.bio, 4096)?;
    if !c.wallet_address.is_empty() {
        require_address("wallet_address", &c.wallet_address)?;
    }
    Ok(())
}

/// The contact book.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Contacts {
    /// By address.
    pub records: BTreeMap<String, pb::ContactRecord>,
}

impl Contacts {
    fn entry(&mut self, address: &str) -> Result<&mut pb::ContactRecord, AppError> {
        require_address("address", address)?;
        Ok(self
            .records
            .entry(address.to_owned())
            .or_insert_with(|| pb::ContactRecord {
                address: address.to_owned(),
                ..Default::default()
            }))
    }

    fn set(rec: &mut pb::ContactRecord, flag: &str, on: bool) {
        rec.states.retain(|s| s != flag);
        if on {
            rec.states.push(flag.to_owned());
        }
        rec.updated_at_ms = now_ms();
    }

    /// Whether a record has a flag.
    #[must_use]
    pub fn has(&self, address: &str, flag: &str) -> bool {
        self.records
            .get(address)
            .map(|r| r.states.iter().any(|s| s == flag))
            .unwrap_or(false)
    }

    /// Records a sent request.
    pub fn request_sent(&mut self, address: &str) -> Result<(), AppError> {
        if self.has(address, BLOCKED) {
            return Err(AppError::Invalid("unblock first".into()));
        }
        if self.has(address, FRIEND) {
            return Err(AppError::Invalid("already a contact".into()));
        }
        let r = self.entry(address)?;
        Self::set(r, PENDING_OUT, true);
        Ok(())
    }

    /// Records a received request. Returns false if it was dropped (blocked).
    pub fn request_received(
        &mut self,
        address: &str,
        username: &str,
        display_name: &str,
    ) -> Result<bool, AppError> {
        if self.has(address, BLOCKED) {
            return Ok(false);
        }
        if self.has(address, FRIEND) {
            return Ok(true);
        }
        let pending_out = self.has(address, PENDING_OUT);
        let r = self.entry(address)?;
        if !username.is_empty() {
            r.username = username.to_owned();
        }
        if !display_name.is_empty() {
            r.display_name = display_name.to_owned();
        }
        if pending_out {
            // Both asked: that is an acceptance.
            Self::set(r, PENDING_OUT, false);
            Self::set(r, FRIEND, true);
        } else {
            Self::set(r, PENDING_IN, true);
        }
        Ok(true)
    }

    /// The user accepts an incoming request.
    pub fn accept(&mut self, address: &str) -> Result<(), AppError> {
        if !self.has(address, PENDING_IN) {
            return Err(AppError::Invalid("no pending request".into()));
        }
        let r = self.entry(address)?;
        Self::set(r, PENDING_IN, false);
        Self::set(r, FRIEND, true);
        Ok(())
    }

    /// The user rejects an incoming request.
    pub fn reject(&mut self, address: &str) -> Result<(), AppError> {
        let r = self.entry(address)?;
        Self::set(r, PENDING_IN, false);
        Ok(())
    }

    /// The other side answered our request.
    pub fn response_received(&mut self, address: &str, accepted: bool) -> Result<(), AppError> {
        let r = self.entry(address)?;
        Self::set(r, PENDING_OUT, false);
        if accepted {
            Self::set(r, FRIEND, true);
        }
        Ok(())
    }

    /// Removes a friend.
    pub fn remove(&mut self, address: &str) -> Result<(), AppError> {
        let r = self.entry(address)?;
        Self::set(r, FRIEND, false);
        Self::set(r, TRUSTED, false);
        Self::set(r, PENDING_IN, false);
        Self::set(r, PENDING_OUT, false);
        Ok(())
    }

    /// Blocks: clears every relationship flag.
    pub fn block(&mut self, address: &str) -> Result<(), AppError> {
        let r = self.entry(address)?;
        for f in [FRIEND, TRUSTED, PENDING_IN, PENDING_OUT] {
            Self::set(r, f, false);
        }
        Self::set(r, BLOCKED, true);
        Ok(())
    }

    /// Unblocks.
    pub fn unblock(&mut self, address: &str) -> Result<(), AppError> {
        let r = self.entry(address)?;
        Self::set(r, BLOCKED, false);
        Ok(())
    }

    /// Mute / unmute.
    pub fn set_muted(&mut self, address: &str, on: bool) -> Result<(), AppError> {
        let r = self.entry(address)?;
        Self::set(r, MUTED, on);
        Ok(())
    }

    /// Trust / untrust (friends only).
    pub fn set_trusted(&mut self, address: &str, on: bool) -> Result<(), AppError> {
        if on && !self.has(address, FRIEND) {
            return Err(AppError::Invalid("only a contact can be trusted".into()));
        }
        let r = self.entry(address)?;
        Self::set(r, TRUSTED, on);
        Ok(())
    }

    /// Mirrors a public follow.
    pub fn set_following(&mut self, address: &str, on: bool) -> Result<(), AppError> {
        let r = self.entry(address)?;
        Self::set(r, FOLLOWING, on);
        Ok(())
    }

    /// Updates the cached name fields.
    pub fn set_names(
        &mut self,
        address: &str,
        username: &str,
        display_name: &str,
    ) -> Result<(), AppError> {
        let r = self.entry(address)?;
        r.username = username.to_owned();
        r.display_name = display_name.to_owned();
        r.updated_at_ms = now_ms();
        Ok(())
    }

    /// Facts for the spam policy.
    #[must_use]
    pub fn sender_facts(&self, address: &str) -> crate::spam::SenderFacts {
        crate::spam::SenderFacts {
            is_contact: self.has(address, FRIEND),
            is_trusted: self.has(address, TRUSTED),
            is_blocked: self.has(address, BLOCKED),
            is_muted: self.has(address, MUTED),
            has_username: self
                .records
                .get(address)
                .map(|r| !r.username.is_empty())
                .unwrap_or(false),
            ..Default::default()
        }
    }

    /// Records with a flag.
    #[must_use]
    pub fn with_flag(&self, flag: &str) -> Vec<&pb::ContactRecord> {
        self.records
            .values()
            .filter(|r| r.states.iter().any(|s| s == flag))
            .collect()
    }

    /// Snapshot for other devices.
    #[must_use]
    pub fn snapshot(&self) -> pb::ContactsSnapshot {
        pb::ContactsSnapshot {
            contacts: self.records.values().cloned().collect(),
            at_ms: now_ms(),
        }
    }

    /// Merges a snapshot from another of the user's devices: newer record
    /// wins per address.
    pub fn merge_snapshot(&mut self, s: &pb::ContactsSnapshot) {
        for c in &s.contacts {
            if !crate::ids::is_address(&c.address) {
                continue;
            }
            let newer = self
                .records
                .get(&c.address)
                .map(|r| c.updated_at_ms > r.updated_at_ms)
                .unwrap_or(true);
            if newer {
                self.records.insert(c.address.clone(), c.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "hash1e0tl2hff03hu4g3sawcjqa2p9tc4uh24e4vfl5";
    const B: &str = "hash178njz3gft77798ssh6lrqxh2hskw5dm68f2uh5";

    #[test]
    fn request_flow() {
        let mut c = Contacts::default();
        c.request_sent(A).unwrap();
        assert!(c.has(A, PENDING_OUT));
        c.response_received(A, true).unwrap();
        assert!(c.has(A, FRIEND) && !c.has(A, PENDING_OUT));
        assert!(c.request_sent(A).is_err());
        c.set_trusted(A, true).unwrap();
        c.remove(A).unwrap();
        assert!(!c.has(A, FRIEND) && !c.has(A, TRUSTED));
        // Incoming.
        assert!(c.request_received(B, "bob", "").unwrap());
        assert!(c.has(B, PENDING_IN));
        c.accept(B).unwrap();
        assert!(c.has(B, FRIEND));
        // Block drops.
        c.block(B).unwrap();
        assert!(!c.has(B, FRIEND) && c.has(B, BLOCKED));
        assert!(!c.request_received(B, "", "").unwrap());
        assert!(c.request_sent(B).is_err());
        c.unblock(B).unwrap();
        // Mutual requests become friendship.
        c.request_sent(B).unwrap();
        assert!(c.request_received(B, "", "").unwrap());
        assert!(c.has(B, FRIEND));
        assert!(c.set_trusted(A, true).is_err());
    }

    #[test]
    fn snapshot_merge_newer_wins() {
        let mut a = Contacts::default();
        a.request_sent(A).unwrap();
        let mut b = Contacts::default();
        std::thread::sleep(std::time::Duration::from_millis(2));
        b.block(A).unwrap();
        a.merge_snapshot(&b.snapshot());
        assert!(a.has(A, BLOCKED));
        // Older snapshot does not override.
        let mut old = Contacts::default();
        old.request_sent(A).unwrap();
        old.records.get_mut(A).unwrap().updated_at_ms = 1;
        a.merge_snapshot(&old.snapshot());
        assert!(a.has(A, BLOCKED));
    }
}

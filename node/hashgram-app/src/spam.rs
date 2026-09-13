//! Local spam and unknown-sender policy for HashMail.
//!
//! Decentralised encrypted mail cannot be filtered by a server: nobody but
//! the recipient sees the content, and nobody should be able to decide who
//! may write to whom. The defence is therefore local and layered:
//!
//! 1. **Contacts first.** Mail from a friend or trusted contact goes to the
//!    Inbox unconditionally.
//! 2. **Blocked senders** are dropped silently (the sender learns nothing).
//! 3. **Unknown senders** land in *Requests* with a trust score. The score
//!    is computed here from facts the client already has: has this sender
//!    ever been written to, do they hold a username (registration costs 1
//!    HASH and a year of commitment), how old is it, does the message look
//!    like a bulk blast (many recipients, no personal reference), and for
//!    gateway-bridged mail, the gateway's SPF/DKIM/DMARC verdicts and spam
//!    score.
//! 4. **Rate window.** More than `MAX_UNKNOWN_PER_HOUR` first-contact
//!    messages from distinct unknown senders in an hour puts the excess in
//!    Spam rather than Requests; a single unknown sender is capped at
//!    `MAX_PER_SENDER_PER_HOUR`.
//!
//! 5. **Postage.** A sender writing to someone who is not yet a contact
//!    attaches a *postage stamp*: a small proof of work over (recipient,
//!    message id) carried as a `postage:<nonce>` label. It costs a real
//!    person's PC well under a second per first-contact message and costs a
//!    bulk sender the same second for each of a million recipients. A valid
//!    stamp raises the trust score; its absence is not a penalty, so older
//!    clients still reach Requests.
//!
//! Nothing here contacts a network. The transport-level backstop is the
//! store node's per-mailbox quota; the economic backstop is that every
//! Hashgram identity costs gas to create and a username costs 1 HASH.

use std::collections::HashMap;

use crate::pb;

/// Label prefix of a postage stamp.
pub const POSTAGE_LABEL_PREFIX: &str = "postage:";
/// Leading zero bits a full stamp has. 2^20 ≈ one million hashes, roughly
/// 0.2–0.8 s on one core of an ordinary PC.
pub const POSTAGE_BITS: u32 = 20;
/// Most hash attempts a sender spends on one stamp before giving up and
/// sending without (2^24: sixteen times the expected work).
pub const POSTAGE_MAX_ITERS: u64 = 1 << 24;
/// Domain separation for the stamp digest.
const POSTAGE_DOMAIN: &[u8] = b"hashgram-mail-postage-v1";

/// The stamp digest for one nonce.
#[must_use]
pub fn postage_digest(recipient: &str, message_id: &[u8], nonce: u64) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(POSTAGE_DOMAIN);
    h.update(&(recipient.len() as u32).to_le_bytes());
    h.update(recipient.as_bytes());
    h.update(&(message_id.len() as u32).to_le_bytes());
    h.update(message_id);
    h.update(&nonce.to_le_bytes());
    *h.finalize().as_bytes()
}

/// Leading zero bits of a digest.
#[must_use]
pub fn leading_zero_bits(d: &[u8]) -> u32 {
    let mut n = 0;
    for b in d {
        if *b == 0 {
            n += 8;
        } else {
            n += b.leading_zeros();
            break;
        }
    }
    n
}

/// Mints a stamp label of at least `bits` for `(recipient, message_id)`,
/// trying at most `max_iters` nonces. `None` when the budget ran out.
#[must_use]
pub fn mint_postage(recipient: &str, message_id: &[u8], bits: u32, max_iters: u64) -> Option<String> {
    // A random start so two devices minting for the same message do not
    // duplicate work, and so the nonce reveals nothing about ordering.
    let mut nonce = u64::from_le_bytes(
        blake3::hash(&[message_id, &std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos().to_le_bytes())
            .unwrap_or([0; 16])].concat())
            .as_bytes()[..8]
            .try_into()
            .unwrap_or([0; 8]),
    );
    for _ in 0..max_iters {
        if leading_zero_bits(&postage_digest(recipient, message_id, nonce)) >= bits {
            return Some(format!("{POSTAGE_LABEL_PREFIX}{nonce:016x}"));
        }
        nonce = nonce.wrapping_add(1);
    }
    None
}

/// The strongest valid stamp a message carries for `recipient`, in bits;
/// 0 when none.
#[must_use]
pub fn postage_bits(m: &pb::MailMessage, recipient: &str) -> u32 {
    m.labels
        .iter()
        .filter_map(|l| l.strip_prefix(POSTAGE_LABEL_PREFIX))
        .filter_map(|n| u64::from_str_radix(n, 16).ok())
        .map(|nonce| leading_zero_bits(&postage_digest(recipient, &m.message_id, nonce)))
        .max()
        .unwrap_or(0)
}

/// Where a message is filed on arrival.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Disposition {
    /// Inbox.
    Inbox,
    /// Unknown sender awaiting the user's decision.
    Requests,
    /// Likely unwanted.
    Spam,
    /// Blocked sender; not stored.
    Drop,
}

/// What the policy knows about a sender.
#[derive(Debug, Clone, Default)]
pub struct SenderFacts {
    /// A contact the user accepted or added.
    pub is_contact: bool,
    /// Marked trusted by the user.
    pub is_trusted: bool,
    /// On the block list.
    pub is_blocked: bool,
    /// On the mute list (goes to Requests silently, no notification).
    pub is_muted: bool,
    /// The user has previously sent this address a message.
    pub previously_written_to: bool,
    /// Sender holds an on-chain username.
    pub has_username: bool,
    /// Age of the username registration in days, if known.
    pub username_age_days: Option<u32>,
    /// Messages from this sender accepted before.
    pub prior_messages: u32,
    /// Strength of the postage stamp the message carries for us, in bits
    /// (see [`postage_bits`]); 0 when none.
    pub postage_bits: u32,
}

/// Bounds of the rate window.
pub const MAX_UNKNOWN_PER_HOUR: u32 = 30;
/// Per-sender first-contact bound.
pub const MAX_PER_SENDER_PER_HOUR: u32 = 5;
/// Score at or above which an unknown sender goes to Requests rather than
/// Spam. A plain first message from a stranger with no username scores
/// −10 and lands in Requests; it takes a bulk shape, a failed gateway
/// authentication or a high gateway spam score to fall below this.
pub const REQUESTS_THRESHOLD: i32 = -20;

/// Sliding window of first-contact arrivals.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct RateWindow {
    /// (sender address, unix seconds).
    arrivals: Vec<(String, u64)>,
}

impl RateWindow {
    /// Records an arrival and reports whether the window is exceeded for
    /// the network as a whole or for this sender.
    pub fn record(&mut self, sender: &str, now_secs: u64) -> (bool, bool) {
        self.arrivals
            .retain(|(_, t)| now_secs.saturating_sub(*t) < 3600);
        self.arrivals.push((sender.to_owned(), now_secs));
        if self.arrivals.len() > 4096 {
            let excess = self.arrivals.len() - 4096;
            self.arrivals.drain(..excess);
        }
        let mut distinct: HashMap<&str, u32> = HashMap::new();
        for (s, _) in &self.arrivals {
            *distinct.entry(s.as_str()).or_insert(0) += 1;
        }
        let total_ok = distinct.len() as u32 <= MAX_UNKNOWN_PER_HOUR;
        let sender_ok = distinct.get(sender).copied().unwrap_or(0) <= MAX_PER_SENDER_PER_HOUR;
        (!total_ok, !sender_ok)
    }
}

/// Computes a trust score for an unknown sender. Positive is trustworthy.
#[must_use]
pub fn trust_score(facts: &SenderFacts, m: &pb::MailMessage) -> i32 {
    let mut s: i32 = 0;
    if facts.previously_written_to {
        s += 40;
    }
    if facts.has_username {
        s += 15;
        match facts.username_age_days {
            Some(d) if d >= 180 => s += 15,
            Some(d) if d >= 30 => s += 8,
            Some(_) => s += 2,
            None => {}
        }
    } else {
        s -= 10;
    }
    s += (facts.prior_messages.min(10) as i32) * 3;
    // Postage: work spent on this recipient specifically. A full stamp
    // outweighs the missing-username penalty; a partial one softens it.
    if facts.postage_bits >= POSTAGE_BITS {
        s += 30;
    } else if facts.postage_bits >= POSTAGE_BITS - 4 {
        s += 12;
    }
    // Bulk shape: many recipients and no reply reference.
    let recipients = m.to.len() + m.cc.len();
    if recipients > 20 && m.in_reply_to.is_empty() {
        s -= 25;
    } else if recipients > 5 && m.in_reply_to.is_empty() {
        s -= 8;
    }
    if !m.in_reply_to.is_empty() {
        s += 10;
    }
    if m.subject.is_empty() && m.body_text.len() < 20 {
        s -= 5;
    }
    if m.origin == pb::MailOrigin::ExternalGateway as i32 {
        // External mail is inherently less trusted; the gateway's verdicts
        // move it up or down.
        s -= 15;
        if let Some(e) = &m.external {
            for r in &e.auth_results {
                let r = r.to_ascii_lowercase();
                if r == "dkim=pass" || r == "spf=pass" {
                    s += 8;
                } else if r == "dmarc=pass" {
                    s += 10;
                } else if r.ends_with("=fail") {
                    s -= 20;
                }
            }
            if e.spam_score > 700 {
                s -= 40;
            } else if e.spam_score > 400 {
                s -= 15;
            }
        }
    }
    s.clamp(-100, 100)
}

/// Decides where an arriving message goes.
pub fn dispose(
    facts: &SenderFacts,
    m: &pb::MailMessage,
    window: &mut RateWindow,
    now_secs: u64,
) -> Disposition {
    if facts.is_blocked {
        return Disposition::Drop;
    }
    if facts.is_trusted || facts.is_contact {
        return Disposition::Inbox;
    }
    if facts.previously_written_to && !facts.is_muted {
        return Disposition::Inbox;
    }
    let sender = m.from.as_ref().map(|a| a.address.as_str()).unwrap_or("");
    let (network_exceeded, sender_exceeded) = window.record(sender, now_secs);
    if sender_exceeded {
        return Disposition::Spam;
    }
    let score = trust_score(facts, m);
    if network_exceeded && score < 30 {
        return Disposition::Spam;
    }
    if score >= REQUESTS_THRESHOLD {
        Disposition::Requests
    } else {
        Disposition::Spam
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(from: &str, recipients: usize) -> pb::MailMessage {
        pb::MailMessage {
            from: Some(pb::MailAddress {
                address: from.into(),
                ..Default::default()
            }),
            to: (0..recipients)
                .map(|i| pb::MailAddress {
                    address: format!("hash1r{i}"),
                    ..Default::default()
                })
                .collect(),
            subject: "hello".into(),
            body_text: "a reasonably long body of text".into(),
            ..Default::default()
        }
    }

    #[test]
    fn contacts_go_to_inbox_blocked_dropped() {
        let mut w = RateWindow::default();
        let m = msg("hash1a", 1);
        assert_eq!(
            dispose(
                &SenderFacts {
                    is_contact: true,
                    ..Default::default()
                },
                &m,
                &mut w,
                0
            ),
            Disposition::Inbox
        );
        assert_eq!(
            dispose(
                &SenderFacts {
                    is_blocked: true,
                    is_contact: true,
                    ..Default::default()
                },
                &m,
                &mut w,
                0
            ),
            Disposition::Drop
        );
    }

    #[test]
    fn unknown_with_username_is_a_request_bulk_is_spam() {
        let mut w = RateWindow::default();
        let facts = SenderFacts {
            has_username: true,
            username_age_days: Some(400),
            ..Default::default()
        };
        assert_eq!(
            dispose(&facts, &msg("hash1a", 1), &mut w, 0),
            Disposition::Requests
        );
        // A stranger without a username is still a request, not spam.
        assert_eq!(
            dispose(&SenderFacts::default(), &msg("hash1z", 1), &mut w, 0),
            Disposition::Requests
        );
        let bulk = msg("hash1b", 50);
        assert_eq!(
            dispose(&SenderFacts::default(), &bulk, &mut w, 0),
            Disposition::Spam
        );
    }

    #[test]
    fn per_sender_rate_limit() {
        let mut w = RateWindow::default();
        let facts = SenderFacts {
            has_username: true,
            ..Default::default()
        };
        let mut last = Disposition::Requests;
        for _ in 0..(MAX_PER_SENDER_PER_HOUR + 1) {
            last = dispose(&facts, &msg("hash1flood", 1), &mut w, 100);
        }
        assert_eq!(last, Disposition::Spam);
        // Window expiry.
        assert_eq!(
            dispose(&facts, &msg("hash1flood", 1), &mut w, 100 + 3601),
            Disposition::Requests
        );
    }

    #[test]
    fn network_wide_window() {
        let mut w = RateWindow::default();
        let facts = SenderFacts::default(); // low score
        let mut d = Disposition::Requests;
        for i in 0..(MAX_UNKNOWN_PER_HOUR + 1) {
            d = dispose(&facts, &msg(&format!("hash1s{i}"), 1), &mut w, 5);
        }
        assert_eq!(
            d,
            Disposition::Spam,
            "past the hourly window low-score strangers go to spam"
        );
    }

    #[test]
    fn postage_is_minted_verified_and_bound_to_the_recipient() {
        let id = vec![7u8; 16];
        // Small difficulty keeps the test fast; the check is structural.
        let label = mint_postage("hash1recipient", &id, 8, 1 << 20).unwrap();
        assert!(label.starts_with(POSTAGE_LABEL_PREFIX));
        let mut m = msg("hash1sender", 1);
        m.message_id = id.clone();
        m.labels.push(label);
        assert!(postage_bits(&m, "hash1recipient") >= 8);
        // The digest is bound to recipient and message id: changing either
        // changes it (so a stamp cannot be reused across a mailing list).
        let nonce = u64::from_str_radix(&m.labels[0][POSTAGE_LABEL_PREFIX.len()..], 16).unwrap();
        assert_ne!(
            postage_digest("hash1recipient", &id, nonce),
            postage_digest("hash1other", &id, nonce)
        );
        assert_ne!(
            postage_digest("hash1recipient", &id, nonce),
            postage_digest("hash1recipient", &[8u8; 16], nonce)
        );
        assert_eq!(postage_bits(&msg("x", 1), "hash1recipient"), 0);
        // A malformed stamp counts as none.
        let mut bad = msg("x", 1);
        bad.labels.push("postage:zz".into());
        assert_eq!(postage_bits(&bad, "hash1recipient"), 0);
        assert_eq!(leading_zero_bits(&[0, 0, 0b0001_0000]), 19);
        assert_eq!(leading_zero_bits(&[0xff]), 0);
    }

    #[test]
    fn postage_lifts_a_stranger_into_requests_more_firmly() {
        let plain = trust_score(&SenderFacts::default(), &msg("hash1a", 1));
        let stamped = trust_score(
            &SenderFacts {
                postage_bits: POSTAGE_BITS,
                ..Default::default()
            },
            &msg("hash1a", 1),
        );
        assert_eq!(stamped - plain, 30);
        // Postage does not launder a bulk blast.
        let bulk = trust_score(
            &SenderFacts {
                postage_bits: POSTAGE_BITS,
                ..Default::default()
            },
            &msg("hash1a", 50),
        );
        assert!(bulk < stamped);
    }

    #[test]
    fn gateway_verdicts_move_score() {
        let mut m = msg("hash1gw", 1);
        m.origin = pb::MailOrigin::ExternalGateway as i32;
        m.external = Some(pb::ExternalMailMeta {
            auth_results: vec!["dmarc=fail".into()],
            spam_score: 800,
            ..Default::default()
        });
        assert!(trust_score(&SenderFacts::default(), &m) < -50);
        m.external = Some(pb::ExternalMailMeta {
            auth_results: vec!["dkim=pass".into(), "spf=pass".into(), "dmarc=pass".into()],
            spam_score: 10,
            ..Default::default()
        });
        assert!(
            trust_score(
                &SenderFacts {
                    has_username: true,
                    ..Default::default()
                },
                &m
            ) >= 0
        );
    }
}

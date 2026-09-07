//! Peer scoring.
//!
//! A peer's score is a running judgement about whether it is worth talking to.
//! It is not a reputation system in the social sense: it is a local, private,
//! decaying number that decides whether this node keeps a connection.
//!
//! # Why scores decay
//!
//! Without decay, a peer that misbehaved once is punished forever, and a
//! long-lived honest peer accumulates enough credit to misbehave freely.
//! Both are wrong. Scores move toward zero over time, so recent behaviour
//! dominates and a node that was briefly broken can recover.
//!
//! # Why scores are local and unshared
//!
//! A shared reputation system is a system an attacker can use to get honest
//! peers banned. Every node forms its own view from its own observations, so
//! poisoning it requires misbehaving toward each victim individually.
//!
//! # What is deliberately not scored
//!
//! Not message content, and not anything requiring plaintext. This layer
//! cannot read an encrypted envelope and does not try. Scoring is on
//! observable protocol behaviour: whether frames parse, whether signatures
//! verify, whether the peer floods, whether it answers.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Below this, a peer is graylisted: existing connections are kept but no new
/// ones are opened to it, and it is not offered to other peers.
///
/// A separate, gentler state than a ban, because most bad scores come from a
/// node that is overloaded or on a poor link rather than from an attacker,
/// and dropping it immediately makes a bad network worse.
pub const GRAYLIST_THRESHOLD: i32 = -50;

/// Below this, a peer is banned: disconnected and refused.
pub const BAN_THRESHOLD: i32 = -100;

/// Score floor and ceiling.
///
/// Bounded so a long-lived peer cannot bank enough credit to misbehave
/// freely, and a briefly broken one cannot dig a hole it takes days to climb
/// out of.
const SCORE_MIN: i32 = -200;
const SCORE_MAX: i32 = 100;

/// How much of the distance to zero is forgiven per decay interval, as a
/// percentage.
const DECAY_PERCENT: i32 = 10;

/// How often decay is applied.
const DECAY_INTERVAL: Duration = Duration::from_secs(60);

/// Something a peer did.
///
/// The weights encode a judgement about intent. A malformed frame might be a
/// bug; an invalid signature cannot be, because producing one requires either
/// a forgery attempt or a broken implementation that should be disconnected
/// either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreEvent {
    /// Completed the Hashgram handshake, including the genesis hash check.
    HandshakeSucceeded,
    /// Failed the handshake. Usually a fork or a foreign network.
    HandshakeFailed,
    /// Delivered a well-formed, useful message.
    UsefulMessage,
    /// Answered a request we made.
    RequestAnswered,
    /// Did not answer a request within the timeout.
    RequestTimedOut,
    /// Sent a frame that did not parse.
    MalformedFrame,
    /// Sent a message whose signature did not verify.
    InvalidSignature,
    /// Exceeded a rate limit.
    RateLimitExceeded,
    /// Sent the same message again.
    DuplicateMessage,
    /// Claimed to serve data it then did not serve.
    FailedToServe,
    /// Served a blob chunk that did not match its content hash.
    ServedCorruptData,
}

impl ScoreEvent {
    /// The score delta for this event.
    #[must_use]
    pub const fn weight(self) -> i32 {
        match self {
            // Positive. Small, because credit should accumulate slowly: a
            // peer should not be able to earn tolerance for abuse by being
            // briefly useful.
            Self::HandshakeSucceeded => 5,
            Self::UsefulMessage => 1,
            Self::RequestAnswered => 2,

            // Ambiguous. A timeout or a duplicate is often a bad link rather
            // than bad intent, so it costs little on its own and only matters
            // when it repeats.
            Self::RequestTimedOut => -2,
            Self::DuplicateMessage => -1,

            // Probably a bug, possibly probing. Worth disconnecting over if
            // sustained, not on a single occurrence.
            Self::MalformedFrame => -10,
            Self::RateLimitExceeded => -15,
            Self::FailedToServe => -20,

            // Cannot be an accident. A wrong-genesis handshake means a fork,
            // an invalid signature means forgery or a broken implementation,
            // and corrupt data served against a content hash means the peer
            // is actively wrong. These are heavy.
            Self::HandshakeFailed => -30,
            Self::InvalidSignature => -50,
            Self::ServedCorruptData => -50,
        }
    }
}

/// One peer's score.
#[derive(Debug, Clone)]
pub struct PeerScore {
    score: i32,
    /// When decay was last applied. Advances on every decay pass.
    last_decay: Instant,
    /// When an event was last recorded for this peer.
    ///
    /// Separate from `last_decay` on purpose. An earlier version used one
    /// field for both, which made pruning a no-op: `decay_all` advanced the
    /// timestamp for every peer, so the "has been idle" test always failed.
    /// Idleness is about observation, not about housekeeping.
    last_seen: Instant,
    observations: u64,
}

impl PeerScore {
    /// A new peer starts at zero: neither trusted nor suspected.
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            score: 0,
            last_decay: now,
            last_seen: now,
            observations: 0,
        }
    }

    /// The current score, after applying any decay that is due.
    #[must_use]
    pub fn score(&self) -> i32 {
        self.score
    }

    /// How many events have been recorded for this peer.
    #[must_use]
    pub fn observations(&self) -> u64 {
        self.observations
    }

    /// When an event was last recorded for this peer.
    #[must_use]
    pub fn last_seen(&self) -> Instant {
        self.last_seen
    }

    /// Whether this peer should be banned.
    #[must_use]
    pub fn is_banned(&self) -> bool {
        self.score <= BAN_THRESHOLD
    }

    /// Whether this peer should be graylisted.
    #[must_use]
    pub fn is_graylisted(&self) -> bool {
        self.score <= GRAYLIST_THRESHOLD
    }

    /// Records an event and returns the new score.
    pub fn record(&mut self, event: ScoreEvent, now: Instant) -> i32 {
        self.decay(now);
        self.score = (self.score + event.weight()).clamp(SCORE_MIN, SCORE_MAX);
        self.observations = self.observations.saturating_add(1);
        self.last_seen = now;
        self.score
    }

    /// Applies time-based decay toward zero.
    ///
    /// Proportional rather than a fixed step, so a deeply negative score
    /// recovers quickly at first and slowly near zero. A fixed step would
    /// either forgive a serious offence too fast or leave a minor one
    /// lingering.
    pub fn decay(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last_decay);
        // Truncating division is intended: a partial interval is not yet due.
        #[allow(clippy::integer_division)]
        let intervals = elapsed.as_secs() / DECAY_INTERVAL.as_secs();
        if intervals == 0 {
            return;
        }

        // Capped so a peer idle for a month is not walked one interval at a
        // time. Thirty intervals brings any score inside the bounds to
        // effectively zero.
        let intervals = intervals.min(30);

        for _ in 0..intervals {
            if self.score == 0 {
                break;
            }
            // Integer arithmetic, always moving at least one point, so a
            // small score reaches zero instead of asymptoting at 1 or -1.
            // Truncating division, floored at one so a small score reaches
            // zero rather than asymptoting at plus or minus one.
            #[allow(clippy::integer_division)]
            let step = (self.score.abs() * DECAY_PERCENT / 100).max(1);
            self.score -= step * self.score.signum();
        }

        self.last_decay = now;
    }
}

/// Scores for every peer this node has observed.
#[derive(Debug, Default)]
pub struct Scoreboard {
    peers: HashMap<String, PeerScore>,
}

impl Scoreboard {
    /// An empty scoreboard.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an event for a peer, returning the new score.
    pub fn record(&mut self, peer: &str, event: ScoreEvent, now: Instant) -> i32 {
        self.peers
            .entry(peer.to_owned())
            .or_insert_with(|| PeerScore::new(now))
            .record(event, now)
    }

    /// A peer's current score, or zero if it has never been seen.
    ///
    /// Zero for an unknown peer, rather than an error, because "we have no
    /// opinion" and "we are neutral" are the same thing here.
    #[must_use]
    pub fn score(&self, peer: &str) -> i32 {
        self.peers.get(peer).map_or(0, PeerScore::score)
    }

    /// Whether a peer is banned.
    #[must_use]
    pub fn is_banned(&self, peer: &str) -> bool {
        self.peers.get(peer).is_some_and(PeerScore::is_banned)
    }

    /// Whether a peer is graylisted.
    #[must_use]
    pub fn is_graylisted(&self, peer: &str) -> bool {
        self.peers.get(peer).is_some_and(PeerScore::is_graylisted)
    }

    /// Applies decay to every peer.
    pub fn decay_all(&mut self, now: Instant) {
        for score in self.peers.values_mut() {
            score.decay(now);
        }
    }

    /// Forgets peers that are at zero and have not been seen recently.
    ///
    /// Without this the scoreboard grows without bound on a long-running
    /// node, which is a slow memory leak an attacker can accelerate by
    /// connecting from many identities. A peer at zero carries no information,
    /// so dropping it loses nothing.
    pub fn prune(&mut self, now: Instant, idle_for: Duration) {
        self.decay_all(now);
        self.peers
            .retain(|_, s| s.score != 0 || now.saturating_duration_since(s.last_seen) < idle_for);
    }

    /// How many peers are tracked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether the scoreboard is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn a_new_peer_is_neutral() {
        let board = Scoreboard::new();
        assert_eq!(board.score("unknown"), 0);
        assert!(!board.is_banned("unknown"));
        assert!(!board.is_graylisted("unknown"));
    }

    #[test]
    fn a_single_invalid_signature_does_not_ban() {
        // One bad signature could be a broken client. It should hurt without
        // immediately partitioning a peer that may be fixable.
        let mut board = Scoreboard::new();
        let now = t0();

        board.record("peer", ScoreEvent::InvalidSignature, now);
        assert!(!board.is_banned("peer"), "one bad signature caused a ban");
        assert!(board.is_graylisted("peer"), "one bad signature was ignored");
    }

    #[test]
    fn repeated_forgery_bans() {
        let mut board = Scoreboard::new();
        let now = t0();

        for _ in 0..3 {
            board.record("forger", ScoreEvent::InvalidSignature, now);
        }
        assert!(board.is_banned("forger"), "sustained forgery did not ban");
    }

    #[test]
    fn a_fork_peer_is_penalised_but_not_instantly_banned() {
        // A failed handshake is usually a fork or a misconfigured node, not
        // an attack. It should cost enough to stop retrying and not enough to
        // ban a peer that might be about to be reconfigured.
        let mut board = Scoreboard::new();
        let now = t0();

        board.record("fork", ScoreEvent::HandshakeFailed, now);
        assert_eq!(board.score("fork"), -30);
        assert!(!board.is_banned("fork"));
        assert!(!board.is_graylisted("fork"));

        board.record("fork", ScoreEvent::HandshakeFailed, now);
        assert!(
            board.is_graylisted("fork"),
            "a repeatedly failing peer stayed accepted"
        );
    }

    #[test]
    fn usefulness_cannot_buy_tolerance_for_abuse() {
        // The property that stops a peer earning credit cheaply and then
        // spending it on abuse. A thousand useful messages must not offset a
        // handful of forgeries.
        let mut board = Scoreboard::new();
        let now = t0();

        for _ in 0..1_000 {
            board.record("mixed", ScoreEvent::UsefulMessage, now);
        }
        assert_eq!(
            board.score("mixed"),
            SCORE_MAX,
            "score is not capped, so credit can be banked without limit"
        );

        for _ in 0..5 {
            board.record("mixed", ScoreEvent::InvalidSignature, now);
        }
        assert!(
            board.is_banned("mixed"),
            "a peer banked enough credit to forge signatures freely"
        );
    }

    #[test]
    fn scores_decay_toward_zero() {
        let mut score = PeerScore::new(t0());
        let start = Instant::now();

        score.record(ScoreEvent::MalformedFrame, start);
        let after_event = score.score();
        assert!(after_event < 0);

        // Ten minutes later.
        score.decay(start + Duration::from_secs(600));
        assert!(
            score.score() > after_event,
            "score did not decay: {} then {}",
            after_event,
            score.score()
        );
        assert!(score.score() <= 0, "decay overshot past zero");
    }

    #[test]
    fn decay_reaches_exactly_zero_rather_than_asymptoting() {
        // Proportional decay with integer arithmetic would stall at 1 or -1
        // without the minimum step, leaving peers permanently non-neutral.
        let mut score = PeerScore::new(t0());
        let start = Instant::now();

        score.record(ScoreEvent::MalformedFrame, start);
        score.decay(start + Duration::from_secs(60 * 60 * 24));
        assert_eq!(score.score(), 0, "decay did not reach zero");
    }

    #[test]
    fn decay_does_not_flip_the_sign() {
        for event in [ScoreEvent::InvalidSignature, ScoreEvent::HandshakeSucceeded] {
            let mut score = PeerScore::new(t0());
            let start = Instant::now();
            let initial = score.record(event, start);

            for minutes in 1..=120 {
                score.decay(start + Duration::from_secs(60 * minutes));
                assert!(
                    score.score().signum() == 0 || score.score().signum() == initial.signum(),
                    "{event:?}: decay flipped the sign from {initial} to {}",
                    score.score()
                );
            }
        }
    }

    #[test]
    fn a_banned_peer_recovers_after_long_good_behaviour() {
        // Permanent bans are wrong for a network where most bad scores come
        // from overload rather than malice.
        let mut board = Scoreboard::new();
        let start = Instant::now();

        for _ in 0..3 {
            board.record("recovering", ScoreEvent::InvalidSignature, start);
        }
        assert!(board.is_banned("recovering"));

        board.decay_all(start + Duration::from_secs(60 * 60 * 2));
        assert!(
            !board.is_banned("recovering"),
            "a peer stayed banned after two hours of quiet: score {}",
            board.score("recovering")
        );
    }

    #[test]
    fn the_score_is_bounded_in_both_directions() {
        let mut score = PeerScore::new(t0());
        let now = Instant::now();

        for _ in 0..10_000 {
            score.record(ScoreEvent::ServedCorruptData, now);
        }
        assert_eq!(score.score(), SCORE_MIN, "score floor not enforced");

        let mut score = PeerScore::new(t0());
        for _ in 0..10_000 {
            score.record(ScoreEvent::HandshakeSucceeded, now);
        }
        assert_eq!(score.score(), SCORE_MAX, "score ceiling not enforced");
    }

    #[test]
    fn every_event_moves_the_score() {
        // A weight of zero would mean an event is recorded and ignored, which
        // is worse than not recording it: it looks like it is being handled.
        for event in [
            ScoreEvent::HandshakeSucceeded,
            ScoreEvent::HandshakeFailed,
            ScoreEvent::UsefulMessage,
            ScoreEvent::RequestAnswered,
            ScoreEvent::RequestTimedOut,
            ScoreEvent::MalformedFrame,
            ScoreEvent::InvalidSignature,
            ScoreEvent::RateLimitExceeded,
            ScoreEvent::DuplicateMessage,
            ScoreEvent::FailedToServe,
            ScoreEvent::ServedCorruptData,
        ] {
            assert_ne!(event.weight(), 0, "{event:?} has no effect on the score");
        }
    }

    #[test]
    fn unforgivable_events_outweigh_ambiguous_ones() {
        // A forged signature must cost more than a timeout, or a peer could
        // hide forgery behind noise.
        assert!(
            ScoreEvent::InvalidSignature.weight() < ScoreEvent::RequestTimedOut.weight(),
            "forgery is cheaper than a timeout"
        );
        assert!(
            ScoreEvent::ServedCorruptData.weight() < ScoreEvent::DuplicateMessage.weight(),
            "serving corrupt data is cheaper than a duplicate"
        );
        // And one unforgivable event must be enough to graylist on its own.
        assert!(ScoreEvent::InvalidSignature.weight() <= GRAYLIST_THRESHOLD);
    }

    #[test]
    fn pruning_bounds_memory_without_losing_information() {
        // The scoreboard is attacker-influenced: connecting from many
        // identities grows it. Neutral, idle peers carry no information, so
        // dropping them is free.
        let mut board = Scoreboard::new();
        let start = Instant::now();

        for i in 0..1_000 {
            board.record(&format!("neutral{i}"), ScoreEvent::UsefulMessage, start);
        }
        board.record("offender", ScoreEvent::InvalidSignature, start);
        assert_eq!(board.len(), 1_001);

        // Long enough for the neutral peers to decay to zero.
        board.prune(
            start + Duration::from_secs(60 * 60 * 24),
            Duration::from_secs(600),
        );

        assert!(
            board.len() < 1_001,
            "pruning kept every peer: {} remain",
            board.len()
        );
        assert!(
            board.len() <= 1,
            "pruning kept {} peers when at most the offender should remain",
            board.len()
        );
    }

    #[test]
    fn graylisting_happens_before_banning() {
        // The thresholds must be ordered, or graylisting is unreachable and a
        // peer goes straight from healthy to banned.
        assert!(
            BAN_THRESHOLD < GRAYLIST_THRESHOLD,
            "the ban threshold is not below the graylist threshold"
        );
        assert!(
            SCORE_MIN < BAN_THRESHOLD,
            "the score floor is above the ban threshold"
        );
    }

    #[test]
    fn observations_are_counted() {
        let mut board = Scoreboard::new();
        let now = t0();
        for _ in 0..7 {
            board.record("counted", ScoreEvent::UsefulMessage, now);
        }
        assert_eq!(board.peers["counted"].observations(), 7);
    }
}

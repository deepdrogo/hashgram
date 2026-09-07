//! Connection limits.
//!
//! Without limits, a node accepts connections until it runs out of file
//! descriptors or memory, which makes exhausting it cheaper than attacking
//! it. The limits here are per-peer, per-subnet and global, because each
//! bounds a different attack.
//!
//! # Why per-subnet and not only per-peer
//!
//! A per-peer limit is trivially bypassed: an attacker with a /24 has 256
//! addresses and can open the per-peer maximum from each. Limiting by /24 for
//! IPv4 and /64 for IPv6 raises the cost from "one machine" to "addresses in
//! many networks", which is a real expense.
//!
//! The /64 choice for IPv6 matters. A single host is routinely handed a whole
//! /64, so limiting by full address would be no limit at all, and limiting by
//! /48 would penalise unrelated customers of the same provider.
//!
//! # Why inbound and outbound are separate
//!
//! Inbound connections are attacker-controlled; outbound are ours. Filling
//! the table with inbound connections must not stop this node from reaching
//! the peers it chose, so outbound slots are reserved and cannot be consumed
//! by inbound pressure.

use std::collections::HashMap;
use std::net::IpAddr;

/// Whether a connection may be accepted, and why not if not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitDecision {
    /// Accept it.
    Allow,
    /// This peer already has the maximum number of connections.
    ///
    /// Usually a buggy client reconnecting in a loop rather than an attack.
    TooManyFromPeer {
        /// How many the peer already has.
        current: usize,
        /// The per-peer maximum.
        max: usize,
    },
    /// This subnet already has the maximum.
    TooManyFromSubnet {
        /// The subnet, as a printable prefix.
        subnet: String,
        /// How many the subnet already has.
        current: usize,
        /// The per-subnet maximum.
        max: usize,
    },
    /// The global inbound limit is reached.
    ///
    /// Outbound slots are unaffected: they are reserved so inbound pressure
    /// cannot stop this node reaching the peers it chose.
    InboundFull {
        /// How many inbound connections are established.
        current: usize,
        /// The inbound maximum.
        max: usize,
    },
    /// The peer is banned by the scoreboard.
    PeerBanned,
}

impl LimitDecision {
    /// Whether the connection is allowed.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

impl std::fmt::Display for LimitDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allow => f.write_str("allowed"),
            Self::TooManyFromPeer { current, max } => write!(
                f,
                "peer already has {current} of {max} permitted connections"
            ),
            Self::TooManyFromSubnet {
                subnet,
                current,
                max,
            } => write!(
                f,
                "subnet {subnet} already has {current} of {max} permitted connections"
            ),
            Self::InboundFull { current, max } => write!(
                f,
                "inbound connections full at {current} of {max}; outbound slots are reserved and unaffected"
            ),
            Self::PeerBanned => f.write_str("peer is banned by its score"),
        }
    }
}

/// Connection limits and the current connection table.
#[derive(Debug)]
pub struct ConnectionLimits {
    max_per_peer: usize,
    max_per_subnet: usize,
    max_inbound: usize,
    max_outbound: usize,

    per_peer: HashMap<String, usize>,
    per_subnet: HashMap<String, usize>,
    inbound: usize,
    outbound: usize,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionLimits {
    /// Default limits, sized for a node on a small VPS.
    ///
    /// Two connections per peer allows a QUIC and a TCP connection to coexist
    /// during a transport migration without either being refused. Four per
    /// subnet allows a small operator running a few nodes on one network
    /// while making a /24 flood cost 256 times more than a single host.
    /// Reserving 32 outbound slots means inbound pressure cannot isolate this
    /// node from the peers it chose.
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_per_peer: 2,
            max_per_subnet: 4,
            max_inbound: 128,
            max_outbound: 32,
            per_peer: HashMap::new(),
            per_subnet: HashMap::new(),
            inbound: 0,
            outbound: 0,
        }
    }

    /// Overrides the limits.
    ///
    /// Every value is forced to at least one. A zero limit would mean a node
    /// that refuses every connection while appearing configured, which is a
    /// misconfiguration that presents as a network fault.
    #[must_use]
    pub fn with_limits(
        mut self,
        max_per_peer: usize,
        max_per_subnet: usize,
        max_inbound: usize,
        max_outbound: usize,
    ) -> Self {
        self.max_per_peer = max_per_peer.max(1);
        self.max_per_subnet = max_per_subnet.max(1);
        self.max_inbound = max_inbound.max(1);
        self.max_outbound = max_outbound.max(1);
        self
    }

    /// Whether an inbound connection may be accepted.
    ///
    /// Checks are ordered cheapest first and, within that, most specific
    /// first, so the reported reason is the most useful one: "this peer is
    /// reconnecting in a loop" is more actionable than "we are full".
    #[must_use]
    pub fn allow_inbound(&self, peer: &str, addr: IpAddr, banned: bool) -> LimitDecision {
        if banned {
            return LimitDecision::PeerBanned;
        }

        let current = self.per_peer.get(peer).copied().unwrap_or(0);
        if current >= self.max_per_peer {
            return LimitDecision::TooManyFromPeer {
                current,
                max: self.max_per_peer,
            };
        }

        let subnet = subnet_key(addr);
        let current = self.per_subnet.get(&subnet).copied().unwrap_or(0);
        if current >= self.max_per_subnet {
            return LimitDecision::TooManyFromSubnet {
                subnet,
                current,
                max: self.max_per_subnet,
            };
        }

        if self.inbound >= self.max_inbound {
            return LimitDecision::InboundFull {
                current: self.inbound,
                max: self.max_inbound,
            };
        }

        LimitDecision::Allow
    }

    /// Whether an outbound connection may be opened.
    ///
    /// Outbound limits are per-peer and global-outbound only. Subnet limits
    /// do not apply: this node chose the address, so an attacker cannot use
    /// the subnet limit to stop it dialling.
    #[must_use]
    pub fn allow_outbound(&self, peer: &str, banned: bool) -> LimitDecision {
        if banned {
            return LimitDecision::PeerBanned;
        }

        let current = self.per_peer.get(peer).copied().unwrap_or(0);
        if current >= self.max_per_peer {
            return LimitDecision::TooManyFromPeer {
                current,
                max: self.max_per_peer,
            };
        }

        if self.outbound >= self.max_outbound {
            return LimitDecision::InboundFull {
                current: self.outbound,
                max: self.max_outbound,
            };
        }

        LimitDecision::Allow
    }

    /// Records an established connection.
    pub fn established(&mut self, peer: &str, addr: IpAddr, inbound: bool) {
        *self.per_peer.entry(peer.to_owned()).or_insert(0) += 1;
        *self.per_subnet.entry(subnet_key(addr)).or_insert(0) += 1;
        if inbound {
            self.inbound += 1;
        } else {
            self.outbound += 1;
        }
    }

    /// Records a closed connection.
    ///
    /// Counters saturate at zero and empty entries are removed. An unbalanced
    /// close would otherwise either underflow or leak a map entry per peer
    /// forever, and a peer can cause many closes.
    pub fn closed(&mut self, peer: &str, addr: IpAddr, inbound: bool) {
        if let Some(count) = self.per_peer.get_mut(peer) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.per_peer.remove(peer);
            }
        }

        let subnet = subnet_key(addr);
        if let Some(count) = self.per_subnet.get_mut(&subnet) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.per_subnet.remove(&subnet);
            }
        }

        if inbound {
            self.inbound = self.inbound.saturating_sub(1);
        } else {
            self.outbound = self.outbound.saturating_sub(1);
        }
    }

    /// Established inbound connections.
    #[must_use]
    pub fn inbound_count(&self) -> usize {
        self.inbound
    }

    /// Established outbound connections.
    #[must_use]
    pub fn outbound_count(&self) -> usize {
        self.outbound
    }

    /// Distinct peers with at least one connection.
    #[must_use]
    pub fn peer_count(&self) -> usize {
        self.per_peer.len()
    }

    /// Distinct subnets with at least one connection.
    #[must_use]
    pub fn subnet_count(&self) -> usize {
        self.per_subnet.len()
    }
}

/// The subnet an address is grouped into: /24 for IPv4, /64 for IPv6.
///
/// The IPv6 choice is the interesting one. A single host is routinely
/// delegated an entire /64, so grouping by full address would be no limit at
/// all. Grouping by /48 would lump together unrelated customers of the same
/// provider and let one of them exhaust the others' slots.
fn subnet_key(addr: IpAddr) -> String {
    match addr {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.0/24", o[0], o[1], o[2])
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn a_fresh_node_accepts_connections() {
        let limits = ConnectionLimits::new();
        assert!(limits
            .allow_inbound("peer", v4(203, 0, 113, 1), false)
            .is_allowed());
    }

    #[test]
    fn a_banned_peer_is_refused_before_anything_else() {
        let limits = ConnectionLimits::new();
        assert_eq!(
            limits.allow_inbound("bad", v4(203, 0, 113, 1), true),
            LimitDecision::PeerBanned
        );
        assert_eq!(
            limits.allow_outbound("bad", true),
            LimitDecision::PeerBanned
        );
    }

    #[test]
    fn one_peer_cannot_open_unlimited_connections() {
        let mut limits = ConnectionLimits::new();
        let addr = v4(203, 0, 113, 1);

        limits.established("loop", addr, true);
        limits.established("loop", addr, true);

        match limits.allow_inbound("loop", addr, false) {
            LimitDecision::TooManyFromPeer { current, max } => {
                assert_eq!(current, 2);
                assert_eq!(max, 2);
            }
            other => panic!("a third connection from one peer was allowed: {other:?}"),
        }
    }

    #[test]
    fn a_single_subnet_cannot_exhaust_the_node() {
        // The attack a per-peer limit alone does not stop: many identities
        // from one network, each within the per-peer limit.
        let mut limits = ConnectionLimits::new();

        for i in 1..=4 {
            limits.established(&format!("sybil{i}"), v4(203, 0, 113, i), true);
        }

        match limits.allow_inbound("sybil5", v4(203, 0, 113, 5), false) {
            LimitDecision::TooManyFromSubnet {
                subnet,
                current,
                max,
            } => {
                assert_eq!(subnet, "203.0.113.0/24");
                assert_eq!(current, 4);
                assert_eq!(max, 4);
            }
            other => panic!("a /24 flood was not limited: {other:?}"),
        }

        // A different subnet is unaffected, so the limit does not partition
        // the node from the rest of the network.
        assert!(limits
            .allow_inbound("elsewhere", v4(198, 51, 100, 1), false)
            .is_allowed());
    }

    #[test]
    fn an_ipv6_host_with_a_whole_slash_64_is_still_limited() {
        // A single IPv6 host is routinely given a /64. Grouping by full
        // address would make the subnet limit meaningless.
        let mut limits = ConnectionLimits::new();

        for i in 1..=4u16 {
            let addr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, i));
            limits.established(&format!("v6peer{i}"), addr, true);
        }

        let next = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 99));
        assert!(
            matches!(
                limits.allow_inbound("v6peer99", next, false),
                LimitDecision::TooManyFromSubnet { .. }
            ),
            "addresses within one /64 were treated as separate subnets"
        );

        // A different /64 is separate, so a genuinely different network is
        // not punished.
        let other = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, 1));
        assert!(limits.allow_inbound("other64", other, false).is_allowed());
    }

    #[test]
    fn inbound_pressure_cannot_stop_this_node_dialling_out() {
        // The property that keeps a node from being isolated by a flood.
        let mut limits = ConnectionLimits::new().with_limits(2, 1_000, 4, 8);

        for i in 0..4 {
            limits.established(&format!("flood{i}"), v4(198, 51, 100, i), true);
        }

        assert!(
            matches!(
                limits.allow_inbound("more", v4(198, 51, 100, 200), false),
                LimitDecision::InboundFull { .. }
            ),
            "the inbound limit was not enforced"
        );

        assert!(
            limits.allow_outbound("chosen-peer", false).is_allowed(),
            "a full inbound table blocked an outbound connection, which would \
             let a flood isolate this node"
        );
    }

    #[test]
    fn closing_a_connection_frees_its_slot() {
        let mut limits = ConnectionLimits::new();
        let addr = v4(203, 0, 113, 1);

        limits.established("peer", addr, true);
        limits.established("peer", addr, true);
        assert!(!limits.allow_inbound("peer", addr, false).is_allowed());

        limits.closed("peer", addr, true);
        assert!(limits.allow_inbound("peer", addr, false).is_allowed());
    }

    #[test]
    fn unbalanced_closes_neither_underflow_nor_leak() {
        // A peer can cause many closes. An unbalanced one must not panic on
        // underflow, and must not leave a map entry behind for every peer
        // that ever connected.
        let mut limits = ConnectionLimits::new();
        let addr = v4(203, 0, 113, 1);

        limits.closed("never-connected", addr, true);
        limits.closed("never-connected", addr, false);
        assert_eq!(limits.inbound_count(), 0);
        assert_eq!(limits.outbound_count(), 0);
        assert_eq!(limits.peer_count(), 0);
        assert_eq!(limits.subnet_count(), 0);

        limits.established("peer", addr, true);
        limits.closed("peer", addr, true);
        limits.closed("peer", addr, true);
        assert_eq!(limits.peer_count(), 0, "a peer entry leaked after closing");
        assert_eq!(
            limits.subnet_count(),
            0,
            "a subnet entry leaked after closing"
        );
    }

    #[test]
    fn a_zero_limit_is_refused_rather_than_bricking_the_node() {
        // A zero limit would refuse every connection while looking
        // configured, presenting as a network fault rather than a
        // misconfiguration.
        let limits = ConnectionLimits::new().with_limits(0, 0, 0, 0);
        assert!(limits
            .allow_inbound("peer", v4(203, 0, 113, 1), false)
            .is_allowed());
        assert!(limits.allow_outbound("peer", false).is_allowed());
    }

    #[test]
    fn every_refusal_explains_itself() {
        // An operator reading a log needs to know which limit fired and what
        // the numbers were, not that "a connection was refused".
        let mut limits = ConnectionLimits::new().with_limits(1, 1, 1, 1);
        let addr = v4(203, 0, 113, 1);
        limits.established("first", addr, true);

        let peer_full = limits.allow_inbound("first", addr, false).to_string();
        assert!(
            peer_full.contains("1 of 1"),
            "unhelpful message: {peer_full}"
        );

        let subnet_full = limits
            .allow_inbound("second", v4(203, 0, 113, 2), false)
            .to_string();
        assert!(
            subnet_full.contains("203.0.113.0/24"),
            "the subnet message does not name the subnet: {subnet_full}"
        );

        let banned = limits.allow_inbound("x", addr, true).to_string();
        assert!(banned.contains("banned"), "unhelpful message: {banned}");
    }

    #[test]
    fn counts_are_tracked_separately_for_each_direction() {
        let mut limits = ConnectionLimits::new();
        limits.established("a", v4(203, 0, 113, 1), true);
        limits.established("b", v4(198, 51, 100, 1), false);

        assert_eq!(limits.inbound_count(), 1);
        assert_eq!(limits.outbound_count(), 1);
        assert_eq!(limits.peer_count(), 2);
        assert_eq!(limits.subnet_count(), 2);
    }
}

//! Chain relay: read-only chain queries and transaction broadcast over P2P.
//!
//! A wallet needs two things from the chain — answers to read queries and a
//! way to hand in a signed transaction — and until now both needed an HTTP
//! REST gateway, which every node binds to loopback. This service lets a
//! `relay` or `bootstrap` node forward an allow-list of read paths, and the
//! one broadcast path, from the peers it already talks to over
//! `/hashgram/rpc/1` to its co-located gateway.
//!
//! Rules, every one of them enforced here and tested:
//!
//! - Only paths on [`ALLOWED_PREFIXES`] (plus the two exact `cosmos/tx`
//!   read paths) are forwarded. `simulate` is refused: it executes
//!   arbitrary transactions against the gateway and is not a read.
//! - A path with `..`, `//`, a `?`, `#`, whitespace, a backslash or any
//!   non-printable / non-ASCII byte is refused before the gateway sees it.
//! - Answers are forwarded verbatim (status, body, reported height) so two
//!   nodes can be compared byte for byte by the client, and are capped at
//!   [`MAX_RESPONSE_BYTES`]; the gateway is given [`UPSTREAM_TIMEOUT`].
//! - Each peer gets its own token bucket ([`RATE_PER_SEC`], [`RATE_BURST`]),
//!   on top of the swarm's per-peer limit.
//! - Nothing here earns service credit: chain relaying is a public good,
//!   like peer exchange, and `docs/SERVICE_REWARDS.md` says so.
//!
//! The same allow-list guards the local API's `/v1/chain/{*path}`
//! passthrough (see `api.rs`), so there is exactly one definition of "a
//! read a node will forward".

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hashgram_p2p::PeerId;
use hashgram_proto::pb;
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::registry::Registry;
use tracing::debug;

use crate::chain::{ChainClient, RawError, RawResponse};

/// Path prefixes (relative to the gateway root, no leading slash) a node
/// forwards. Everything under them is a read in the Cosmos SDK gateway.
pub const ALLOWED_PREFIXES: &[&str] = &[
    "cosmos/bank/",
    "cosmos/auth/",
    "cosmos/staking/",
    "cosmos/distribution/",
    "cosmos/gov/",
    "cosmos/base/",
    "hashgram/",
];

/// The transaction list read (`?query=...` in the query string).
pub const TX_LIST_PATH: &str = "cosmos/tx/v1beta1/txs";
/// The transaction-by-hash read prefix; the tail must be a 64-hex hash.
pub const TX_BY_HASH_PREFIX: &str = "cosmos/tx/v1beta1/txs/";
/// The one path a broadcast is forwarded to.
pub const BROADCAST_PATH: &str = "cosmos/tx/v1beta1/txs";

/// Longest path accepted.
pub const MAX_PATH_LEN: usize = 512;
/// Longest query string accepted.
pub const MAX_QUERY_LEN: usize = 1024;
/// Largest gateway answer forwarded.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Largest signed transaction accepted for broadcast.
pub const MAX_TX_BYTES: usize = 32 * 1024;
/// How long the gateway gets to answer.
pub const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(5);
/// Sustained requests per second per peer.
pub const RATE_PER_SEC: f64 = 10.0;
/// Burst per peer.
pub const RATE_BURST: f64 = 20.0;

/// Why a path was refused. Returned to the requester in the error message
/// so a client author learns the rule, not just "no".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Empty path.
    Empty,
    /// Longer than [`MAX_PATH_LEN`] / [`MAX_QUERY_LEN`].
    TooLong,
    /// A byte that has no business in a gateway path.
    BadCharacter,
    /// `..`, `//`, `./` or a trailing `/.`.
    Traversal,
    /// Not on the allow-list (includes `simulate`).
    NotAllowed,
}

impl Refusal {
    /// A one-line explanation.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "empty path",
            Self::TooLong => "path or query too long",
            Self::BadCharacter => "path contains a character that is not allowed",
            Self::Traversal => "path contains a traversal sequence",
            Self::NotAllowed => "only read queries under cosmos/bank/, cosmos/auth/, cosmos/staking/, cosmos/distribution/, cosmos/gov/, cosmos/base/, hashgram/ and cosmos/tx/v1beta1/txs are forwarded",
        }
    }
}

fn path_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '/' | '-' | '_' | '.' | '~' | '%' | ':' | '@' | '=' | '+' | ','
        )
}

fn query_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '/' | '-'
                | '_'
                | '.'
                | '~'
                | '%'
                | ':'
                | '@'
                | '='
                | '&'
                | '+'
                | ','
                | '\''
                | '('
                | ')'
                | '*'
                | '!'
                | '$'
                | ';'
        )
}

/// Checks a read path and returns it normalised (leading slashes removed)
/// when a node may forward it.
pub fn allowed_read_path(path: &str) -> Result<String, Refusal> {
    let p = path.trim_start_matches('/');
    if p.is_empty() {
        return Err(Refusal::Empty);
    }
    if p.len() > MAX_PATH_LEN {
        return Err(Refusal::TooLong);
    }
    if !p.chars().all(path_char_ok) {
        return Err(Refusal::BadCharacter);
    }
    if p.contains("..")
        || p.contains("//")
        || p.contains("/./")
        || p.starts_with("./")
        || p.ends_with("/.")
    {
        return Err(Refusal::Traversal);
    }
    // Percent-encoded traversal or separators are refused too; the gateway
    // decodes them, we do not.
    let lower = p.to_ascii_lowercase();
    if lower.contains("%2e")
        || lower.contains("%2f")
        || lower.contains("%5c")
        || lower.contains("%00")
    {
        return Err(Refusal::Traversal);
    }
    if p == TX_LIST_PATH {
        return Ok(p.to_owned());
    }
    if let Some(tail) = p.strip_prefix(TX_BY_HASH_PREFIX) {
        if tail.len() == 64 && tail.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(p.to_owned());
        }
        return Err(Refusal::NotAllowed);
    }
    if ALLOWED_PREFIXES.iter().any(|pre| p.starts_with(pre)) {
        return Ok(p.to_owned());
    }
    Err(Refusal::NotAllowed)
}

/// Checks a query string (without `?`).
pub fn allowed_query(query: &str) -> Result<(), Refusal> {
    if query.len() > MAX_QUERY_LEN {
        return Err(Refusal::TooLong);
    }
    if !query.chars().all(query_char_ok) {
        return Err(Refusal::BadCharacter);
    }
    Ok(())
}

/// Outcome label for metrics (bounded).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
#[allow(missing_docs)]
pub enum Outcome {
    Ok,
    Denied,
    RateLimited,
    Timeout,
    Unreachable,
    TooLarge,
}

/// Metric label set.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct OutcomeLabel {
    /// Outcome.
    pub outcome: Outcome,
}

/// Kind label set.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct KindLabel {
    /// `query` or `broadcast`.
    pub kind: Kind,
}

/// What was asked.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
#[allow(missing_docs)]
pub enum Kind {
    Query,
    Broadcast,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// The relay service.
pub struct ChainRelayService {
    chain: ChainClient,
    buckets: Mutex<HashMap<PeerId, Bucket>>,
    requests: Family<KindLabel, Counter>,
    outcomes: Family<OutcomeLabel, Counter>,
}

impl ChainRelayService {
    /// Creates the service and registers its metrics under
    /// `hashgram_node_chain_relay_`.
    pub fn new(chain: ChainClient, registry: &mut Registry) -> Self {
        let r = registry.sub_registry_with_prefix("hashgram_node_chain_relay");
        let requests = Family::<KindLabel, Counter>::default();
        let outcomes = Family::<OutcomeLabel, Counter>::default();
        r.register(
            "requests",
            "Chain relay requests received, by kind",
            requests.clone(),
        );
        r.register("outcomes", "Chain relay request outcomes", outcomes.clone());
        Self {
            chain,
            buckets: Mutex::new(HashMap::new()),
            requests,
            outcomes,
        }
    }

    fn take_token(&self, peer: PeerId) -> bool {
        let now = Instant::now();
        let Ok(mut map) = self.buckets.lock() else {
            return false;
        };
        // Bounded: forget idle peers so the map cannot grow without limit.
        if map.len() > 4096 {
            map.retain(|_, b| now.duration_since(b.last) < Duration::from_secs(60));
        }
        let b = map.entry(peer).or_insert(Bucket {
            tokens: RATE_BURST,
            last: now,
        });
        let elapsed = now.duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * RATE_PER_SEC).min(RATE_BURST);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn note(&self, outcome: Outcome) {
        self.outcomes.get_or_create(&OutcomeLabel { outcome }).inc();
    }

    fn map_raw(&self, r: Result<RawResponse, RawError>) -> Result<RawResponse, Box<pb::Response>> {
        match r {
            Ok(r) => {
                self.note(Outcome::Ok);
                Ok(r)
            }
            Err(RawError::Timeout) => {
                self.note(Outcome::Timeout);
                Err(Box::new(err("internal", "chain gateway timed out")))
            }
            Err(RawError::TooLarge(n)) => {
                self.note(Outcome::TooLarge);
                // Data, not an error: the client can narrow its query.
                Ok(RawResponse {
                    status: 413,
                    body: format!("{{\"error\":\"answer exceeds {n} bytes; narrow the query\"}}")
                        .into_bytes(),
                    height: 0,
                })
            }
            Err(RawError::Unreachable(e)) => {
                self.note(Outcome::Unreachable);
                debug!(error = %e, "chain gateway unreachable for relay");
                Err(Box::new(err("internal", "chain gateway unreachable")))
            }
        }
    }

    /// Answers a `ChainQuery`.
    pub async fn query(&self, peer: PeerId, q: &pb::ChainQuery) -> pb::Response {
        self.requests
            .get_or_create(&KindLabel { kind: Kind::Query })
            .inc();
        if !self.take_token(peer) {
            self.note(Outcome::RateLimited);
            return err("rate_limited", "too many chain queries; slow down");
        }
        let path = match allowed_read_path(&q.path) {
            Ok(p) => p,
            Err(why) => {
                self.note(Outcome::Denied);
                return err("invalid", why.message());
            }
        };
        if let Err(why) = allowed_query(&q.query) {
            self.note(Outcome::Denied);
            return err("invalid", why.message());
        }
        let fetched = tokio::time::timeout(
            UPSTREAM_TIMEOUT,
            self.chain.get_raw(&path, &q.query, MAX_RESPONSE_BYTES),
        )
        .await
        .unwrap_or(Err(RawError::Timeout));
        match self.map_raw(fetched) {
            Ok(r) => pb::Response {
                body: Some(pb::response::Body::ChainQuery(pb::ChainQueryResponse {
                    status: u32::from(r.status),
                    body: r.body,
                    height: r.height,
                })),
            },
            Err(e) => *e,
        }
    }

    /// Answers a `ChainBroadcast`.
    pub async fn broadcast(&self, peer: PeerId, b: &pb::ChainBroadcast) -> pb::Response {
        self.requests
            .get_or_create(&KindLabel {
                kind: Kind::Broadcast,
            })
            .inc();
        if !self.take_token(peer) {
            self.note(Outcome::RateLimited);
            return err("rate_limited", "too many broadcasts; slow down");
        }
        if b.tx_bytes.is_empty() || b.tx_bytes.len() > MAX_TX_BYTES {
            self.note(Outcome::Denied);
            return err("invalid", "transaction is empty or too large");
        }
        let body = serde_json::json!({
            "tx_bytes": b64(&b.tx_bytes),
            "mode": "BROADCAST_MODE_SYNC",
        });
        let fetched = tokio::time::timeout(
            UPSTREAM_TIMEOUT,
            self.chain
                .post_raw(BROADCAST_PATH, &body, MAX_RESPONSE_BYTES),
        )
        .await
        .unwrap_or(Err(RawError::Timeout));
        match self.map_raw(fetched) {
            Ok(r) => pb::Response {
                body: Some(pb::response::Body::ChainBroadcast(
                    pb::ChainBroadcastResult {
                        status: u32::from(r.status),
                        body: r.body,
                        height: r.height,
                    },
                )),
            },
            Err(e) => *e,
        }
    }
}

fn err(code: &str, msg: impl Into<String>) -> pb::Response {
    pb::Response {
        body: Some(pb::response::Body::Error(pb::Error {
            code: code.to_owned(),
            message: msg.into(),
        })),
    }
}

/// Standard base64 with padding (the gateway's `tx_bytes` encoding).
fn b64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(
                    T.get(((n >> (18 - 6 * i)) & 63) as usize)
                        .copied()
                        .unwrap_or(b'A') as char,
                );
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list_accepts_wallet_reads() {
        for p in [
            "cosmos/bank/v1beta1/balances/hash1abc/by_denom",
            "/cosmos/bank/v1beta1/balances/hash1abc/by_denom",
            "cosmos/auth/v1beta1/accounts/hash1abc",
            "cosmos/staking/v1beta1/validators",
            "cosmos/distribution/v1beta1/delegators/hash1abc/rewards",
            "cosmos/gov/v1/proposals",
            "cosmos/base/tendermint/v1beta1/blocks/latest",
            "hashgram/username/v1/lookup/alice",
            "hashgram/founder/v1/revenue",
            "cosmos/tx/v1beta1/txs",
            "cosmos/tx/v1beta1/txs/0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF",
        ] {
            assert!(allowed_read_path(p).is_ok(), "{p} should be allowed");
        }
        assert_eq!(
            allowed_read_path("/cosmos/bank/v1beta1/supply").unwrap(),
            "cosmos/bank/v1beta1/supply"
        );
    }

    #[test]
    fn allow_list_denies_simulate() {
        assert_eq!(
            allowed_read_path("cosmos/tx/v1beta1/simulate"),
            Err(Refusal::NotAllowed)
        );
        assert_eq!(
            allowed_read_path("/cosmos/tx/v1beta1/simulate"),
            Err(Refusal::NotAllowed)
        );
    }

    #[test]
    fn allow_list_denies_traversal() {
        for p in [
            "cosmos/bank/../tx/v1beta1/simulate",
            "cosmos/bank//v1beta1/supply",
            "hashgram/./x",
            "hashgram/%2e%2e/x",
            "hashgram/x%2Fy",
            "cosmos/bank/v1beta1/supply/.",
        ] {
            assert_eq!(allowed_read_path(p), Err(Refusal::Traversal), "{p}");
        }
    }

    #[test]
    fn allow_list_denies_everything_off_list() {
        for p in [
            "cosmos/tx/v1beta1/txs/notahash",
            "cosmos/tx/v1beta1/txs/0123",
            "cosmos/txs",
            "cosmos/slashing/v1beta1/params",
            "cosmos/upgrade/v1beta1/current_plan",
            "node_info",
            "hashgramx/founder/v1/params",
            "v1/status",
            "metrics",
        ] {
            assert_eq!(allowed_read_path(p), Err(Refusal::NotAllowed), "{p}");
        }
        assert_eq!(allowed_read_path(""), Err(Refusal::Empty));
        assert_eq!(allowed_read_path("///"), Err(Refusal::Empty));
    }

    #[test]
    fn allow_list_denies_bad_bytes() {
        for p in [
            "cosmos/bank/v1beta1/supply?denom=uhash",
            "cosmos/bank/v1beta1/supply#x",
            "cosmos/bank/v1beta1/sup ply",
            "cosmos/bank\\v1beta1",
            "cosmos/bank/v1beta1/\u{7}",
            "hashgram/username/v1/lookup/ალისა",
        ] {
            assert_eq!(allowed_read_path(p), Err(Refusal::BadCharacter), "{p}");
        }
        let long = format!("hashgram/{}", "a".repeat(MAX_PATH_LEN));
        assert_eq!(allowed_read_path(&long), Err(Refusal::TooLong));
    }

    #[test]
    fn query_strings_are_bounded_and_printable() {
        assert!(allowed_query("").is_ok());
        assert!(allowed_query("denom=uhash").is_ok());
        assert!(allowed_query(
            "query=transfer.recipient%3D'hash1abc'&limit=20&order_by=ORDER_BY_DESC"
        )
        .is_ok());
        assert_eq!(allowed_query("a=b c"), Err(Refusal::BadCharacter));
        assert_eq!(allowed_query("a=<script>"), Err(Refusal::BadCharacter));
        assert_eq!(
            allowed_query(&"a".repeat(MAX_QUERY_LEN + 1)),
            Err(Refusal::TooLong)
        );
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64(b"fo"), "Zm8=");
    }
}

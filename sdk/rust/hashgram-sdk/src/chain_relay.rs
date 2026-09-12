//! Chain reads and broadcast over the P2P relay, cross-checked.
//!
//! A wallet without an HTTP gateway of its own reads the chain through the
//! nodes it is already connected to (`ChainQuery` on `/hashgram/rpc/1`,
//! served by `relay`/`bootstrap` nodes). No single node is trusted for an
//! answer: every read is issued to **two peers run by different operators**
//! (operator address from the handshake) and the answers are compared byte
//! for byte after JSON normalisation. Heights must be within
//! [`MAX_HEIGHT_SKEW`] blocks. A mismatch marks both peers disputed, asks a
//! third, and returns whichever of the two the third agrees with; the UI
//! shows "verified by 2 nodes" or a warning from [`Verification`].
//!
//! This is not a light client: nothing here checks a Merkle proof against a
//! header. It is the interim that makes "no single server" true for the
//! wallet, and `docs/CLIENT_CONNECTIVITY_SPEC.md` §10 says so.

use std::collections::HashSet;
use std::sync::Mutex;

use hashgram_chain::{ChainTransport, ClientError, TransportResponse, Verification};
use hashgram_p2p::PeerId;
use tracing::{debug, warn};

use crate::link::{ChainAnswer, KnownPeer, Link, LinkError};

/// How far apart two answers' heights may be and still be compared.
pub const MAX_HEIGHT_SKEW: u64 = 3;

/// Peers that relay chain queries, and how to ask them. [`Link`] is the
/// real one; tests plug in a mock with a lying peer.
#[async_trait::async_trait]
pub trait RelayPeers: Send + Sync {
    /// Verified peers that serve the relay (role `relay` or `bootstrap`).
    async fn relays(&self) -> Vec<KnownPeer>;
    /// One read through one peer.
    async fn get(&self, peer: PeerId, path: &str, query: &str) -> Result<ChainAnswer, LinkError>;
    /// One broadcast through one peer.
    async fn broadcast(&self, peer: PeerId, tx_bytes: Vec<u8>) -> Result<ChainAnswer, LinkError>;
}

#[async_trait::async_trait]
impl RelayPeers for Link {
    async fn relays(&self) -> Vec<KnownPeer> {
        self.chain_relays().await
    }
    async fn get(&self, peer: PeerId, path: &str, query: &str) -> Result<ChainAnswer, LinkError> {
        self.chain_get(peer, path, query).await
    }
    async fn broadcast(&self, peer: PeerId, tx_bytes: Vec<u8>) -> Result<ChainAnswer, LinkError> {
        self.chain_broadcast(peer, tx_bytes).await
    }
}

/// The transport.
pub struct P2pChainTransport<B: RelayPeers> {
    backend: std::sync::Arc<B>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Peers that gave an answer another node contradicted. Used last.
    disputed: HashSet<PeerId>,
    /// Who served the most recent read.
    last: Option<Verification>,
    /// The most recent per-peer failure, for error messages.
    last_error: Option<String>,
}

/// One answer with who gave it.
#[derive(Debug, Clone)]
struct Served {
    peer: KnownPeer,
    answer: ChainAnswer,
}

/// Paths whose bodies legitimately differ between honest nodes at different
/// heights (the latest block, sync status); they are read from one peer and
/// reported as not compared.
fn comparable(path: &str) -> bool {
    !path.starts_with("cosmos/base/tendermint/")
}

/// Canonical form for comparison: JSON re-serialised with sorted keys; raw
/// bytes when the body is not JSON.
fn normalise(body: &[u8]) -> Vec<u8> {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(v) => serde_json::to_vec(&v).unwrap_or_else(|_| body.to_vec()),
        Err(_) => body.to_vec(),
    }
}

fn same_answer(a: &ChainAnswer, b: &ChainAnswer) -> bool {
    a.status == b.status && normalise(&a.body) == normalise(&b.body)
}

fn heights_close(a: u64, b: u64) -> bool {
    a == 0 || b == 0 || a.abs_diff(b) <= MAX_HEIGHT_SKEW
}

impl<B: RelayPeers> P2pChainTransport<B> {
    /// Wraps a backend.
    pub fn new(backend: std::sync::Arc<B>) -> Self {
        Self {
            backend,
            state: Mutex::new(State::default()),
        }
    }

    /// Peers currently set aside for disagreeing.
    pub fn disputed(&self) -> Vec<PeerId> {
        self.state
            .lock()
            .map(|s| s.disputed.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Forgets disputes (e.g. after the user asks to retry).
    pub fn clear_disputes(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.disputed.clear();
        }
    }

    fn set_last(&self, v: Verification) {
        if let Ok(mut s) = self.state.lock() {
            s.last = Some(v);
        }
    }

    fn mark_disputed(&self, peers: &[PeerId]) {
        if let Ok(mut s) = self.state.lock() {
            for p in peers {
                s.disputed.insert(*p);
            }
        }
    }

    /// Orders candidate peers: undisputed first, then disputed; within each
    /// group, one peer per operator first so the first two picked come from
    /// different operators whenever the network allows it.
    fn order(&self, relays: Vec<KnownPeer>) -> Vec<KnownPeer> {
        let disputed: HashSet<PeerId> = self
            .state
            .lock()
            .map(|s| s.disputed.clone())
            .unwrap_or_default();
        let (clean, bad): (Vec<_>, Vec<_>) = relays
            .into_iter()
            .partition(|p| !disputed.contains(&p.peer));
        let mut out = Vec::new();
        for group in [clean, bad] {
            let mut seen_ops = HashSet::new();
            let mut firsts = Vec::new();
            let mut rest = Vec::new();
            for p in group {
                if p.operator.is_empty() || !seen_ops.insert(p.operator.clone()) {
                    rest.push(p);
                } else {
                    firsts.push(p);
                }
            }
            out.extend(firsts);
            out.extend(rest);
        }
        out
    }

    async fn ask(&self, peer: &KnownPeer, path: &str, query: &str) -> Option<Served> {
        match self.backend.get(peer.peer, path, query).await {
            Ok(answer) => Some(Served {
                peer: peer.clone(),
                answer,
            }),
            Err(e) => {
                debug!(peer = %peer.peer, error = %e, "chain relay peer did not answer");
                if let Ok(mut s) = self.state.lock() {
                    s.last_error = Some(format!("{}: {e}", peer.peer));
                }
                None
            }
        }
    }

    fn last_error(&self) -> String {
        self.state
            .lock()
            .ok()
            .and_then(|s| s.last_error.clone())
            .unwrap_or_else(|| "no answer".into())
    }

    /// Picks the next peer from `pool` whose operator is not in `used`
    /// (falling back to any unused peer), removing it.
    fn take_distinct(pool: &mut Vec<KnownPeer>, used_ops: &HashSet<String>) -> Option<KnownPeer> {
        let idx = pool
            .iter()
            .position(|p| !p.operator.is_empty() && !used_ops.contains(&p.operator))
            .or_else(|| (!pool.is_empty()).then_some(0))?;
        Some(pool.remove(idx))
    }

    async fn cross_checked_get(
        &self,
        path: &str,
        query: &str,
    ) -> Result<TransportResponse, ClientError> {
        let relays = self.backend.relays().await;
        if relays.is_empty() {
            return Err(ClientError::NoNode(
                "no connected node serves the chain relay (relay or bootstrap role)".into(),
            ));
        }
        let distinct_operators: HashSet<&str> = relays
            .iter()
            .map(|p| p.operator.as_str())
            .filter(|o| !o.is_empty())
            .collect();
        let single_operator = distinct_operators.len() <= 1;
        let mut pool = self.order(relays);
        let mut used_ops: HashSet<String> = HashSet::new();

        // First answer.
        let mut first = None;
        while first.is_none() {
            let Some(p) = Self::take_distinct(&mut pool, &used_ops) else {
                break;
            };
            used_ops.insert(p.operator.clone());
            first = self.ask(&p, path, query).await;
        }
        let Some(first) = first else {
            return Err(ClientError::NoNode(format!(
                "every connected relay node failed to answer (last: {})",
                self.last_error()
            )));
        };

        if !comparable(path) || single_operator {
            let v = Verification {
                peers: vec![first.peer.peer.to_string()],
                operators: vec![first.peer.operator.clone()],
                heights: vec![first.answer.height],
                agreed: false,
                single_operator,
                disputed: Vec::new(),
            };
            self.set_last(v);
            return Ok(to_response(first.answer));
        }

        // Second answer from another operator.
        let mut second = None;
        while second.is_none() {
            let Some(p) = Self::take_distinct(&mut pool, &used_ops) else {
                break;
            };
            used_ops.insert(p.operator.clone());
            second = self.ask(&p, path, query).await;
        }
        let Some(mut second) = second else {
            let v = Verification {
                peers: vec![first.peer.peer.to_string()],
                operators: vec![first.peer.operator.clone()],
                heights: vec![first.answer.height],
                agreed: false,
                single_operator: false,
                disputed: Vec::new(),
            };
            self.set_last(v);
            return Ok(to_response(first.answer));
        };

        let mut first = first;
        // A height race is not a lie: if the two were served at different
        // heights, ask the one behind again once before judging.
        if !same_answer(&first.answer, &second.answer)
            && first.answer.height != second.answer.height
            && first.answer.height != 0
            && second.answer.height != 0
        {
            let behind = if first.answer.height < second.answer.height {
                &mut first
            } else {
                &mut second
            };
            if let Some(again) = self.ask(&behind.peer.clone(), path, query).await {
                behind.answer = again.answer;
            }
        }

        if same_answer(&first.answer, &second.answer)
            && heights_close(first.answer.height, second.answer.height)
        {
            let v = Verification {
                peers: vec![first.peer.peer.to_string(), second.peer.peer.to_string()],
                operators: vec![first.peer.operator.clone(), second.peer.operator.clone()],
                heights: vec![first.answer.height, second.answer.height],
                agreed: true,
                single_operator: false,
                disputed: Vec::new(),
            };
            self.set_last(v);
            return Ok(to_response(first.answer));
        }

        // Disagreement: both are suspect; a third decides.
        warn!(
            a = %first.peer.peer,
            b = %second.peer.peer,
            path,
            "chain relay peers disagree; asking a third"
        );
        self.mark_disputed(&[first.peer.peer, second.peer.peer]);
        let mut third = None;
        while third.is_none() {
            let Some(p) = Self::take_distinct(&mut pool, &used_ops) else {
                break;
            };
            used_ops.insert(p.operator.clone());
            third = self.ask(&p, path, query).await;
        }
        let Some(third) = third else {
            self.set_last(Verification {
                peers: Vec::new(),
                operators: Vec::new(),
                heights: Vec::new(),
                agreed: false,
                single_operator: false,
                disputed: vec![first.peer.peer.to_string(), second.peer.peer.to_string()],
            });
            return Err(ClientError::Disputed(format!(
                "{} and {} answered differently for {path} and no third node was reachable",
                first.peer.peer, second.peer.peer
            )));
        };
        let (winner, loser) = if same_answer(&third.answer, &first.answer) {
            (first, second)
        } else if same_answer(&third.answer, &second.answer) {
            (second, first)
        } else {
            self.mark_disputed(&[third.peer.peer]);
            self.set_last(Verification {
                peers: Vec::new(),
                operators: Vec::new(),
                heights: Vec::new(),
                agreed: false,
                single_operator: false,
                disputed: vec![
                    first.peer.peer.to_string(),
                    second.peer.peer.to_string(),
                    third.peer.peer.to_string(),
                ],
            });
            return Err(ClientError::Disputed(format!(
                "three nodes gave three different answers for {path}"
            )));
        };
        // The one the third agreed with is no longer in dispute.
        if let Ok(mut s) = self.state.lock() {
            s.disputed.remove(&winner.peer.peer);
        }
        self.set_last(Verification {
            peers: vec![winner.peer.peer.to_string(), third.peer.peer.to_string()],
            operators: vec![winner.peer.operator.clone(), third.peer.operator.clone()],
            heights: vec![winner.answer.height, third.answer.height],
            agreed: true,
            single_operator: false,
            disputed: vec![loser.peer.peer.to_string()],
        });
        Ok(to_response(winner.answer))
    }
}

fn to_response(a: ChainAnswer) -> TransportResponse {
    TransportResponse {
        status: a.status,
        body: a.body,
        height: a.height,
    }
}

#[async_trait::async_trait]
impl<B: RelayPeers + 'static> ChainTransport for P2pChainTransport<B> {
    async fn get(&self, path: &str, query: &str) -> Result<TransportResponse, ClientError> {
        self.cross_checked_get(path, query).await
    }

    async fn broadcast(&self, tx_bytes: &[u8]) -> Result<TransportResponse, ClientError> {
        let relays = self.backend.relays().await;
        if relays.is_empty() {
            return Err(ClientError::NoNode(
                "no connected node serves the chain relay (relay or bootstrap role)".into(),
            ));
        }
        let mut last = String::from("no relay answered");
        for p in self.order(relays) {
            match self.backend.broadcast(p.peer, tx_bytes.to_vec()).await {
                Ok(a) => return Ok(to_response(a)),
                Err(e) => {
                    debug!(peer = %p.peer, error = %e, "broadcast relay failed; trying another");
                    last = e.to_string();
                }
            }
        }
        Err(ClientError::NoNode(last))
    }

    fn verification(&self) -> Option<Verification> {
        self.state.lock().ok().and_then(|s| s.last.clone())
    }

    fn describe(&self) -> String {
        "p2p chain relay".into()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::*;
    use hashgram_chain::Client;

    /// A fake network: each peer has an operator and a scripted answer.
    struct Mock {
        peers: Vec<KnownPeer>,
        answers: HashMap<PeerId, Result<ChainAnswer, ()>>,
        calls: Mutex<Vec<PeerId>>,
    }

    fn peer() -> PeerId {
        PeerId::from(hashgram_p2p::Keypair::generate_ed25519().public())
    }

    fn known(p: PeerId, op: &str) -> KnownPeer {
        KnownPeer {
            peer: p,
            roles: vec!["relay".into()],
            operator: op.into(),
        }
    }

    fn ok(body: &str, height: u64) -> Result<ChainAnswer, ()> {
        Ok(ChainAnswer {
            status: 200,
            body: body.as_bytes().to_vec(),
            height,
        })
    }

    #[async_trait::async_trait]
    impl RelayPeers for Mock {
        async fn relays(&self) -> Vec<KnownPeer> {
            self.peers.clone()
        }
        async fn get(&self, peer: PeerId, _p: &str, _q: &str) -> Result<ChainAnswer, LinkError> {
            self.calls.lock().unwrap().push(peer);
            match self.answers.get(&peer) {
                Some(Ok(a)) => Ok(a.clone()),
                _ => Err(LinkError::Unexpected),
            }
        }
        async fn broadcast(&self, peer: PeerId, _tx: Vec<u8>) -> Result<ChainAnswer, LinkError> {
            self.calls.lock().unwrap().push(peer);
            match self.answers.get(&peer) {
                Some(Ok(a)) => Ok(a.clone()),
                _ => Err(LinkError::Unexpected),
            }
        }
    }

    const BAL: &str = r#"{"balance":{"denom":"uhash","amount":"1000000"}}"#;
    const BAL_REORDERED: &str = r#"{"balance":{"amount":"1000000","denom":"uhash"}}"#;
    const LIE: &str = r#"{"balance":{"denom":"uhash","amount":"999999999999"}}"#;

    #[tokio::test]
    async fn two_honest_operators_agree() {
        let (a, b) = (peer(), peer());
        let mock = Arc::new(Mock {
            peers: vec![known(a, "hash1opA"), known(b, "hash1opB")],
            answers: HashMap::from([(a, ok(BAL, 100)), (b, ok(BAL_REORDERED, 101))]),
            calls: Mutex::new(vec![]),
        });
        let t = P2pChainTransport::new(mock.clone());
        let r = t
            .get("cosmos/bank/v1beta1/balances/x/by_denom", "denom=uhash")
            .await
            .unwrap();
        assert_eq!(r.status, 200);
        let v = t.verification().unwrap();
        assert!(v.agreed);
        assert_eq!(v.verified_by(), 2);
        assert!(!v.single_operator);
        assert_eq!(mock.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_lying_peer_is_outvoted_and_marked_disputed() {
        let (a, liar, c) = (peer(), peer(), peer());
        let mock = Arc::new(Mock {
            peers: vec![
                known(a, "hash1opA"),
                known(liar, "hash1opB"),
                known(c, "hash1opC"),
            ],
            answers: HashMap::from([(a, ok(BAL, 100)), (liar, ok(LIE, 100)), (c, ok(BAL, 100))]),
            calls: Mutex::new(vec![]),
        });
        let t = P2pChainTransport::new(mock.clone());
        let r = t
            .get("cosmos/bank/v1beta1/balances/x/by_denom", "denom=uhash")
            .await
            .unwrap();
        assert_eq!(r.body, BAL.as_bytes());
        let v = t.verification().unwrap();
        assert!(v.agreed);
        assert_eq!(v.verified_by(), 2);
        assert_eq!(v.disputed, vec![liar.to_string()]);
        assert_eq!(t.disputed(), vec![liar]);
        // Next read avoids the liar: only the two honest peers are asked.
        mock.calls.lock().unwrap().clear();
        t.get("cosmos/bank/v1beta1/balances/x/by_denom", "denom=uhash")
            .await
            .unwrap();
        let calls = mock.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2);
        assert!(!calls.contains(&liar));
    }

    #[tokio::test]
    async fn two_liars_and_no_third_is_an_error_not_a_guess() {
        let (a, b) = (peer(), peer());
        let mock = Arc::new(Mock {
            peers: vec![known(a, "hash1opA"), known(b, "hash1opB")],
            answers: HashMap::from([(a, ok(BAL, 100)), (b, ok(LIE, 100))]),
            calls: Mutex::new(vec![]),
        });
        let t = P2pChainTransport::new(mock);
        let e = t.get("hashgram/founder/v1/params", "").await.unwrap_err();
        assert!(matches!(e, ClientError::Disputed(_)), "{e}");
        assert_eq!(t.disputed().len(), 2);
    }

    #[tokio::test]
    async fn single_operator_is_reported_not_faked() {
        let (a, b) = (peer(), peer());
        let mock = Arc::new(Mock {
            peers: vec![known(a, "hash1opA"), known(b, "hash1opA")],
            answers: HashMap::from([(a, ok(BAL, 100)), (b, ok(BAL, 100))]),
            calls: Mutex::new(vec![]),
        });
        let t = P2pChainTransport::new(mock.clone());
        t.get("hashgram/founder/v1/params", "").await.unwrap();
        let v = t.verification().unwrap();
        assert!(v.single_operator);
        assert!(!v.agreed);
        assert_eq!(v.verified_by(), 1);
        assert_eq!(mock.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_dead_peer_is_skipped() {
        let (dead, a, b) = (peer(), peer(), peer());
        let mock = Arc::new(Mock {
            peers: vec![
                known(dead, "hash1opX"),
                known(a, "hash1opA"),
                known(b, "hash1opB"),
            ],
            answers: HashMap::from([(a, ok(BAL, 5)), (b, ok(BAL, 7))]),
            calls: Mutex::new(vec![]),
        });
        let t = P2pChainTransport::new(mock);
        t.get("hashgram/founder/v1/params", "").await.unwrap();
        let v = t.verification().unwrap();
        assert!(v.agreed);
        assert_eq!(v.verified_by(), 2);
    }

    #[tokio::test]
    async fn heights_too_far_apart_do_not_count_as_agreement() {
        let (a, b) = (peer(), peer());
        let mock = Arc::new(Mock {
            peers: vec![known(a, "hash1opA"), known(b, "hash1opB")],
            answers: HashMap::from([(a, ok(BAL, 100)), (b, ok(BAL, 200))]),
            calls: Mutex::new(vec![]),
        });
        let t = P2pChainTransport::new(mock);
        // Same body, far-apart heights: no third to break the tie → disputed.
        let e = t.get("hashgram/founder/v1/params", "").await.unwrap_err();
        assert!(matches!(e, ClientError::Disputed(_)));
    }

    #[tokio::test]
    async fn no_relay_peers_is_a_clear_error() {
        let mock = Arc::new(Mock {
            peers: vec![],
            answers: HashMap::new(),
            calls: Mutex::new(vec![]),
        });
        let t = P2pChainTransport::new(mock);
        let e = t.get("hashgram/founder/v1/params", "").await.unwrap_err();
        assert!(matches!(e, ClientError::NoNode(_)));
    }

    #[tokio::test]
    async fn the_chain_client_reads_a_balance_through_the_relay() {
        let (a, b) = (peer(), peer());
        let mock = Arc::new(Mock {
            peers: vec![known(a, "hash1opA"), known(b, "hash1opB")],
            answers: HashMap::from([(a, ok(BAL, 100)), (b, ok(BAL, 100))]),
            calls: Mutex::new(vec![]),
        });
        let client = Client::over(Arc::new(P2pChainTransport::new(mock)), "hashgram-1");
        assert_eq!(client.balance("hash1whoever").await.unwrap(), 1_000_000);
        assert!(client.verification().unwrap().agreed);
        assert!(!client.transport().can_simulate());
    }

    #[test]
    fn latest_block_is_not_compared() {
        assert!(!comparable("cosmos/base/tendermint/v1beta1/blocks/latest"));
        assert!(comparable("cosmos/bank/v1beta1/supply/by_denom"));
    }
}

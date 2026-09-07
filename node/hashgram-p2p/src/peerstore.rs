//! The persistent peerstore.
//!
//! Remembers peers that completed the handshake, with their addresses and
//! when they were last seen, so a restart reconnects to the network it was
//! part of rather than starting again from the bundled bootstrap list. Every
//! restart that starts from the bundled list is a fresh dependence on whoever
//! built the release; this file is what removes it.
//!
//! The store is a JSON file rewritten atomically. It holds public
//! information only — peer ids and addresses — so its permissions matter for
//! integrity, not confidentiality: a writable peerstore is a way to steer a
//! node toward attacker peers on restart. It is written 0600 regardless.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use libp2p::{Multiaddr, PeerId};
use serde::{Deserialize, Serialize};

/// Most peers kept. Above this, the least recently seen are dropped.
pub const MAX_PEERS: usize = 1000;

/// Most addresses kept per peer.
pub const MAX_ADDRS_PER_PEER: usize = 8;

/// One remembered peer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerRecord {
    /// Addresses, most recently confirmed first.
    pub addrs: Vec<String>,
    /// Unix seconds of the last completed handshake.
    pub last_seen: u64,
    /// Consecutive dial failures since the last success.
    pub failures: u32,
    /// Roles the peer claimed at its last handshake. Informational.
    #[serde(default)]
    pub roles: Vec<String>,
}

/// The store.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Peerstore {
    #[serde(default)]
    peers: HashMap<String, PeerRecord>,
    #[serde(skip)]
    path: Option<PathBuf>,
    #[serde(skip)]
    dirty: bool,
}

/// Why the store could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum PeerstoreError {
    /// Filesystem.
    #[error("peerstore {path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The error.
        source: std::io::Error,
    },
    /// Not JSON we understand. The store is discarded rather than trusted
    /// halfway: a partially parsed peer list is not better than none.
    #[error("peerstore {path} is not valid: {source}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// The error.
        source: serde_json::Error,
    },
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Peerstore {
    /// An empty in-memory store that never persists.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Loads the store from `path`, or starts empty if it does not exist.
    pub fn load(path: &Path) -> Result<Self, PeerstoreError> {
        let mut store = match std::fs::read(path) {
            Ok(raw) => {
                serde_json::from_slice::<Self>(&raw).map_err(|source| PeerstoreError::Parse {
                    path: path.to_path_buf(),
                    source,
                })?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(source) => {
                return Err(PeerstoreError::Io {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        store.path = Some(path.to_path_buf());
        store.dirty = false;
        Ok(store)
    }

    /// Writes the store atomically if anything changed.
    pub fn save(&mut self) -> Result<(), PeerstoreError> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        if !self.dirty {
            return Ok(());
        }
        let io = |source| PeerstoreError::Io {
            path: path.clone(),
            source,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let tmp = path.with_extension("json.tmp");
        let raw = serde_json::to_vec_pretty(self).map_err(|source| PeerstoreError::Parse {
            path: path.clone(),
            source,
        })?;
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)
                .map_err(io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                f.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .map_err(io)?;
            }
            f.write_all(&raw).map_err(io)?;
            f.sync_all().map_err(io)?;
        }
        std::fs::rename(&tmp, &path).map_err(io)?;
        self.dirty = false;
        Ok(())
    }

    /// Records a successful handshake with a peer at an address.
    pub fn record_success(&mut self, peer: &PeerId, addr: Option<&Multiaddr>, roles: &[String]) {
        let key = peer.to_string();
        let rec = self.peers.entry(key).or_insert_with(|| PeerRecord {
            addrs: Vec::new(),
            last_seen: 0,
            failures: 0,
            roles: Vec::new(),
        });
        rec.last_seen = now();
        rec.failures = 0;
        rec.roles = roles.to_vec();
        if let Some(addr) = addr {
            let s = strip_p2p(addr);
            rec.addrs.retain(|a| a != &s);
            rec.addrs.insert(0, s);
            rec.addrs.truncate(MAX_ADDRS_PER_PEER);
        }
        self.dirty = true;
        self.evict();
    }

    /// Records a dial failure. A peer that fails enough times is forgotten.
    pub fn record_failure(&mut self, peer: &PeerId) {
        let key = peer.to_string();
        if let Some(rec) = self.peers.get_mut(&key) {
            rec.failures = rec.failures.saturating_add(1);
            self.dirty = true;
            if rec.failures >= 10 {
                self.peers.remove(&key);
            }
        }
    }

    /// Forgets a peer, for example after a ban.
    pub fn forget(&mut self, peer: &PeerId) {
        if self.peers.remove(&peer.to_string()).is_some() {
            self.dirty = true;
        }
    }

    /// Dial candidates: most recently seen first, fewest failures first,
    /// each as a full multiaddr with the `/p2p/` component.
    #[must_use]
    pub fn candidates(&self, limit: usize) -> Vec<(PeerId, Multiaddr)> {
        let mut entries: Vec<(&String, &PeerRecord)> = self.peers.iter().collect();
        entries.sort_by(|a, b| {
            a.1.failures
                .cmp(&b.1.failures)
                .then(b.1.last_seen.cmp(&a.1.last_seen))
        });
        let mut out = Vec::new();
        for (id, rec) in entries {
            let Ok(peer) = id.parse::<PeerId>() else {
                continue;
            };
            for addr in &rec.addrs {
                let Ok(mut ma) = addr.parse::<Multiaddr>() else {
                    continue;
                };
                ma.push(libp2p::multiaddr::Protocol::P2p(peer));
                out.push((peer, ma));
                if out.len() >= limit {
                    return out;
                }
            }
        }
        out
    }

    /// Addresses for peer exchange: verified peers with at least one
    /// address, as full multiaddrs.
    #[must_use]
    pub fn exchange_addrs(&self, limit: usize) -> Vec<String> {
        self.candidates(limit)
            .into_iter()
            .map(|(_, a)| a.to_string())
            .collect()
    }

    /// How many peers are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether nothing is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    fn evict(&mut self) {
        if self.peers.len() <= MAX_PEERS {
            return;
        }
        let mut entries: Vec<(String, u64)> = self
            .peers
            .iter()
            .map(|(k, v)| (k.clone(), v.last_seen))
            .collect();
        entries.sort_by_key(|(_, seen)| *seen);
        let excess = self.peers.len() - MAX_PEERS;
        for (k, _) in entries.into_iter().take(excess) {
            self.peers.remove(&k);
        }
    }
}

/// Removes a trailing `/p2p/<id>` so addresses are stored once per
/// transport and the id is kept as the map key.
fn strip_p2p(addr: &Multiaddr) -> String {
    let mut a = addr.clone();
    if matches!(a.iter().last(), Some(libp2p::multiaddr::Protocol::P2p(_))) {
        a.pop();
    }
    a.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> PeerId {
        PeerId::random()
    }

    #[test]
    fn success_records_address_and_candidates_include_peer_id() {
        let mut s = Peerstore::in_memory();
        let p = peer();
        let addr: Multiaddr = "/ip4/10.0.0.1/tcp/26670".parse().unwrap();
        s.record_success(&p, Some(&addr), &["relay".into()]);
        let c = s.candidates(10);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].0, p);
        assert!(c[0].1.to_string().ends_with(&format!("/p2p/{p}")));
    }

    #[test]
    fn p2p_suffix_is_not_duplicated() {
        let mut s = Peerstore::in_memory();
        let p = peer();
        let addr: Multiaddr = format!("/ip4/10.0.0.1/tcp/26670/p2p/{p}").parse().unwrap();
        s.record_success(&p, Some(&addr), &[]);
        let c = s.candidates(10);
        assert_eq!(
            c[0].1
                .iter()
                .filter(|x| matches!(x, libp2p::multiaddr::Protocol::P2p(_)))
                .count(),
            1
        );
    }

    #[test]
    fn repeated_failures_forget_a_peer() {
        let mut s = Peerstore::in_memory();
        let p = peer();
        s.record_success(&p, Some(&"/ip4/10.0.0.1/tcp/1".parse().unwrap()), &[]);
        for _ in 0..10 {
            s.record_failure(&p);
        }
        assert!(s.is_empty());
    }

    #[test]
    fn eviction_keeps_the_most_recent() {
        let mut s = Peerstore::in_memory();
        for i in 0..(MAX_PEERS + 5) {
            let p = peer();
            s.record_success(
                &p,
                Some(
                    &format!("/ip4/10.0.{}.{}/tcp/1", i >> 8, i & 0xff)
                        .parse()
                        .unwrap(),
                ),
                &[],
            );
        }
        assert_eq!(s.len(), MAX_PEERS);
    }

    #[test]
    fn persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("hg-peerstore-{}", std::process::id()));
        let path = dir.join("peerstore.json");
        let p = peer();
        {
            let mut s = Peerstore::load(&path).unwrap();
            s.record_success(
                &p,
                Some(&"/ip4/10.0.0.1/udp/26670/quic-v1".parse().unwrap()),
                &["store".into()],
            );
            s.save().unwrap();
        }
        let s = Peerstore::load(&path).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s.candidates(1)[0].0, p);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_store_is_refused_not_half_trusted() {
        let dir = std::env::temp_dir().join(format!("hg-peerstore-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("peerstore.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(matches!(
            Peerstore::load(&path),
            Err(PeerstoreError::Parse { .. })
        ));
        let _ = std::fs::remove_dir_all(dir);
    }
}

//! The node's P2P identity key.
//!
//! One ed25519 key, stored in libp2p's protobuf encoding at
//! `<data_dir>/node_key`, mode 0600. It identifies this node to peers and
//! signs its announcements. It is **not** a consensus key, an operator key, a
//! reward key or a user key: losing it costs this node its peer id and its
//! reputation, and nothing else. That is by design, and it is why the P2P
//! daemon runs as a different Unix user from the chain node.

use std::path::Path;

use anyhow::Context;
use hashgram_p2p::Keypair;
use hashgram_proto::Ed25519Signer;

/// Loads the node key, creating it if absent.
pub fn load_or_create(data_dir: &Path) -> anyhow::Result<Keypair> {
    let path = data_dir.join("node_key");
    match std::fs::read(&path) {
        Ok(bytes) => Keypair::from_protobuf_encoding(&bytes)
            .with_context(|| format!("{} is not a valid node key", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(data_dir)
                .with_context(|| format!("creating {}", data_dir.display()))?;
            let key = Keypair::generate_ed25519();
            let bytes = key.to_protobuf_encoding().context("encoding node key")?;
            write_secret(&path, &bytes)?;
            tracing::info!(path = %path.display(), "generated a new node key");
            Ok(key)
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Writes a secret file with 0600 permissions, atomically.
pub fn write_secret(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

/// The node key as a Hashgram signer, for announcements.
///
/// libp2p keeps the ed25519 secret behind its own type; the announcement
/// signer needs the raw 32-byte seed. `to_bytes` on the ed25519 keypair
/// yields the 64-byte secret||public form, of which the first half is the
/// seed.
pub fn announce_signer(key: &Keypair) -> anyhow::Result<Ed25519Signer> {
    let ed = key
        .clone()
        .try_into_ed25519()
        .map_err(|_| anyhow::anyhow!("node key is not ed25519"))?;
    let bytes = ed.to_bytes();
    let seed: [u8; 32] = bytes
        .get(..32)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| anyhow::anyhow!("ed25519 keypair encoding is not 64 bytes"))?;
    Ok(Ed25519Signer::from_secret(seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_then_reloads_the_same_key() {
        let d = std::env::temp_dir().join(format!("hg-nodekey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let a = load_or_create(&d).unwrap();
        let b = load_or_create(&d).unwrap();
        assert_eq!(a.public(), b.public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(d.join("node_key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn announce_signer_matches_the_libp2p_public_key() {
        let key = Keypair::generate_ed25519();
        let signer = announce_signer(&key).unwrap();
        let libp2p_raw = key.public().try_into_ed25519().unwrap().to_bytes();
        assert_eq!(signer.public_key(), libp2p_raw);
        // And the libp2p envelope round-trips through hashgram-proto.
        let env = key.public().encode_protobuf();
        assert_eq!(
            hashgram_proto::keys::ed25519_from_libp2p(&env).unwrap(),
            libp2p_raw
        );
    }
}

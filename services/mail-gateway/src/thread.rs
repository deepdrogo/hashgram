//! Threading across the two worlds.
//!
//! HashMail identifies messages by 16 random bytes; Internet mail by the
//! `Message-ID` header. Neither side can be told to use the other's ids,
//! so the gateway *derives* a 16-byte id from a header deterministically
//! (`blake3(normalised header)[..16]`) and remembers every pairing it has
//! seen in the `thread_map` table. Determinism matters twice:
//!
//! * the same Internet message delivered twice (two MX attempts, two
//!   recipients on separate connections) yields the same HashMail
//!   `message_id`, so the SDK's dedup files it once;
//! * a reply from the Internet quoting `In-Reply-To: <x@example.com>`
//!   maps to the same id we filed `<x@example.com>` under, even if the
//!   `thread_map` row was lost — the map is an optimisation for ids we
//!   *generated* (outbound `<hex@domain>`), which cannot be re-derived.
//!
//! Outbound: a native message with id `H` leaves as
//! `Message-ID: <hex(H)@domain>`; replies carrying that header map back to
//! `H` through the same normalisation.

/// Bytes in a HashMail id.
pub const ID_LEN: usize = 16;

/// Normalises a `Message-ID`-like header value: trims, strips one pair of
/// angle brackets, lowercases the domain part (RFC 5322 treats the whole id
/// as opaque, but real senders vary the case of the domain across hops).
#[must_use]
pub fn normalise_message_id(raw: &str) -> String {
    let s = raw.trim();
    let s = s
        .strip_prefix('<')
        .and_then(|x| x.strip_suffix('>'))
        .unwrap_or(s);
    match s.rsplit_once('@') {
        Some((l, d)) => format!("{l}@{}", d.to_ascii_lowercase()),
        None => s.to_owned(),
    }
}

/// Derives the 16-byte id for a header value. Empty input yields `None`
/// rather than a fixed id, so a missing header never threads unrelated
/// messages together.
#[must_use]
pub fn derive_id(header: &str) -> Option<Vec<u8>> {
    let n = normalise_message_id(header);
    if n.is_empty() {
        return None;
    }
    let h = blake3::hash(n.as_bytes());
    Some(h.as_bytes().get(..ID_LEN)?.to_vec())
}

/// Splits a `References:` value into individual ids (tokens in angle
/// brackets, whitespace separated), most distant ancestor first.
#[must_use]
pub fn split_references(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find('<') {
        let after = rest.get(start + 1..).unwrap_or("");
        let Some(end) = after.find('>') else { break };
        if let Some(id) = after.get(..end) {
            let id = id.trim();
            if !id.is_empty() {
                out.push(normalise_message_id(id));
            }
        }
        rest = after.get(end + 1..).unwrap_or("");
    }
    out
}

/// The `Message-ID` value the gateway emits for a native message id.
#[must_use]
pub fn outbound_message_id(hashgram_id: &[u8], domain: &str) -> String {
    format!("<{}@{}>", hex::encode(hashgram_id), domain)
}

/// If a header value is one of ours (`<32 hex@our domain>`), the native
/// id it encodes. Lets replies be threaded even when the `thread_map` row
/// is gone.
#[must_use]
pub fn parse_outbound_message_id(header: &str, domain: &str) -> Option<Vec<u8>> {
    let n = normalise_message_id(header);
    let (l, d) = n.rsplit_once('@')?;
    if !d.eq_ignore_ascii_case(domain) || l.len() != ID_LEN * 2 {
        return None;
    }
    hex::decode(l).ok()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn derivation_is_stable_and_normalised() {
        let a = derive_id("<abc@Example.com>").unwrap();
        let b = derive_id("  abc@example.COM ").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
        assert_ne!(a, derive_id("<abd@example.com>").unwrap());
        assert!(derive_id("").is_none());
        assert!(derive_id("<>").is_none());
        // Known answer so a future change to the derivation is noticed.
        assert_eq!(
            hex::encode(&a),
            hex::encode(&blake3::hash(b"abc@example.com").as_bytes()[..16])
        );
    }

    #[test]
    fn references_split() {
        let r = split_references("<a@x.org>\r\n <b@Y.org>   <c@z.org>");
        assert_eq!(r, vec!["a@x.org", "b@y.org", "c@z.org"]);
        assert!(split_references("garbage").is_empty());
    }

    #[test]
    fn outbound_ids_round_trip() {
        let id = vec![7u8; 16];
        let h = outbound_message_id(&id, "hashgram.io");
        assert_eq!(h, format!("<{}@hashgram.io>", "07".repeat(16)));
        assert_eq!(parse_outbound_message_id(&h, "hashgram.io").unwrap(), id);
        assert!(parse_outbound_message_id(&h, "other.io").is_none());
        assert!(parse_outbound_message_id("<zz@hashgram.io>", "hashgram.io").is_none());
    }
}

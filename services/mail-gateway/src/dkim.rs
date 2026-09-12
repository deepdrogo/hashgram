//! DKIM (RFC 6376) signing with relaxed/relaxed canonicalisation.
//!
//! Written in-tree rather than pulled from a crate: the algorithm is a
//! few pages of RFC, every step is a known-answer test away from being
//! verified, and a signing bug here silently makes all outbound mail look
//! forged — the reviewer should be able to read the whole thing.
//!
//! Two algorithms:
//!
//! * `rsa-sha256` (§3.3.3) behind the `dkim-rsa` feature (the `rsa` crate);
//!   what every large receiver verifies today.
//! * `ed25519-sha256` (RFC 8463) always available on `ed25519-dalek`,
//!   already in the workspace. Not yet verified by all receivers, so
//!   operators should publish an RSA key too until it is.
//!
//! What is signed: the canonicalised body hash (`bh=`) and the selected
//! headers in `h=` order, followed by the canonicalised `DKIM-Signature`
//! header with an empty `b=`. Headers are taken from the *rendered bytes*,
//! not from the structure that produced them, so what is signed is
//! exactly what leaves.

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::GatewayError;

/// Headers we sign when present, in this order. `from` is mandatory
/// (§5.4); the rest are the ones whose alteration would change meaning or
/// threading.
pub const SIGNED_HEADERS: &[&str] = &[
    "from",
    "to",
    "cc",
    "reply-to",
    "subject",
    "date",
    "message-id",
    "in-reply-to",
    "references",
    "mime-version",
    "content-type",
    "content-transfer-encoding",
];

/// A loaded private key.
pub enum SigningKey {
    /// RFC 8463.
    Ed25519(Box<ed25519_dalek::SigningKey>),
    /// RFC 6376 §3.3.3.
    #[cfg(feature = "dkim-rsa")]
    Rsa(Box<rsa::RsaPrivateKey>),
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Ed25519(_) => "SigningKey::Ed25519",
            #[cfg(feature = "dkim-rsa")]
            Self::Rsa(_) => "SigningKey::Rsa",
        })
    }
}

/// The `a=` tag value.
impl SigningKey {
    /// Algorithm name for the `a=` tag.
    #[must_use]
    pub fn algorithm(&self) -> &'static str {
        match self {
            Self::Ed25519(_) => "ed25519-sha256",
            #[cfg(feature = "dkim-rsa")]
            Self::Rsa(_) => "rsa-sha256",
        }
    }

    /// Parses a key file: PKCS#8 PEM (`PRIVATE KEY`, RSA or Ed25519),
    /// PKCS#1 PEM (`RSA PRIVATE KEY`), or a raw Ed25519 seed as 64 hex or
    /// 44 base64 characters.
    pub fn parse(text: &str) -> Result<Self, GatewayError> {
        let t = text.trim();
        if let Some(der) = pem_body(t, "PRIVATE KEY") {
            if let Some(seed) = ed25519_seed_from_pkcs8(&der) {
                return Ok(Self::Ed25519(Box::new(
                    ed25519_dalek::SigningKey::from_bytes(&seed),
                )));
            }
            #[cfg(feature = "dkim-rsa")]
            {
                use rsa::pkcs8::DecodePrivateKey;
                let k = rsa::RsaPrivateKey::from_pkcs8_der(&der)
                    .map_err(|e| GatewayError::Dkim(format!("PKCS#8 key: {e}")))?;
                return Ok(Self::Rsa(Box::new(k)));
            }
            #[cfg(not(feature = "dkim-rsa"))]
            return Err(GatewayError::Dkim(
                "PKCS#8 key is not Ed25519 and this build has no RSA support (feature dkim-rsa)"
                    .into(),
            ));
        }
        if let Some(der) = pem_body(t, "RSA PRIVATE KEY") {
            #[cfg(feature = "dkim-rsa")]
            {
                use rsa::pkcs1::DecodeRsaPrivateKey;
                let k = rsa::RsaPrivateKey::from_pkcs1_der(&der)
                    .map_err(|e| GatewayError::Dkim(format!("PKCS#1 key: {e}")))?;
                return Ok(Self::Rsa(Box::new(k)));
            }
            #[cfg(not(feature = "dkim-rsa"))]
            {
                let _ = der;
                return Err(GatewayError::Dkim(
                    "RSA key but this build has no RSA support (feature dkim-rsa)".into(),
                ));
            }
        }
        if t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            let bytes = hex::decode(t).map_err(|e| GatewayError::Dkim(e.to_string()))?;
            let seed: [u8; 32] = bytes
                .try_into()
                .map_err(|_| GatewayError::Dkim("seed length".into()))?;
            return Ok(Self::Ed25519(Box::new(
                ed25519_dalek::SigningKey::from_bytes(&seed),
            )));
        }
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(t) {
            if bytes.len() == 32 {
                let seed: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| GatewayError::Dkim("seed length".into()))?;
                return Ok(Self::Ed25519(Box::new(
                    ed25519_dalek::SigningKey::from_bytes(&seed),
                )));
            }
        }
        Err(GatewayError::Dkim(
            "unrecognised key format (want PKCS#8/PKCS#1 PEM or a raw 32-byte Ed25519 seed)".into(),
        ))
    }

    /// The DNS TXT record value to publish at `<selector>._domainkey.<domain>`.
    pub fn dns_record(&self) -> Result<String, GatewayError> {
        match self {
            Self::Ed25519(k) => Ok(format!(
                "v=DKIM1; k=ed25519; p={}",
                base64::engine::general_purpose::STANDARD.encode(k.verifying_key().as_bytes())
            )),
            #[cfg(feature = "dkim-rsa")]
            Self::Rsa(k) => {
                use rsa::pkcs8::EncodePublicKey;
                let der = k
                    .to_public_key()
                    .to_public_key_der()
                    .map_err(|e| GatewayError::Dkim(e.to_string()))?;
                Ok(format!(
                    "v=DKIM1; k=rsa; p={}",
                    base64::engine::general_purpose::STANDARD.encode(der.as_bytes())
                ))
            }
        }
    }

    /// Signs the canonicalised data (RFC 6376 §3.7).
    fn sign_data(&self, data: &[u8]) -> Result<Vec<u8>, GatewayError> {
        match self {
            Self::Ed25519(k) => {
                // RFC 8463 §3: PureEdDSA over the SHA-256 digest.
                use ed25519_dalek::Signer;
                let digest = Sha256::digest(data);
                Ok(k.sign(&digest).to_bytes().to_vec())
            }
            #[cfg(feature = "dkim-rsa")]
            Self::Rsa(k) => {
                use rsa::signature::{SignatureEncoding, Signer};
                let sk = rsa::pkcs1v15::SigningKey::<Sha256>::new(*k.clone());
                Ok(sk.sign(data).to_vec())
            }
        }
    }
}

/// PEM body for a label, DER-decoded.
fn pem_body(text: &str, label: &str) -> Option<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let start = text.find(&begin)? + begin.len();
    let stop = text.get(start..)?.find(&end)? + start;
    let b64: String = text
        .get(start..stop)?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

/// RFC 8410 PKCS#8 v1 Ed25519 keys are a fixed 48-byte structure:
/// `30 2e 02 01 00 30 05 06 03 2b 65 70 04 22 04 20 || seed`. Anything
/// else (v2 with public key appended, other OIDs) returns `None` and falls
/// through to the RSA parser.
fn ed25519_seed_from_pkcs8(der: &[u8]) -> Option<[u8; 32]> {
    const PREFIX: [u8; 16] = [
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    if der.len() < 48 || der.get(..16)? != PREFIX {
        // Also accept v2 (0x30 0x51 …) by locating the OID and seed.
        let oid_at = der.windows(3).position(|w| w == [0x2b, 0x65, 0x70])?;
        let rest = der.get(oid_at + 3..)?;
        let seed_at = rest
            .windows(4)
            .position(|w| w == [0x04, 0x22, 0x04, 0x20])?;
        let seed = rest.get(seed_at + 4..seed_at + 36)?;
        return seed.try_into().ok();
    }
    der.get(16..48)?.try_into().ok()
}

// ---------------------------------------------------------------------------
// Canonicalisation (§3.4)
// ---------------------------------------------------------------------------

/// Splits a message into (raw header fields, body). Header fields are
/// returned unfolded-as-is (continuation lines still attached) so both
/// canonicalisations can be derived from them.
#[must_use]
pub fn split_message(msg: &[u8]) -> (Vec<Vec<u8>>, &[u8]) {
    let mut headers = Vec::new();
    let mut pos = 0;
    let mut current: Vec<u8> = Vec::new();
    loop {
        let rest = msg.get(pos..).unwrap_or(&[]);
        let Some(nl) = rest.iter().position(|&b| b == b'\n') else {
            // Headers without a body separator: whole thing is headers.
            if !rest.is_empty() {
                if !current.is_empty() {
                    headers.push(std::mem::take(&mut current));
                }
                headers.push(rest.to_vec());
            } else if !current.is_empty() {
                headers.push(current);
            }
            return (headers, &[]);
        };
        let line = rest.get(..nl).unwrap_or(&[]);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        pos += nl + 1;
        if line.is_empty() {
            if !current.is_empty() {
                headers.push(current);
            }
            return (headers, msg.get(pos..).unwrap_or(&[]));
        }
        if line.first().is_some_and(|b| *b == b' ' || *b == b'\t') && !current.is_empty() {
            current.extend_from_slice(b"\r\n");
            current.extend_from_slice(line);
        } else {
            if !current.is_empty() {
                headers.push(std::mem::take(&mut current));
            }
            current.extend_from_slice(line);
        }
    }
}

fn is_wsp(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Collapses WSP runs to one SP and trims both ends.
fn collapse_wsp(v: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len());
    let mut in_ws = false;
    for &b in v {
        if is_wsp(b) {
            in_ws = true;
        } else {
            if in_ws && !out.is_empty() {
                out.push(b' ');
            }
            in_ws = false;
            out.push(b);
        }
    }
    out
}

/// Relaxed header canonicalisation (§3.4.2) of one raw field. Returns the
/// lowercase name and `name:value\r\n`.
#[must_use]
pub fn relaxed_header(raw: &[u8]) -> Option<(String, Vec<u8>)> {
    let colon = raw.iter().position(|&b| b == b':')?;
    let name = raw.get(..colon)?;
    let name: Vec<u8> = name
        .iter()
        .filter(|b| !is_wsp(**b))
        .map(u8::to_ascii_lowercase)
        .collect();
    let name = String::from_utf8(name).ok()?;
    // Unfold: remove CRLF, keep the WSP that followed (it collapses).
    let value: Vec<u8> = raw
        .get(colon + 1..)?
        .iter()
        .copied()
        .filter(|&b| b != b'\r' && b != b'\n')
        .collect();
    let value = collapse_wsp(&value);
    let mut out = name.clone().into_bytes();
    out.push(b':');
    out.extend_from_slice(&value);
    out.extend_from_slice(b"\r\n");
    Some((name, out))
}

/// Relaxed body canonicalisation (§3.4.4).
#[must_use]
pub fn relaxed_body(body: &[u8]) -> Vec<u8> {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let (line, consumed) = match rest.iter().position(|&b| b == b'\n') {
            Some(i) => (rest.get(..i).unwrap_or(&[]), i + 1),
            None => (rest, rest.len()),
        };
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        // Trailing WSP off, internal runs to one SP, leading WSP kept as
        // a single SP (the collapse keeps a leading run as one SP because
        // `out` is empty at that point — see collapse_wsp — so handle it
        // explicitly here).
        let trimmed_end = {
            let mut e = line.len();
            while e > 0 && line.get(e - 1).is_some_and(|b| is_wsp(*b)) {
                e -= 1;
            }
            line.get(..e).unwrap_or(&[])
        };
        let leading = trimmed_end.first().is_some_and(|b| is_wsp(*b));
        let mut canon = collapse_wsp(trimmed_end);
        if leading {
            canon.insert(0, b' ');
        }
        lines.push(canon);
        rest = rest.get(consumed..).unwrap_or(&[]);
    }
    while lines.last().is_some_and(Vec::is_empty) {
        lines.pop();
    }
    let mut out = Vec::with_capacity(body.len());
    for l in &lines {
        out.extend_from_slice(l);
        out.extend_from_slice(b"\r\n");
    }
    out
}

// ---------------------------------------------------------------------------
// Signing
// ---------------------------------------------------------------------------

/// A configured signer.
#[derive(Debug)]
pub struct Signer {
    /// `d=`.
    pub domain: String,
    /// `s=`.
    pub selector: String,
    /// The key.
    pub key: SigningKey,
}

/// The pieces of a signature, exposed so tests can check each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInput {
    /// `h=` value.
    pub signed_headers: Vec<String>,
    /// `bh=` value.
    pub body_hash_b64: String,
    /// The full byte string that is hashed and signed.
    pub data: Vec<u8>,
    /// The `DKIM-Signature` header text (folded) with an empty `b=`.
    pub header_without_signature: String,
}

impl Signer {
    /// Builds the signing input for a rendered message (CRLF lines).
    pub fn signature_input(
        &self,
        message: &[u8],
        timestamp: u64,
    ) -> Result<SignatureInput, GatewayError> {
        let (raw_headers, body) = split_message(message);
        let canon: Vec<(String, Vec<u8>)> = raw_headers
            .iter()
            .filter_map(|h| relaxed_header(h))
            .collect();
        if !canon.iter().any(|(n, _)| n == "from") {
            return Err(GatewayError::Dkim("message has no From header".into()));
        }
        // §5.4.2: when a header appears more than once, sign from the
        // bottom up. Our renderer emits each at most once; if it ever
        // does not, the last instance is the one signed here.
        let mut signed_headers = Vec::new();
        let mut header_data = Vec::new();
        for want in SIGNED_HEADERS {
            if let Some((_, c)) = canon.iter().rev().find(|(n, _)| n == want) {
                signed_headers.push((*want).to_owned());
                header_data.extend_from_slice(c);
            }
        }
        let body_hash_b64 =
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(relaxed_body(body)));
        let header_without_signature =
            self.header_text(&signed_headers, &body_hash_b64, timestamp, "");
        let mut raw_sig_header = b"DKIM-Signature:".to_vec();
        raw_sig_header.extend_from_slice(header_without_signature.as_bytes());
        let (_, mut canon_sig) = relaxed_header(&raw_sig_header)
            .ok_or_else(|| GatewayError::Dkim("could not canonicalise own header".into()))?;
        // §3.7: the DKIM-Signature header is included without its trailing CRLF.
        canon_sig.truncate(canon_sig.len().saturating_sub(2));
        let mut data = header_data;
        data.extend_from_slice(&canon_sig);
        Ok(SignatureInput {
            signed_headers,
            body_hash_b64,
            data,
            header_without_signature,
        })
    }

    fn header_text(&self, signed_headers: &[String], bh: &str, timestamp: u64, b: &str) -> String {
        let mut s = format!(
            " v=1; a={}; c=relaxed/relaxed; d={}; s={}; t={};\r\n\th={};\r\n\tbh={};\r\n\tb=",
            self.key.algorithm(),
            self.domain,
            self.selector,
            timestamp,
            signed_headers.join(":"),
            bh
        );
        // Fold the signature in 72-character chunks.
        let bytes = b.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let end = (i + 72).min(bytes.len());
            if let Ok(chunk) = std::str::from_utf8(bytes.get(i..end).unwrap_or(&[])) {
                if i > 0 {
                    s.push_str("\r\n\t");
                }
                s.push_str(chunk);
            }
            i = end;
        }
        s
    }

    /// Produces the complete `DKIM-Signature: …\r\n` header line to
    /// prepend to `message`.
    pub fn sign(&self, message: &[u8], timestamp: u64) -> Result<String, GatewayError> {
        let input = self.signature_input(message, timestamp)?;
        let sig = self.key.sign_data(&input.data)?;
        let b = base64::engine::general_purpose::STANDARD.encode(sig);
        Ok(format!(
            "DKIM-Signature:{}\r\n",
            self.header_text(&input.signed_headers, &input.body_hash_b64, timestamp, &b)
        ))
    }

    /// Signs and returns the message with the header prepended.
    pub fn sign_message(&self, message: &[u8], timestamp: u64) -> Result<Vec<u8>, GatewayError> {
        let header = self.sign(message, timestamp)?;
        let mut out = header.into_bytes();
        out.extend_from_slice(message);
        Ok(out)
    }
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

    /// RFC 6376 §3.4.5 Example 1, with `<SP>`/`<HTAB>`/`<CRLF>` expanded.
    const RFC_EXAMPLE: &[u8] = b"A: X\r\nB : Y\t\r\n\tZ  \r\n\r\n C \r\nD \t E\r\n\r\n\r\n";

    #[test]
    fn rfc6376_relaxed_known_answer() {
        let (headers, body) = split_message(RFC_EXAMPLE);
        assert_eq!(headers.len(), 2);
        let a = relaxed_header(&headers[0]).unwrap();
        let b = relaxed_header(&headers[1]).unwrap();
        assert_eq!(a.0, "a");
        assert_eq!(a.1, b"a:X\r\n");
        assert_eq!(b.0, "b");
        assert_eq!(b.1, b"b:Y Z\r\n");
        assert_eq!(relaxed_body(body), b" C\r\nD E\r\n");
    }

    #[test]
    fn body_canonicalisation_edges() {
        assert_eq!(relaxed_body(b""), b"");
        assert_eq!(relaxed_body(b"\r\n\r\n"), b"");
        assert_eq!(relaxed_body(b"x"), b"x\r\n");
        assert_eq!(
            relaxed_body(b"a  b\t\tc \r\n\r\nd\r\n\r\n\r\n"),
            b"a b c\r\n\r\nd\r\n"
        );
        assert_eq!(relaxed_body(b"  lead\r\n"), b" lead\r\n");
        // §3.4.4 body hash of the empty body is the well-known value.
        let bh =
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(relaxed_body(b"")));
        assert_eq!(bh, "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=");
    }

    #[test]
    fn header_split_and_unfold() {
        let (h, body) = split_message(b"Subject: a\r\n b\r\nX-Y :  z  \r\n\r\nbody\r\n");
        assert_eq!(h.len(), 2);
        assert_eq!(relaxed_header(&h[0]).unwrap().1, b"subject:a b\r\n");
        assert_eq!(relaxed_header(&h[1]).unwrap().1, b"x-y:z\r\n");
        assert_eq!(body, b"body\r\n");
        let (h, body) = split_message(b"Only: headers");
        assert_eq!(h.len(), 1);
        assert!(body.is_empty());
    }

    const ED_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIBMu+hOqTah+gfOTib0GRwr4zoSYwQ8VC492UKuvZj4M\n-----END PRIVATE KEY-----\n";

    const MESSAGE: &[u8] = b"From: alice@hashgram.io\r\nTo: bob@example.com\r\nSubject: test\r\nDate: Sat, 12 Sep 2026 18:25:00 +0000\r\nMessage-ID: <x@hashgram.io>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nX-Unsigned: whatever\r\n\r\nHello \r\n";

    fn signer(key: SigningKey) -> Signer {
        Signer {
            domain: "hashgram.io".into(),
            selector: "s1".into(),
            key,
        }
    }

    /// A minimal verifier: re-derives the signing input from the signed
    /// message exactly as a receiver would (strip `b=`, canonicalise).
    fn reconstruct(signed: &[u8]) -> (Vec<u8>, Vec<u8>, String) {
        let (headers, body) = split_message(signed);
        let sig_raw = headers
            .iter()
            .find(|h| h.to_ascii_lowercase().starts_with(b"dkim-signature:"))
            .unwrap();
        let (_, canon_sig) = relaxed_header(sig_raw).unwrap();
        let canon_str = String::from_utf8(canon_sig).unwrap();
        // `; b=` cannot occur inside base64, unlike a bare `b=`.
        let b_at = canon_str.find("; b=").unwrap() + 2;
        // Receivers ignore FWS inside the b= value (it was folded).
        let b_value: String = canon_str[b_at + 2..]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let without_b = format!("{}b=", &canon_str[..b_at]);
        let h_tag = canon_str
            .split(';')
            .find_map(|t| t.trim().strip_prefix("h="))
            .unwrap()
            .to_owned();
        let bh_tag = canon_str
            .split(';')
            .find_map(|t| t.trim().strip_prefix("bh="))
            .unwrap()
            .to_owned();
        let canon: Vec<(String, Vec<u8>)> =
            headers.iter().filter_map(|h| relaxed_header(h)).collect();
        let mut data = Vec::new();
        for name in h_tag.split(':') {
            if let Some((_, c)) = canon
                .iter()
                .rev()
                .find(|(n, _)| n == name && n != "dkim-signature")
            {
                data.extend_from_slice(c);
            }
        }
        data.extend_from_slice(without_b.as_bytes());
        let body_hash =
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(relaxed_body(body)));
        assert_eq!(body_hash, bh_tag);
        (
            data,
            base64::engine::general_purpose::STANDARD
                .decode(b_value)
                .unwrap(),
            h_tag,
        )
    }

    #[test]
    fn ed25519_sign_and_verify() {
        let key = SigningKey::parse(ED_PEM).unwrap();
        assert_eq!(key.algorithm(), "ed25519-sha256");
        let dns = key.dns_record().unwrap();
        assert!(dns.starts_with("v=DKIM1; k=ed25519; p="));
        let s = signer(key);
        let input = s.signature_input(MESSAGE, 1_789_237_500).unwrap();
        assert_eq!(
            input.signed_headers,
            vec![
                "from",
                "to",
                "subject",
                "date",
                "message-id",
                "mime-version",
                "content-type"
            ]
        );
        let signed = s.sign_message(MESSAGE, 1_789_237_500).unwrap();
        assert!(signed.starts_with(b"DKIM-Signature: v=1; a=ed25519-sha256; c=relaxed/relaxed; d=hashgram.io; s=s1; t=1789237500;\r\n\th=from:to:subject:date:message-id:mime-version:content-type;\r\n\tbh="));
        let (data, sig, h) = reconstruct(&signed);
        assert_eq!(data, input.data);
        assert_eq!(
            h,
            "from:to:subject:date:message-id:mime-version:content-type"
        );
        let vk = match &s.key {
            SigningKey::Ed25519(k) => k.verifying_key(),
            #[cfg(feature = "dkim-rsa")]
            SigningKey::Rsa(_) => panic!("ed25519 key expected"),
        };
        use ed25519_dalek::Verifier;
        let sig = ed25519_dalek::Signature::from_slice(&sig).unwrap();
        vk.verify(&Sha256::digest(&data), &sig).unwrap();
        // Tampering with a signed header breaks the reconstruction.
        let tampered = String::from_utf8(signed.clone())
            .unwrap()
            .replace("Subject: test", "Subject: pwned");
        let (data2, _, _) = reconstruct(tampered.as_bytes());
        assert_ne!(data2, input.data);
        // Every line of the signature header is short enough.
        for l in String::from_utf8(signed).unwrap().lines() {
            assert!(l.len() < 200);
        }
    }

    #[test]
    fn key_parsing_formats() {
        let hex_seed = "132efa13aa4da87e81f39389bd06470af8ce8498c10f150b8f7650abaf663e0c";
        let k1 = SigningKey::parse(ED_PEM).unwrap();
        let k2 = SigningKey::parse(hex_seed).unwrap();
        let k3 = SigningKey::parse("Ey76E6pNqH6B85OJvQZHCvjOhJjBDxULj3ZQq69mPgw=").unwrap();
        assert_eq!(k1.dns_record().unwrap(), k2.dns_record().unwrap());
        assert_eq!(k1.dns_record().unwrap(), k3.dns_record().unwrap());
        assert!(SigningKey::parse("garbage").is_err());
        assert!(
            SigningKey::parse("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----")
                .is_err()
        );
    }

    #[test]
    fn missing_from_is_refused() {
        let s = signer(SigningKey::parse(ED_PEM).unwrap());
        assert!(s.sign(b"To: x@y.z\r\n\r\nbody\r\n", 1).is_err());
    }

    #[cfg(feature = "dkim-rsa")]
    const RSA_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIICdQIBADANBgkqhkiG9w0BAQEFAASCAl8wggJbAgEAAoGBAJJNCuuwjKFNon4U
G05nyMw6ZjhXUNF/+idXiYkit3m91K+QVGe/ouyQeCxrJqec53tKG8PdhF7nzLEp
parlr5fKkv++T08ZokL/t0qrH+jQ8qheqo180Tiqc0FFjd95gN3eI6YCNcEt/4/p
B2ITlCjJHXwzD/ODQFU2V2PL8MBjAgMBAAECgYBPA4MJZdGd8HL5CtzwjIbbHhNF
DIteinLNOq7SPMjA3HB43UdovQw+HYx52OkIj2pJoO276/Bo3WIksKyDzwb07+yR
mtly1aHB72tuSUN83oPLAmdLVhLMF7b66l6blhhjR15fgAKx4WvbuxtQpK/7vLy4
I51KbSiLdIxDqtUV4QJBAMKPVR/AZU11tcLl9Gm9PymFyNMhvjnCc4+2ANv3/nWX
iaonvMXVKb1laZR5uQ7wpO4ivaqw3jM1g1VUA9a6WvMCQQDAgFizAS1XwLr36NaF
iH0SZhXPbrOb2/z8zYGN8o7/wpSyzdNVI/m1i7ZVMl06IH/k122W71TEznkrck7N
IYDRAkAbI89mDHqVIZRnSZicn2+OJUFsYkqc2AkyxNq91IxEbw0fFUf5+NBHwTvH
IGu2L89yAJqgkueMESzu3Ddk3r4NAkAKU9huYhXIq3JccoVvzI7JOejZpBrGtdqw
xWW589VwK0RHA3vfCXsQHlq932HZCH1UDaq3ekeV923QwuUvZCjBAkBf0u8ggs13
ATNo/dCYu/bKLgYw+9M3Fn67jMJarbjoU/YCyzFKESVkZgIcYeqXiKPIuFYHXG3B
9gYSvE9pyb3o
-----END PRIVATE KEY-----";

    #[cfg(feature = "dkim-rsa")]
    #[test]
    fn rsa_sign_and_verify() {
        use rsa::pkcs1v15::{Signature, VerifyingKey};
        use rsa::signature::Verifier;
        let key = SigningKey::parse(RSA_PEM).unwrap();
        assert_eq!(key.algorithm(), "rsa-sha256");
        assert!(key
            .dns_record()
            .unwrap()
            .starts_with("v=DKIM1; k=rsa; p=MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQCSTQrrsIyhTaJ+"));
        let s = signer(key);
        let signed = s.sign_message(MESSAGE, 42).unwrap();
        assert!(signed.starts_with(b"DKIM-Signature: v=1; a=rsa-sha256;"));
        let (data, sig, _) = reconstruct(&signed);
        let SigningKey::Rsa(k) = &s.key else { panic!() };
        let vk = VerifyingKey::<Sha256>::new(k.to_public_key());
        vk.verify(&data, &Signature::try_from(sig.as_slice()).unwrap())
            .unwrap();
    }
}

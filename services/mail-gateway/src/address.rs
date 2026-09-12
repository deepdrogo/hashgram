//! Address mapping between SMTP mailboxes and Hashgram identities.
//!
//! `alice@<our domain>` is the mail form of on-chain username `alice`
//! (`hashgram_app::mail::parse_address` knows the canonical
//! `hashgram.io`; a gateway run under another domain needs the same rule
//! for *its* domain, so the check is parameterised here). The local part
//! is validated with the username rules before any chain lookup, so a
//! `RCPT TO` with junk never causes a network round trip.

use crate::GatewayError;

/// Where an SMTP address points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mailbox {
    /// A username under our domain, lowercased.
    Local(String),
    /// Some other domain: relayed (outbound) or refused (inbound).
    Remote {
        /// Local part as written (case preserved: RFC 5321 says the
        /// receiving host decides its meaning).
        local: String,
        /// Domain, lowercased.
        domain: String,
    },
}

/// Parses an SMTP path or bare address (`<alice@hashgram.io>`,
/// `alice@hashgram.io`, with optional source route dropped) against our
/// domain.
pub fn classify(addr: &str, our_domain: &str) -> Result<Mailbox, GatewayError> {
    let raw = addr.trim();
    let raw = raw
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(raw);
    // Drop an obsolete source route (`@relay:user@host`).
    let raw = raw
        .rsplit_once(':')
        .filter(|(l, _)| l.starts_with('@'))
        .map_or(raw, |(_, r)| r);
    if raw.is_empty() || raw.len() > 254 {
        return Err(GatewayError::Address(format!(
            "bad address length: {}",
            raw.len()
        )));
    }
    let (local, domain) = raw
        .rsplit_once('@')
        .ok_or_else(|| GatewayError::Address(format!("no domain in {raw:?}")))?;
    if local.is_empty() || domain.is_empty() {
        return Err(GatewayError::Address(format!(
            "empty local part or domain in {raw:?}"
        )));
    }
    if local.len() > 64 {
        return Err(GatewayError::Address(
            "local part longer than 64 bytes".into(),
        ));
    }
    if !domain.contains('.')
        || !domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return Err(GatewayError::Address(format!("not a hostname: {domain:?}")));
    }
    let domain_lc = domain.to_ascii_lowercase();
    if domain_lc == our_domain.to_ascii_lowercase() {
        let name = local_part_to_username(local)?;
        return Ok(Mailbox::Local(name));
    }
    if !local.chars().all(|c| c.is_ascii_graphic()) {
        return Err(GatewayError::Address(
            "local part contains control or non-ASCII characters".into(),
        ));
    }
    Ok(Mailbox::Remote {
        local: local.to_owned(),
        domain: domain_lc,
    })
}

/// Username rules mirrored from `hashgram_app::mail` (2..=32 chars,
/// `[a-z0-9._-]`); plus-addressing (`alice+tag`) is accepted and the tag
/// dropped, as most providers do.
pub fn local_part_to_username(local: &str) -> Result<String, GatewayError> {
    let base = local.split('+').next().unwrap_or(local);
    match hashgram_app::mail::parse_address(&format!("@{base}")) {
        Ok(hashgram_app::mail::AddressForm::Username(u)) => Ok(u),
        _ => Err(GatewayError::Address(format!(
            "{local:?} is not a valid username"
        ))),
    }
}

/// `alice` → `alice@<domain>`.
#[must_use]
pub fn mail_address(username: &str, domain: &str) -> String {
    format!("{username}@{domain}")
}

/// Whether a domain is ours (case-insensitive).
#[must_use]
pub fn is_our_domain(domain: &str, our_domain: &str) -> bool {
    domain.eq_ignore_ascii_case(our_domain)
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

    const D: &str = "hashgram.io";

    #[test]
    fn local_addresses_map_to_usernames() {
        assert_eq!(
            classify("<Alice@Hashgram.IO>", D).unwrap(),
            Mailbox::Local("alice".into())
        );
        assert_eq!(
            classify("alice+news@hashgram.io", D).unwrap(),
            Mailbox::Local("alice".into())
        );
        assert_eq!(
            classify("@relay.example:bob@hashgram.io", D).unwrap(),
            Mailbox::Local("bob".into())
        );
        assert_eq!(
            classify("bob.b-2_x@hashgram.io", D).unwrap(),
            Mailbox::Local("bob.b-2_x".into())
        );
    }

    #[test]
    fn invalid_local_parts_are_refused() {
        assert!(classify("a@hashgram.io", D).is_err()); // too short
        assert!(classify("has space@hashgram.io", D).is_err());
        assert!(classify("a/b@hashgram.io", D).is_err());
        assert!(classify(&format!("{}@hashgram.io", "x".repeat(33)), D).is_err());
        assert!(classify("", D).is_err());
        assert!(classify("nodomain", D).is_err());
        assert!(classify("@hashgram.io", D).is_err());
        assert!(classify("alice@", D).is_err());
    }

    #[test]
    fn other_domains_are_remote() {
        assert_eq!(
            classify("<Someone@Example.COM>", D).unwrap(),
            Mailbox::Remote {
                local: "Someone".into(),
                domain: "example.com".into()
            }
        );
        assert!(classify("x@localhost", D).is_err()); // no dot
        assert!(classify("x@exa mple.com", D).is_err());
    }

    #[test]
    fn other_operator_domain() {
        assert_eq!(
            classify("carol@mail.example.org", "mail.example.org").unwrap(),
            Mailbox::Local("carol".into())
        );
        assert!(matches!(
            classify("carol@hashgram.io", "mail.example.org").unwrap(),
            Mailbox::Remote { .. }
        ));
    }
}

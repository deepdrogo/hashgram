//! Signature purposes.
//!
//! Every signature in Hashgram is over a digest that commits to both the
//! network and the purpose. Without the purpose, a signature over a device
//! certificate could be presented as a service receipt, and the two message
//! types would have to be structurally impossible to confuse — which is a
//! much harder property to maintain than a domain string.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A signature purpose.
///
/// The string values are part of the wire protocol. They appear inside signing
/// domains, which appear inside signed digests, so changing one invalidates
/// every signature ever produced for that purpose. They match the Go constants
/// in `app/params/network.go` exactly, and the cross-language vector tests
/// fail if they drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SigningPurpose {
    /// A signed social event: profile update, post, follow, reaction.
    SocialEvent,
    /// A device certificate authorising a device under a root identity.
    DeviceCert,
    /// A client-signed receipt for relay, retrieval or call service.
    ServiceReceipt,
    /// A provider's answer to a storage challenge.
    StorageChallenge,
    /// An attestor's statement that an address is eligible for a welcome reward.
    EligibilityAttestation,
    /// A safety provider's attestation about public content.
    ContentAttestation,
    /// A signed bootstrap record, so peer discovery does not rely on
    /// unauthenticated hints.
    BootstrapRecord,
    /// The Hashgram P2P handshake.
    PeerHandshake,
    /// A node announcing the services it offers.
    NodeAnnounce,
    /// A device proving it owns a mailbox in order to read it.
    MailboxFetch,
    /// A device acknowledging envelopes so a store node may delete them.
    MailboxAck,
    /// A device publishing an MLS key package.
    KeyPackage,
    /// A device authorising a blob upload under its quota.
    BlobUpload,
}

/// Every purpose, in a fixed order.
///
/// Fixed so that iterating for tests or for operator output is reproducible.
pub const ALL_PURPOSES: [SigningPurpose; 13] = [
    SigningPurpose::SocialEvent,
    SigningPurpose::DeviceCert,
    SigningPurpose::ServiceReceipt,
    SigningPurpose::StorageChallenge,
    SigningPurpose::EligibilityAttestation,
    SigningPurpose::ContentAttestation,
    SigningPurpose::BootstrapRecord,
    SigningPurpose::PeerHandshake,
    SigningPurpose::NodeAnnounce,
    SigningPurpose::MailboxFetch,
    SigningPurpose::MailboxAck,
    SigningPurpose::KeyPackage,
    SigningPurpose::BlobUpload,
];

impl SigningPurpose {
    /// The wire string for this purpose.
    ///
    /// Matches the Go constants character for character. These are not
    /// display strings and must not be localised, prettified or reordered.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SocialEvent => "social-event",
            Self::DeviceCert => "device-cert",
            Self::ServiceReceipt => "service-receipt",
            Self::StorageChallenge => "storage-challenge",
            Self::EligibilityAttestation => "eligibility-attestation",
            Self::ContentAttestation => "content-attestation",
            Self::BootstrapRecord => "bootstrap-record",
            Self::PeerHandshake => "peer-handshake",
            Self::NodeAnnounce => "node-announce",
            Self::MailboxFetch => "mailbox-fetch",
            Self::MailboxAck => "mailbox-ack",
            Self::KeyPackage => "key-package",
            Self::BlobUpload => "blob-upload",
        }
    }
}

impl fmt::Display for SigningPurpose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Returned when a string is not a known signing purpose.
///
/// An unknown purpose is rejected rather than mapped to a default. A default
/// would mean a signature produced for a purpose this build does not
/// understand could be verified under some other purpose, which is exactly
/// the confusion domain separation exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown signing purpose {0:?}; a purpose this build does not know must be rejected, not defaulted")]
pub struct ParsePurposeError(pub String);

impl FromStr for SigningPurpose {
    type Err = ParsePurposeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ALL_PURPOSES
            .iter()
            .copied()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| ParsePurposeError(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_its_wire_string() {
        for purpose in ALL_PURPOSES {
            let parsed: SigningPurpose = purpose
                .as_str()
                .parse()
                .unwrap_or_else(|_| panic!("{purpose} did not parse back"));
            assert_eq!(parsed, purpose);
        }
    }

    #[test]
    fn wire_strings_are_distinct() {
        // Two purposes sharing a string would produce identical domains, and
        // a signature for one would verify for the other.
        let mut seen = std::collections::HashSet::new();
        for purpose in ALL_PURPOSES {
            assert!(
                seen.insert(purpose.as_str()),
                "{purpose} shares a wire string with another purpose"
            );
        }
        assert_eq!(seen.len(), ALL_PURPOSES.len());
    }

    #[test]
    fn unknown_purpose_is_refused() {
        let err = "not-a-purpose".parse::<SigningPurpose>();
        assert!(err.is_err(), "an unknown purpose was accepted");
    }

    #[test]
    fn a_purpose_is_not_matched_case_insensitively() {
        // The domain is bytes in a hash. "Social-Event" is a different byte
        // string from "social-event" and must not silently become it.
        assert!("Social-Event".parse::<SigningPurpose>().is_err());
        assert!("SOCIAL-EVENT".parse::<SigningPurpose>().is_err());
    }
}

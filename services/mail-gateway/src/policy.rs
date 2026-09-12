//! Acceptance policy: the few decisions the operator configures.
//!
//! The gateway is not a spam filter. It records what the fronting MTA
//! already decided (`Authentication-Results`, `X-Spam-Score`) into
//! `ExternalMailMeta` so the *recipient's* client can weigh it
//! (`hashgram_app::spam` treats gateway verdicts as one advisory input).
//! The policy here is the coarse gate before that: refuse outright what
//! the operator does not want to carry at all.

use crate::config::PolicySection;

/// Authentication verdicts as recorded into `ExternalMailMeta.auth_results`.
/// `Default` is [`AuthResults::none`]: the absence of a verdict is itself a
/// verdict ("nothing was checked"), never an empty string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthResults {
    /// `spf=pass|fail|softfail|neutral|none|temperror|permerror`.
    pub spf: String,
    /// `dkim=pass|fail|none|...`.
    pub dkim: String,
    /// `dmarc=pass|fail|none|...`.
    pub dmarc: String,
}

impl Default for AuthResults {
    fn default() -> Self {
        Self::none()
    }
}

impl AuthResults {
    /// Nothing verified — what we record honestly when no fronting MTA
    /// added a header.
    #[must_use]
    pub fn none() -> Self {
        Self {
            spf: "none".into(),
            dkim: "none".into(),
            dmarc: "none".into(),
        }
    }

    /// The strings for `ExternalMailMeta.auth_results`.
    #[must_use]
    pub fn as_strings(&self) -> Vec<String> {
        vec![
            format!("spf={}", self.spf),
            format!("dkim={}", self.dkim),
            format!("dmarc={}", self.dmarc),
        ]
    }

    /// Either SPF or DKIM passed.
    #[must_use]
    pub fn authenticated(&self) -> bool {
        self.spf == "pass" || self.dkim == "pass"
    }
}

/// What the policy says about an inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Carry it.
    Accept,
    /// Refuse with a `550` and this text.
    Reject(String),
}

/// Applies `[policy]` to the parsed authentication and spam facts.
#[must_use]
pub fn decide_inbound(policy: &PolicySection, auth: &AuthResults, spam_score: u32) -> Decision {
    if policy.require_spf_or_dkim && !auth.authenticated() {
        return Decision::Reject(
            "5.7.1 message not authenticated (SPF or DKIM pass required)".into(),
        );
    }
    if policy.reject_spam_score_over > 0 && spam_score > policy.reject_spam_score_over {
        return Decision::Reject(format!(
            "5.7.1 spam score {spam_score} exceeds this gateway's threshold"
        ));
    }
    Decision::Accept
}

/// Whether a Hashgram user may relay outbound through us.
#[must_use]
pub fn allow_outbound(policy: &PolicySection, sender_is_contact: bool) -> bool {
    !policy.allow_outbound_from_contacts_only || sender_is_contact
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

    fn pol(spf_dkim: bool, score: u32, contacts: bool) -> PolicySection {
        PolicySection {
            require_spf_or_dkim: spf_dkim,
            reject_spam_score_over: score,
            allow_outbound_from_contacts_only: contacts,
        }
    }

    #[test]
    fn default_policy_accepts_everything() {
        assert_eq!(
            decide_inbound(&pol(false, 0, false), &AuthResults::none(), 999),
            Decision::Accept
        );
    }

    #[test]
    fn spf_or_dkim_required() {
        let p = pol(true, 0, false);
        assert!(matches!(
            decide_inbound(&p, &AuthResults::none(), 0),
            Decision::Reject(_)
        ));
        let mut a = AuthResults::none();
        a.dkim = "pass".into();
        assert_eq!(decide_inbound(&p, &a, 0), Decision::Accept);
        let mut a = AuthResults::none();
        a.spf = "pass".into();
        assert_eq!(decide_inbound(&p, &a, 0), Decision::Accept);
        a.spf = "softfail".into();
        assert!(matches!(decide_inbound(&p, &a, 0), Decision::Reject(_)));
    }

    #[test]
    fn spam_threshold() {
        let p = pol(false, 500, false);
        assert_eq!(
            decide_inbound(&p, &AuthResults::none(), 500),
            Decision::Accept
        );
        assert!(matches!(
            decide_inbound(&p, &AuthResults::none(), 501),
            Decision::Reject(_)
        ));
    }

    #[test]
    fn outbound_contacts_only() {
        assert!(allow_outbound(&pol(false, 0, false), false));
        assert!(!allow_outbound(&pol(false, 0, true), false));
        assert!(allow_outbound(&pol(false, 0, true), true));
    }

    #[test]
    fn auth_strings() {
        assert_eq!(
            AuthResults::none().as_strings(),
            vec!["spf=none", "dkim=none", "dmarc=none"]
        );
    }
}

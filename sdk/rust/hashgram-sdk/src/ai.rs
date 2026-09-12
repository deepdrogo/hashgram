//! Hash AI: interfaces only.
//!
//! A client may later let the user ask questions over their own Mail,
//! Drive, Contacts, Spaces and Feed ("find the contract John sent me").
//! This module fixes the boundary so that can be added without touching
//! anything else:
//!
//! * [`AiProvider`] is the trait a client implements. Two shapes are
//!   anticipated: a [`LocalAiProvider`] that runs entirely on the device,
//!   and a remote one that MUST hold an explicit [`UserConsent`] covering
//!   the exact categories of content it is about to transmit.
//! * [`Corpus`] is the *only* thing an AI provider receives: plaintext the
//!   user already has, selected by category, never keys, never the vault,
//!   never other users' content beyond what is in the user's own mailbox.
//! * Nothing in consensus, the node or the protocol knows about this.
//!
//! No implementation here transmits anything. [`LocalAiProvider`] is a
//! reference that answers with keyword matches so the interface can be
//! exercised in tests and the desktop can wire the UI before a model is
//! chosen.

use std::collections::BTreeSet;

/// Categories of content an AI request may touch.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum Category {
    /// Mail subjects and bodies.
    Mail,
    /// Drive file names and text content.
    Drive,
    /// Contact names and addresses.
    Contacts,
    /// Space content.
    Spaces,
    /// Feed and Circle posts.
    Feed,
}

/// Explicit, revocable consent for remote processing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserConsent {
    /// Categories the user allowed.
    pub categories: BTreeSet<Category>,
    /// Provider identifier the consent is for.
    pub provider: String,
    /// Unix seconds when given.
    pub granted_at: u64,
    /// Unix seconds after which it lapses (0 = until revoked).
    pub expires_at: u64,
}

impl UserConsent {
    /// Whether this consent covers `categories` for `provider` at `now`.
    #[must_use]
    pub fn covers(&self, provider: &str, categories: &BTreeSet<Category>, now: u64) -> bool {
        self.provider == provider
            && (self.expires_at == 0 || now < self.expires_at)
            && categories.is_subset(&self.categories)
    }
}

/// A unit of user content handed to a provider.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Document {
    /// Category.
    pub category: Category,
    /// Stable id (mail id hex, drive entry hex, …).
    pub id: String,
    /// Title / subject / file name.
    pub title: String,
    /// Plain text.
    pub text: String,
    /// Timestamp (ms).
    pub at_ms: u64,
    /// People involved (addresses or usernames).
    pub people: Vec<String>,
}

/// What a provider gets.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Corpus {
    /// Documents.
    pub documents: Vec<Document>,
}

/// A query.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Query {
    /// Natural-language question.
    pub text: String,
    /// Categories the caller allows the provider to consult.
    pub categories: BTreeSet<Category>,
    /// Maximum results.
    pub limit: usize,
}

/// An answer.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Answer {
    /// Free-text answer (may be empty for pure retrieval).
    pub text: String,
    /// Referenced documents, most relevant first.
    pub references: Vec<Document>,
    /// Whether any content left the device.
    pub transmitted: bool,
}

/// Errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AiError {
    /// Remote processing without matching consent.
    #[error("no consent for provider {provider} over {categories:?}")]
    NoConsent {
        /// Provider.
        provider: String,
        /// Categories requested.
        categories: BTreeSet<Category>,
    },
    /// Provider failure.
    #[error("ai provider: {0}")]
    Provider(String),
}

/// The provider boundary.
pub trait AiProvider: Send + Sync {
    /// Stable identifier ("local", "acme-cloud-v2", …).
    fn id(&self) -> &str;
    /// Whether content leaves the device.
    fn is_remote(&self) -> bool;
    /// Answers a query over a corpus. Remote providers MUST check
    /// `consent` and return [`AiError::NoConsent`] otherwise.
    fn answer(
        &self,
        corpus: &Corpus,
        query: &Query,
        consent: Option<&UserConsent>,
    ) -> Result<Answer, AiError>;
}

/// Reference on-device provider: keyword retrieval, no model.
#[derive(Debug, Default)]
pub struct LocalAiProvider;

impl AiProvider for LocalAiProvider {
    fn id(&self) -> &str {
        "local"
    }
    fn is_remote(&self) -> bool {
        false
    }
    fn answer(
        &self,
        corpus: &Corpus,
        query: &Query,
        _consent: Option<&UserConsent>,
    ) -> Result<Answer, AiError> {
        let terms: Vec<String> = query
            .text
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() > 2)
            .map(str::to_owned)
            .collect();
        let mut scored: Vec<(usize, &Document)> = corpus
            .documents
            .iter()
            .filter(|d| query.categories.contains(&d.category))
            .map(|d| {
                let hay = format!("{} {} {}", d.title, d.text, d.people.join(" ")).to_lowercase();
                (terms.iter().filter(|t| hay.contains(t.as_str())).count(), d)
            })
            .filter(|(s, _)| *s > 0)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.at_ms.cmp(&a.1.at_ms)));
        Ok(Answer {
            text: String::new(),
            references: scored
                .into_iter()
                .take(query.limit)
                .map(|(_, d)| d.clone())
                .collect(),
            transmitted: false,
        })
    }
}

/// Guard every remote provider must call first.
pub fn require_consent(
    provider: &dyn AiProvider,
    query: &Query,
    consent: Option<&UserConsent>,
    now: u64,
) -> Result<(), AiError> {
    if !provider.is_remote() {
        return Ok(());
    }
    match consent {
        Some(c) if c.covers(provider.id(), &query.categories, now) => Ok(()),
        _ => Err(AiError::NoConsent {
            provider: provider.id().to_owned(),
            categories: query.categories.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Remote;
    impl AiProvider for Remote {
        fn id(&self) -> &str {
            "remote-test"
        }
        fn is_remote(&self) -> bool {
            true
        }
        fn answer(
            &self,
            _c: &Corpus,
            q: &Query,
            consent: Option<&UserConsent>,
        ) -> Result<Answer, AiError> {
            require_consent(self, q, consent, 100)?;
            Ok(Answer {
                transmitted: true,
                ..Default::default()
            })
        }
    }

    #[test]
    fn local_retrieval_and_remote_consent() {
        let corpus = Corpus {
            documents: vec![
                Document {
                    category: Category::Mail,
                    id: "1".into(),
                    title: "Contract".into(),
                    text: "the signed contract from John".into(),
                    at_ms: 5,
                    people: vec!["john".into()],
                },
                Document {
                    category: Category::Drive,
                    id: "2".into(),
                    title: "invoice-august.pdf".into(),
                    text: String::new(),
                    at_ms: 6,
                    people: vec![],
                },
            ],
        };
        let q = Query {
            text: "find the contract John sent me".into(),
            categories: [Category::Mail].into_iter().collect(),
            limit: 5,
        };
        let a = LocalAiProvider.answer(&corpus, &q, None).unwrap();
        assert_eq!(a.references.len(), 1);
        assert!(!a.transmitted);
        let r = Remote;
        assert!(matches!(
            r.answer(&corpus, &q, None),
            Err(AiError::NoConsent { .. })
        ));
        let consent = UserConsent {
            categories: [Category::Mail].into_iter().collect(),
            provider: "remote-test".into(),
            granted_at: 1,
            expires_at: 0,
        };
        assert!(r.answer(&corpus, &q, Some(&consent)).unwrap().transmitted);
        let q2 = Query {
            categories: [Category::Mail, Category::Drive].into_iter().collect(),
            ..q
        };
        assert!(
            r.answer(&corpus, &q2, Some(&consent)).is_err(),
            "consent must cover every category"
        );
    }
}

//! In-app Help: Markdown bundled at compile time, rendered offline.

use serde::Serialize;

/// A help page.
#[derive(Debug, Clone, Serialize)]
pub struct HelpPage {
    /// Slug.
    pub slug: &'static str,
    /// Title.
    pub title: &'static str,
    /// Matching file in `docs/` of the repository, at this commit.
    pub docs_path: &'static str,
}

macro_rules! pages {
    ($( ($slug:literal, $title:literal, $docs:literal, $file:literal) ),* $(,)?) => {
        /// Every page.
        pub const PAGES: &[HelpPage] = &[
            $( HelpPage { slug: $slug, title: $title, docs_path: $docs } ),*
        ];
        fn source(slug: &str) -> Option<&'static str> {
            match slug {
                $( $slug => Some(include_str!(concat!("../../help/", $file))), )*
                _ => None,
            }
        }
    };
}

pages![
    (
        "quick-start",
        "Quick start",
        "docs/CLIENT_CONNECTIVITY_SPEC.md",
        "quick-start.md"
    ),
    (
        "keys",
        "Your keys and what they control",
        "docs/SECURITY.md",
        "keys.md"
    ),
    (
        "sending",
        "Sending and fees",
        "docs/TOKENOMICS.md",
        "sending.md"
    ),
    (
        "messages",
        "Messages and devices",
        "docs/MESSAGING.md",
        "messages.md"
    ),
    (
        "social",
        "Feed, reels, channels",
        "docs/SOCIAL_PROTOCOL.md",
        "social.md"
    ),
    ("calls", "Calls", "docs/CALLS.md", "calls.md"),
    (
        "earn",
        "Earn by running a node",
        "docs/SERVICE_REWARDS.md",
        "earn.md"
    ),
    (
        "staking",
        "Staking and governance",
        "docs/TOKENOMICS.md",
        "staking.md"
    ),
    (
        "network",
        "Network and nodes",
        "docs/PROTOCOL.md",
        "network.md"
    ),
    (
        "troubleshooting",
        "Troubleshooting",
        "docs/OPERATIONS.md",
        "troubleshooting.md"
    ),
    ("faq", "FAQ", "docs/ARCHITECTURE.md", "faq.md"),
    ("privacy", "Privacy", "docs/THREAT_MODEL.md", "privacy.md"),
];

/// Renders a page to HTML (links to the repository docs are appended).
#[must_use]
pub fn render(slug: &str, commit: &str) -> Option<String> {
    let page = PAGES.iter().find(|p| p.slug == slug)?;
    let md = source(slug)?;
    let mut opts = pulldown_cmark::Options::empty();
    opts.insert(pulldown_cmark::Options::ENABLE_TABLES);
    opts.insert(pulldown_cmark::Options::ENABLE_STRIKETHROUGH);
    let parser = pulldown_cmark::Parser::new_ext(md, opts);
    let mut html = String::with_capacity(md.len() * 2);
    pulldown_cmark::html::push_html(&mut html, parser);
    html.push_str(&format!(
        "<p class=\"help-source\">Source in the repository: <code>{}</code> at commit <code>{}</code>.</p>",
        page.docs_path, commit
    ));
    Some(html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_renders_and_none_says_mining() {
        for p in PAGES {
            let html = render(p.slug, "abc1234").unwrap();
            assert!(
                html.contains("<h1") || html.contains("<h2"),
                "{} has no heading",
                p.slug
            );
            let lower = html.to_ascii_lowercase();
            // "no mining" is the one allowed phrasing; "mine"/"mining" as a
            // feature is not.
            for word in [
                "start mining",
                "mining rewards",
                "mine hash",
                "gpu",
                "cpu earning",
            ] {
                assert!(!lower.contains(word), "{} mentions {word:?}", p.slug);
            }
        }
    }
}

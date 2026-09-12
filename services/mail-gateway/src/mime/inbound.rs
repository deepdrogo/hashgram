//! Internet message → [`InboundMail`].
//!
//! Everything here reads attacker-controlled bytes, so every field is
//! bounded to what `hashgram_app::mail::validate` will accept before the
//! bridge ever builds a `Draft`: subjects and header copies to
//! `MAX_HEADER`, bodies to `MAX_BODY_*`, attachment counts and names to
//! their limits. Over-limit content is truncated or dropped *with a note
//! in the text body*, never silently: the recipient should know the
//! gateway cut something.
//!
//! What is preserved for threading and provenance: `Message-ID`,
//! `In-Reply-To`, `References` (normalised), the raw `From` line, the
//! fronting MTA's `Authentication-Results` and `X-Spam-Score`/
//! `X-Spam-Status`. What is not: `Received` chains, `DKIM-Signature`
//! bodies, list headers — the recipient's client has no use for them and
//! `ExternalMailMeta` has no room.

use hashgram_app::mail::{
    MAX_ATTACHMENTS, MAX_BODY_HTML, MAX_BODY_TEXT, MAX_HEADER, MAX_MIME, MAX_SUBJECT,
};
use hashgram_app::pb as app;
use mailparse::{DispositionType, MailAddr, MailHeaderMap, ParsedMail};

use super::html::html_to_text;
use super::safe_filename;
use crate::policy::AuthResults;
use crate::thread::{normalise_message_id, split_references};
use crate::GatewayError;

/// A parsed mailbox from an address header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mailbox {
    /// Display name, possibly empty.
    pub name: String,
    /// `local@domain`, as written.
    pub addr: String,
}

/// One attachment (or inline image) with its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundAttachment {
    /// Safe file name.
    pub name: String,
    /// MIME type (bounded).
    pub mime: String,
    /// `Content-ID` without angle brackets, for `cid:` references.
    pub content_id: String,
    /// Decoded bytes.
    pub data: Vec<u8>,
}

/// The structured message.
#[derive(Debug, Clone, Default)]
pub struct InboundMail {
    /// First `From` mailbox, if parseable.
    pub from: Option<Mailbox>,
    /// The `From` header value verbatim (bounded), for `ExternalMailMeta`.
    pub from_header: String,
    /// `Reply-To` mailboxes.
    pub reply_to: Vec<Mailbox>,
    /// `To` mailboxes.
    pub to: Vec<Mailbox>,
    /// `Cc` mailboxes.
    pub cc: Vec<Mailbox>,
    /// Decoded subject (bounded, no line breaks).
    pub subject: String,
    /// Normalised `Message-ID` (no angle brackets), if present.
    pub message_id: Option<String>,
    /// Normalised `In-Reply-To`, if present.
    pub in_reply_to: Option<String>,
    /// Normalised `References`, oldest first.
    pub references: Vec<String>,
    /// `Date` header as unix ms, if parseable.
    pub date_ms: Option<u64>,
    /// Plain text body (derived from HTML when there was none).
    pub body_text: String,
    /// HTML body, if any and within bounds.
    pub body_html: String,
    /// Attachments and inline parts.
    pub attachments: Vec<InboundAttachment>,
    /// Authentication verdicts from the fronting MTA.
    pub auth: AuthResults,
    /// Spam score in 0..=1000 (0 = unknown).
    pub spam_score: u32,
    /// Importance from `Importance`/`X-Priority`.
    pub importance: app::MailImportance,
    /// Notes about what the gateway had to cut.
    pub notes: Vec<String>,
}

/// Parses an RFC 5322 message.
pub fn parse(raw: &[u8]) -> Result<InboundMail, GatewayError> {
    let mail = mailparse::parse_mail(raw).map_err(|e| GatewayError::Mime(e.to_string()))?;
    let h = &mail.headers;
    let mut m = InboundMail::default();

    let from_raw = h.get_first_value("From").unwrap_or_default();
    m.from_header = bound(&from_raw, MAX_HEADER);
    m.from = parse_mailboxes(&from_raw).into_iter().next();
    m.reply_to = parse_mailboxes(&h.get_first_value("Reply-To").unwrap_or_default());
    m.to = h
        .get_all_values("To")
        .iter()
        .flat_map(|v| parse_mailboxes(v))
        .collect();
    m.cc = h
        .get_all_values("Cc")
        .iter()
        .flat_map(|v| parse_mailboxes(v))
        .collect();
    m.subject = clean_subject(&h.get_first_value("Subject").unwrap_or_default());
    m.message_id = h
        .get_first_value("Message-ID")
        .map(|v| normalise_message_id(&v))
        .filter(|v| !v.is_empty() && v.len() <= MAX_HEADER);
    m.in_reply_to = h
        .get_first_value("In-Reply-To")
        .map(|v| {
            split_references(&v)
                .into_iter()
                .next()
                .unwrap_or_else(|| normalise_message_id(&v))
        })
        .filter(|v| !v.is_empty() && v.len() <= MAX_HEADER);
    m.references = h
        .get_all_values("References")
        .iter()
        .flat_map(|v| split_references(v))
        .filter(|v| v.len() <= MAX_HEADER)
        .take(hashgram_app::mail::MAX_REFERENCES)
        .collect();
    m.date_ms = h
        .get_first_value("Date")
        .and_then(|d| mailparse::dateparse(&d).ok())
        .filter(|s| *s > 0)
        .map(|s| s as u64 * 1000);
    m.auth = parse_auth_results(h.get_first_value("Authentication-Results").as_deref());
    m.spam_score = parse_spam_score(
        h.get_first_value("X-Spam-Score").as_deref(),
        h.get_first_value("X-Spam-Status").as_deref(),
    );
    m.importance = parse_importance(
        h.get_first_value("Importance").as_deref(),
        h.get_first_value("X-Priority").as_deref(),
    );

    let mut body = BodyCollector::default();
    body.walk(&mail, 0);
    finish_body(&mut m, body);
    Ok(m)
}

fn bound(s: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if out.len() + ch.len_utf8() > max {
            break;
        }
        out.push(ch);
    }
    out
}

fn clean_subject(s: &str) -> String {
    let one_line: String = s
        .chars()
        .map(|c| {
            if c == '\r' || c == '\n' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .filter(|c| !c.is_control())
        .collect();
    bound(
        one_line
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .as_str(),
        MAX_SUBJECT,
    )
}

/// Parses an address header into mailboxes; garbage yields nothing rather
/// than an error, because a bad `Cc` must not lose the whole message.
#[must_use]
pub fn parse_mailboxes(value: &str) -> Vec<Mailbox> {
    let Ok(list) = mailparse::addrparse(value) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for a in list.iter() {
        match a {
            MailAddr::Single(s) => out.push(Mailbox {
                name: bound(
                    s.display_name.as_deref().unwrap_or("").trim(),
                    hashgram_app::mail::MAX_NAME,
                ),
                addr: s.addr.trim().to_owned(),
            }),
            MailAddr::Group(g) => {
                for s in &g.addrs {
                    out.push(Mailbox {
                        name: bound(
                            s.display_name.as_deref().unwrap_or("").trim(),
                            hashgram_app::mail::MAX_NAME,
                        ),
                        addr: s.addr.trim().to_owned(),
                    });
                }
            }
        }
    }
    out
}

/// `Authentication-Results: authserv; spf=pass smtp.mailfrom=…; dkim=pass
/// header.d=…; dmarc=pass` → verdicts. Absent header → all `none`,
/// recorded honestly rather than guessed.
#[must_use]
pub fn parse_auth_results(value: Option<&str>) -> AuthResults {
    let mut out = AuthResults::none();
    let Some(v) = value else { return out };
    for (i, part) in v.split(';').enumerate() {
        if i == 0 {
            continue; // authserv-id
        }
        let part = part.trim();
        let Some(first) = part.split_whitespace().next() else {
            continue;
        };
        let Some((method, result)) = first.split_once('=') else {
            continue;
        };
        let result = result.trim().to_ascii_lowercase();
        let result: String = result
            .chars()
            .take(32)
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        if result.is_empty() {
            continue;
        }
        match method.trim().to_ascii_lowercase().as_str() {
            "spf" if out.spf == "none" => out.spf = result,
            "dkim" if out.dkim == "none" || result == "pass" => out.dkim = result,
            "dmarc" if out.dmarc == "none" => out.dmarc = result,
            _ => {}
        }
    }
    out
}

/// `X-Spam-Score: 5.2` or `X-Spam-Status: Yes, score=5.2 required=5.0` →
/// 0..=1000 (points × 100, clamped). Missing → 0 ("unknown").
#[must_use]
pub fn parse_spam_score(score: Option<&str>, status: Option<&str>) -> u32 {
    let from_score = score.and_then(|s| s.trim().parse::<f64>().ok());
    let from_status = status.and_then(|s| {
        s.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .find_map(|tok| {
                tok.strip_prefix("score=")
                    .and_then(|v| v.parse::<f64>().ok())
            })
    });
    let points = from_score.or(from_status).unwrap_or(0.0);
    if !points.is_finite() || points <= 0.0 {
        return 0;
    }
    (points * 100.0).round().min(1000.0) as u32
}

fn parse_importance(importance: Option<&str>, x_priority: Option<&str>) -> app::MailImportance {
    if let Some(i) = importance {
        match i.trim().to_ascii_lowercase().as_str() {
            "high" | "urgent" => return app::MailImportance::High,
            "low" | "non-urgent" => return app::MailImportance::Low,
            _ => {}
        }
    }
    if let Some(p) = x_priority {
        match p.trim().chars().next() {
            Some('1' | '2') => return app::MailImportance::High,
            Some('4' | '5') => return app::MailImportance::Low,
            _ => {}
        }
    }
    app::MailImportance::Normal
}

#[derive(Default)]
struct BodyCollector {
    text: Option<String>,
    html: Option<String>,
    attachments: Vec<InboundAttachment>,
    dropped_attachments: usize,
    part_counter: usize,
}

impl BodyCollector {
    fn walk(&mut self, part: &ParsedMail<'_>, depth: usize) {
        if depth > 16 {
            return; // pathological nesting
        }
        let mime = part.ctype.mimetype.to_ascii_lowercase();
        if mime.starts_with("multipart/") {
            // `multipart/alternative` needs no special case: the first
            // text/plain and first text/html leaves win, and attachments
            // nested in any branch (`multipart/related` inside the HTML
            // alternative) are still collected.
            for sub in &part.subparts {
                self.walk(sub, depth + 1);
            }
            return;
        }
        let disp = part.get_content_disposition();
        let filename = disp
            .params
            .get("filename")
            .or_else(|| part.ctype.params.get("name"))
            .cloned();
        let is_attachment = disp.disposition == DispositionType::Attachment || filename.is_some();
        if !is_attachment {
            if mime == "text/plain" && self.text.is_none() {
                self.text = part.get_body().ok();
                return;
            }
            if mime == "text/html" && self.html.is_none() {
                self.html = part.get_body().ok();
                return;
            }
            if mime == "text/plain" || mime == "text/html" {
                // A second body of the same kind (e.g. a forwarded
                // signature block): append to text so nothing is lost.
                if let Ok(extra) = part.get_body() {
                    let t = if mime == "text/html" {
                        html_to_text(&extra)
                    } else {
                        extra
                    };
                    let cur = self.text.get_or_insert_with(String::new);
                    if !t.trim().is_empty() {
                        cur.push_str("\n\n");
                        cur.push_str(&t);
                    }
                }
                return;
            }
        }
        // Everything else is an attachment: files, inline images,
        // message/rfc822, calendar invites…
        if self.attachments.len() >= MAX_ATTACHMENTS {
            self.dropped_attachments += 1;
            return;
        }
        let Ok(data) = part.get_body_raw() else {
            self.dropped_attachments += 1;
            return;
        };
        self.part_counter += 1;
        let fallback = format!("part-{}{}", self.part_counter, extension_for(&mime));
        let content_id = part
            .headers
            .get_first_value("Content-ID")
            .map(|v| normalise_message_id(&v))
            .map(|v| bound(&v, hashgram_app::mail::MAX_NAME))
            .unwrap_or_default();
        self.attachments.push(InboundAttachment {
            name: safe_filename(filename.as_deref().unwrap_or(""), &fallback),
            mime: bound(
                if mime.is_empty() {
                    "application/octet-stream"
                } else {
                    &mime
                },
                MAX_MIME,
            ),
            content_id,
            data,
        });
    }
}

fn extension_for(mime: &str) -> &'static str {
    match mime {
        "image/png" => ".png",
        "image/jpeg" => ".jpg",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "application/pdf" => ".pdf",
        "text/calendar" => ".ics",
        "message/rfc822" => ".eml",
        "text/plain" => ".txt",
        "text/html" => ".html",
        _ => ".bin",
    }
}

fn finish_body(m: &mut InboundMail, b: BodyCollector) {
    let mut text = b.text.unwrap_or_default();
    let mut html = b.html.unwrap_or_default();
    if text.trim().is_empty() && !html.is_empty() {
        text = html_to_text(&html);
    }
    // Normalise line endings; MailMessage bodies are plain strings.
    text = text.replace("\r\n", "\n");
    if text.len() > MAX_BODY_TEXT {
        let cut = MAX_BODY_TEXT - 200;
        let mut end = cut;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n\n[gateway: text body truncated at 1 MiB]");
        m.notes.push("text body truncated".into());
    }
    if html.len() > MAX_BODY_HTML {
        html = String::new();
        m.notes.push("html body dropped (over 1 MiB)".into());
    }
    if b.dropped_attachments > 0 {
        text.push_str(&format!(
            "\n\n[gateway: {} attachment(s) omitted, over the {MAX_ATTACHMENTS} limit]",
            b.dropped_attachments
        ));
        m.notes
            .push(format!("{} attachments dropped", b.dropped_attachments));
    }
    m.body_text = text;
    m.body_html = html;
    m.attachments = b.attachments;
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

    const PLAIN: &str = "From: \"Bob Example\" <bob@example.com>\r\nTo: Alice <alice@hashgram.io>, carol@hashgram.io\r\nCc: dave@example.org\r\nSubject: =?UTF-8?B?4YOS4YOQ4YOb4YOQ4YOg4YOv4YOd4YOR4YOQ?= hi\r\nDate: Sat, 12 Sep 2026 18:25:00 +0000\r\nMessage-ID: <abc123@Example.com>\r\nIn-Reply-To: <parent@hashgram.io>\r\nReferences: <root@x.org> <parent@hashgram.io>\r\nAuthentication-Results: mx.hashgram.io; spf=pass smtp.mailfrom=example.com; dkim=pass (2048-bit key) header.d=example.com; dmarc=pass\r\nX-Spam-Score: 1.5\r\nX-Priority: 1 (Highest)\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello there\r\nsecond line\r\n";

    #[test]
    fn plain_message() {
        let m = parse(PLAIN.as_bytes()).unwrap();
        assert_eq!(m.from.as_ref().unwrap().addr, "bob@example.com");
        assert_eq!(m.from.as_ref().unwrap().name, "Bob Example");
        assert_eq!(m.from_header, "\"Bob Example\" <bob@example.com>");
        assert_eq!(m.to.len(), 2);
        assert_eq!(m.to[0].name, "Alice");
        assert_eq!(m.cc[0].addr, "dave@example.org");
        assert_eq!(m.subject, "გამარჯობა hi");
        assert_eq!(m.message_id.as_deref(), Some("abc123@example.com"));
        assert_eq!(m.in_reply_to.as_deref(), Some("parent@hashgram.io"));
        assert_eq!(m.references, vec!["root@x.org", "parent@hashgram.io"]);
        assert_eq!(m.date_ms, Some(1_789_237_500_000));
        assert_eq!(m.body_text, "Hello there\nsecond line\n");
        assert!(m.body_html.is_empty());
        assert!(m.attachments.is_empty());
        assert_eq!(
            m.auth.as_strings(),
            vec!["spf=pass", "dkim=pass", "dmarc=pass"]
        );
        assert_eq!(m.spam_score, 150);
        assert_eq!(m.importance, app::MailImportance::High);
    }

    #[test]
    fn alternative_and_attachment() {
        let raw = concat!(
            "From: bob@example.com\r\n",
            "To: alice@hashgram.io\r\n",
            "Subject: files\r\n",
            "Content-Type: multipart/mixed; boundary=\"outer\"\r\n\r\n",
            "--outer\r\n",
            "Content-Type: multipart/alternative; boundary=\"inner\"\r\n\r\n",
            "--inner\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n\r\n",
            "plain body\r\n",
            "--inner\r\n",
            "Content-Type: text/html; charset=utf-8\r\n\r\n",
            "<p>html <b>body</b></p>\r\n",
            "--inner--\r\n",
            "--outer\r\n",
            "Content-Type: application/pdf; name=\"report.pdf\"\r\n",
            "Content-Disposition: attachment; filename=\"../report.pdf\"\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\n",
            "JVBERi0xLjQK\r\n",
            "--outer\r\n",
            "Content-Type: image/png\r\n",
            "Content-ID: <img1@example.com>\r\n",
            "Content-Disposition: inline\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\n",
            "iVBORw0KGgo=\r\n",
            "--outer--\r\n"
        );
        let m = parse(raw.as_bytes()).unwrap();
        // The CRLF before a boundary belongs to the boundary (RFC 2046).
        assert_eq!(m.body_text, "plain body");
        assert_eq!(m.body_html.trim(), "<p>html <b>body</b></p>");
        assert_eq!(m.attachments.len(), 2);
        assert_eq!(m.attachments[0].name, ".._report.pdf");
        assert_eq!(m.attachments[0].mime, "application/pdf");
        assert_eq!(m.attachments[0].data, b"%PDF-1.4\n");
        assert_eq!(m.attachments[1].name, "part-2.png");
        assert_eq!(m.attachments[1].content_id, "img1@example.com");
        assert_eq!(m.attachments[1].data, b"\x89PNG\r\n\x1a\n");
        assert_eq!(m.auth, AuthResults::none());
        assert_eq!(m.spam_score, 0);
    }

    #[test]
    fn html_only_derives_text() {
        let raw = "From: a@example.com\r\nTo: b@hashgram.io\r\nSubject: s\r\nContent-Type: text/html\r\n\r\n<div>Hi<br>there</div>";
        let m = parse(raw.as_bytes()).unwrap();
        assert_eq!(m.body_text, "Hi\nthere");
        assert!(m.body_html.contains("<div>"));
    }

    #[test]
    fn auth_results_and_spam_parsing() {
        assert_eq!(parse_auth_results(None), AuthResults::none());
        let a = parse_auth_results(Some(
            "mx; spf=softfail (x) smtp.mailfrom=a; dkim=fail header.d=x; dkim=pass header.d=y",
        ));
        assert_eq!(a.spf, "softfail");
        assert_eq!(a.dkim, "pass"); // any passing signature counts
        assert_eq!(a.dmarc, "none");
        assert!(a.authenticated());
        assert_eq!(parse_spam_score(Some("7.25"), None), 725);
        assert_eq!(
            parse_spam_score(None, Some("Yes, score=12.0 required=5.0 tests=X")),
            1000
        );
        assert_eq!(parse_spam_score(Some("-3"), None), 0);
        assert_eq!(parse_spam_score(Some("nan"), None), 0);
        assert_eq!(parse_spam_score(None, None), 0);
    }

    #[test]
    fn garbage_headers_do_not_fail() {
        let raw = "From: not an address\r\nSubject: a\r\nb\r\n\r\nbody";
        let m = parse(raw.as_bytes()).unwrap();
        assert!(m.from.is_none());
        assert_eq!(m.from_header, "not an address");
        assert_eq!(m.body_text, "body");
        let m = parse(b"").unwrap();
        assert!(m.body_text.is_empty());
    }

    #[test]
    fn subject_is_one_line_and_bounded() {
        assert_eq!(clean_subject("a\r\n b\tc"), "a b c");
        assert!(clean_subject(&"x".repeat(2000)).len() <= MAX_SUBJECT);
    }

    #[test]
    fn attachment_limit_is_noted() {
        let mut raw = String::from("From: a@example.com\r\nTo: b@hashgram.io\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nt\r\n");
        for i in 0..(MAX_ATTACHMENTS + 3) {
            raw.push_str(&format!(
                "--b\r\nContent-Type: application/octet-stream; name=\"f{i}\"\r\n\r\nx\r\n"
            ));
        }
        raw.push_str("--b--\r\n");
        let m = parse(raw.as_bytes()).unwrap();
        assert_eq!(m.attachments.len(), MAX_ATTACHMENTS);
        assert!(m.body_text.contains("3 attachment(s) omitted"));
        assert_eq!(m.notes.len(), 1);
    }
}

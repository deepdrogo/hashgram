//! Native `MailMessage` → Internet message bytes.
//!
//! The output is deliberately conservative: CRLF everywhere, 7-bit-safe
//! transfer encodings (quoted-printable for text, base64 for binary),
//! encoded-words for anything non-ASCII in headers, one `Message-ID` under
//! our domain. Conservative output survives the widest range of receivers
//! and, because DKIM signs the bytes we emit, must not depend on anything
//! a later hop might normalise differently.
//!
//! Header order is fixed and the `Date`/`Message-ID`/boundary inputs come
//! from the caller, so rendering is a pure function — the round-trip test
//! feeds the output back through `mailparse`.

use hashgram_app::pb as app;

use super::{base64_folded, encode_header_value, format_mailbox, quoted_printable, rfc5322_date};

/// A recipient or sender line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mailbox {
    /// Display name (may be empty).
    pub name: String,
    /// `local@domain`.
    pub addr: String,
}

/// An attachment with its plaintext bytes (already decrypted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// File name.
    pub name: String,
    /// MIME type.
    pub mime: String,
    /// `Content-ID` (without brackets) for inline images; empty otherwise.
    pub content_id: String,
    /// Bytes.
    pub data: Vec<u8>,
}

/// Everything needed to render one message.
#[derive(Debug, Clone, Default)]
pub struct OutboundMail {
    /// `From`.
    pub from: Mailbox,
    /// `To`.
    pub to: Vec<Mailbox>,
    /// `Cc`.
    pub cc: Vec<Mailbox>,
    /// `Reply-To`, if the sender wants replies somewhere else.
    pub reply_to: Option<Mailbox>,
    /// `Subject`.
    pub subject: String,
    /// `Date`, unix ms.
    pub date_ms: u64,
    /// `Message-ID` value including angle brackets.
    pub message_id: String,
    /// `In-Reply-To` value including angle brackets.
    pub in_reply_to: Option<String>,
    /// `References` values including angle brackets, oldest first.
    pub references: Vec<String>,
    /// Plain body.
    pub body_text: String,
    /// Optional HTML body.
    pub body_html: String,
    /// Attachments.
    pub attachments: Vec<Attachment>,
    /// Importance.
    pub importance: app::MailImportance,
    /// Extra headers the caller wants first (e.g. `X-Hashgram-Origin`).
    /// Names must be ASCII tokens; values are encoded as needed.
    pub extra_headers: Vec<(String, String)>,
}

/// Rendered message plus the headers DKIM should sign (in order).
#[derive(Debug, Clone)]
pub struct Rendered {
    /// The bytes to hand to SMTP `DATA` (CRLF line endings).
    pub bytes: Vec<u8>,
    /// Header names present in the top-level header block, in emission
    /// order; DKIM signs the ones it is configured to.
    pub header_names: Vec<String>,
}

fn boundary(seed: &[u8], tag: &str) -> String {
    // Deterministic from caller-provided randomness so rendering is a
    // pure function; the seed itself is fresh per message.
    let h = blake3::hash(&[seed, tag.as_bytes()].concat());
    format!(
        "=_hg_{}_{}",
        tag,
        hex::encode(h.as_bytes().get(..12).unwrap_or(&[]))
    )
}

fn mailbox_list(list: &[Mailbox]) -> String {
    list.iter()
        .map(|m| format_mailbox(&m.name, &m.addr))
        .collect::<Vec<_>>()
        .join(",\r\n ")
}

fn fold_ids(ids: &[String]) -> String {
    // One id per continuation line keeps every line short.
    ids.join("\r\n ")
}

fn push_header(out: &mut String, names: &mut Vec<String>, name: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    out.push_str(name);
    out.push_str(": ");
    out.push_str(value);
    out.push_str("\r\n");
    names.push(name.to_owned());
}

fn text_part(out: &mut String, mime: &str, body: &str) {
    out.push_str(&format!("Content-Type: {mime}; charset=utf-8\r\n"));
    out.push_str("Content-Transfer-Encoding: quoted-printable\r\n\r\n");
    let qp = quoted_printable(body.as_bytes());
    out.push_str(&qp);
    if !qp.ends_with("\r\n") {
        out.push_str("\r\n");
    }
}

/// A MIME parameter: quoted-string for ASCII, RFC 2231 `name*=UTF-8''…`
/// percent-encoding otherwise (what receivers decode reliably; encoded-
/// words inside parameters are a widespread but non-standard habit).
fn param(name: &str, value: &str) -> String {
    let ascii_ok = value.bytes().all(|b| (0x20..0x7f).contains(&b));
    if ascii_ok {
        let escaped: String = value
            .chars()
            .flat_map(|c| match c {
                '"' | '\\' => vec!['\\', c],
                _ => vec![c],
            })
            .collect();
        return format!("{name}=\"{escaped}\"");
    }
    let mut enc = String::with_capacity(value.len() * 3);
    for b in value.bytes() {
        let attr_char = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            );
        if attr_char {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("{name}*=UTF-8''{enc}")
}

fn attachment_part(out: &mut String, a: &Attachment) {
    let mime = if a.mime.is_empty() {
        "application/octet-stream"
    } else {
        &a.mime
    };
    out.push_str(&format!(
        "Content-Type: {mime}; {}\r\n",
        param("name", &a.name)
    ));
    let disposition = if a.content_id.is_empty() {
        "attachment"
    } else {
        "inline"
    };
    out.push_str(&format!(
        "Content-Disposition: {disposition}; {}\r\n",
        param("filename", &a.name)
    ));
    if !a.content_id.is_empty() {
        out.push_str(&format!("Content-ID: <{}>\r\n", a.content_id));
    }
    out.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
    out.push_str(&base64_folded(&a.data));
}

impl OutboundMail {
    /// Renders. `seed` feeds the MIME boundaries (16 random bytes from the
    /// caller; any bytes work, uniqueness per message is what matters).
    #[must_use]
    pub fn render(&self, seed: &[u8]) -> Rendered {
        let mut out = String::with_capacity(4096 + self.body_text.len() + self.body_html.len());
        let mut names = Vec::new();
        for (k, v) in &self.extra_headers {
            if k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') && !k.is_empty() {
                push_header(&mut out, &mut names, k, &encode_header_value(v));
            }
        }
        push_header(
            &mut out,
            &mut names,
            "From",
            &format_mailbox(&self.from.name, &self.from.addr),
        );
        push_header(&mut out, &mut names, "To", &mailbox_list(&self.to));
        push_header(&mut out, &mut names, "Cc", &mailbox_list(&self.cc));
        if let Some(r) = &self.reply_to {
            push_header(
                &mut out,
                &mut names,
                "Reply-To",
                &format_mailbox(&r.name, &r.addr),
            );
        }
        push_header(
            &mut out,
            &mut names,
            "Subject",
            &encode_header_value(&self.subject),
        );
        push_header(&mut out, &mut names, "Date", &rfc5322_date(self.date_ms));
        push_header(&mut out, &mut names, "Message-ID", &self.message_id);
        if let Some(irt) = &self.in_reply_to {
            push_header(&mut out, &mut names, "In-Reply-To", irt);
        }
        if !self.references.is_empty() {
            push_header(
                &mut out,
                &mut names,
                "References",
                &fold_ids(&self.references),
            );
        }
        match self.importance {
            app::MailImportance::High => {
                push_header(&mut out, &mut names, "Importance", "high");
                push_header(&mut out, &mut names, "X-Priority", "1 (Highest)");
            }
            app::MailImportance::Low => {
                push_header(&mut out, &mut names, "Importance", "low");
                push_header(&mut out, &mut names, "X-Priority", "5 (Lowest)");
            }
            app::MailImportance::Normal => {}
        }
        push_header(&mut out, &mut names, "MIME-Version", "1.0");

        let has_html = !self.body_html.is_empty();
        let has_attachments = !self.attachments.is_empty();
        match (has_html, has_attachments) {
            (false, false) => {
                names.push("Content-Type".into());
                text_part(&mut out, "text/plain", &self.body_text);
            }
            (true, false) => {
                let b = boundary(seed, "alt");
                push_header(
                    &mut out,
                    &mut names,
                    "Content-Type",
                    &format!("multipart/alternative; boundary=\"{b}\""),
                );
                out.push_str("\r\n");
                self.alternative_body(&mut out, &b);
                out.push_str(&format!("--{b}--\r\n"));
            }
            (_, true) => {
                let outer = boundary(seed, "mix");
                push_header(
                    &mut out,
                    &mut names,
                    "Content-Type",
                    &format!("multipart/mixed; boundary=\"{outer}\""),
                );
                out.push_str("\r\n");
                out.push_str(&format!("--{outer}\r\n"));
                if has_html {
                    let inner = boundary(seed, "alt");
                    out.push_str(&format!(
                        "Content-Type: multipart/alternative; boundary=\"{inner}\"\r\n\r\n"
                    ));
                    self.alternative_body(&mut out, &inner);
                    out.push_str(&format!("--{inner}--\r\n"));
                } else {
                    text_part(&mut out, "text/plain", &self.body_text);
                }
                for a in &self.attachments {
                    out.push_str(&format!("--{outer}\r\n"));
                    attachment_part(&mut out, a);
                }
                out.push_str(&format!("--{outer}--\r\n"));
            }
        }
        Rendered {
            bytes: out.into_bytes(),
            header_names: names,
        }
    }

    fn alternative_body(&self, out: &mut String, b: &str) {
        out.push_str(&format!("--{b}\r\n"));
        text_part(out, "text/plain", &self.body_text);
        out.push_str(&format!("--{b}\r\n"));
        text_part(out, "text/html", &self.body_html);
    }
}

/// Sanity check on rendered output: every line ≤ 998 bytes and CRLF only.
/// Used by tests and by the bridge before signing.
#[must_use]
pub fn well_formed(bytes: &[u8]) -> bool {
    let mut start = 0;
    let n = bytes.len();
    while start < n {
        let rel = bytes
            .get(start..)
            .and_then(|s| s.iter().position(|&b| b == b'\n'));
        let Some(rel) = rel else {
            return false; // no trailing CRLF
        };
        let end = start + rel;
        if end == 0 || bytes.get(end - 1) != Some(&b'\r') || end - start > 999 {
            return false;
        }
        start = end + 1;
    }
    true
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
    use mailparse::MailHeaderMap;

    fn sample() -> OutboundMail {
        OutboundMail {
            from: Mailbox {
                name: "ალისა".into(),
                addr: "alice@hashgram.io".into(),
            },
            to: vec![
                Mailbox {
                    name: "Bob \"B\" Example".into(),
                    addr: "bob@example.com".into(),
                },
                Mailbox {
                    name: String::new(),
                    addr: "carol@example.org".into(),
                },
            ],
            cc: vec![],
            reply_to: None,
            subject: "Re: გამარჯობა — long subject that goes on and on to exercise folding of encoded words".into(),
            date_ms: 1_789_237_500_000,
            message_id: "<00112233445566778899aabbccddeeff@hashgram.io>".into(),
            in_reply_to: Some("<parent@example.com>".into()),
            references: vec!["<root@example.com>".into(), "<parent@example.com>".into()],
            body_text: "Hello Bob,\n\nline with = sign and trailing space \n.leading dot\n\n— ალისა".into(),
            body_html: "<p>Hello <b>Bob</b>,</p><p>— ალისა</p>".into(),
            attachments: vec![
                Attachment {
                    name: "report ინვოისი.pdf".into(),
                    mime: "application/pdf".into(),
                    content_id: String::new(),
                    data: b"%PDF-1.4\n%binary\x00\xff".to_vec(),
                },
                Attachment {
                    name: "logo.png".into(),
                    mime: "image/png".into(),
                    content_id: "logo@hashgram.io".into(),
                    data: vec![0x89, b'P', b'N', b'G'],
                },
            ],
            importance: app::MailImportance::High,
            extra_headers: vec![("X-Hashgram-Origin".into(), "native".into())],
        }
    }

    #[test]
    fn round_trips_through_mailparse() {
        let m = sample();
        let r = m.render(&[1u8; 16]);
        assert!(
            well_formed(&r.bytes),
            "{}",
            String::from_utf8_lossy(&r.bytes)
        );
        let parsed = mailparse::parse_mail(&r.bytes).unwrap();
        let h = &parsed.headers;
        assert_eq!(h.get_first_value("Subject").unwrap(), m.subject);
        assert_eq!(h.get_first_value("Message-ID").unwrap(), m.message_id);
        assert_eq!(
            h.get_first_value("In-Reply-To").unwrap(),
            "<parent@example.com>"
        );
        assert_eq!(
            h.get_first_value("Date").unwrap(),
            "Sat, 12 Sep 2026 18:25:00 +0000"
        );
        assert_eq!(h.get_first_value("X-Hashgram-Origin").unwrap(), "native");
        assert_eq!(h.get_first_value("Importance").unwrap(), "high");
        let refs = h.get_first_value("References").unwrap();
        assert!(refs.contains("<root@example.com>") && refs.contains("<parent@example.com>"));
        let from = mailparse::addrparse(&h.get_first_value("From").unwrap()).unwrap();
        match &from[0] {
            mailparse::MailAddr::Single(s) => {
                assert_eq!(s.addr, "alice@hashgram.io");
                assert_eq!(s.display_name.as_deref(), Some("ალისა"));
            }
            _ => panic!(),
        }
        let to = mailparse::addrparse(&h.get_first_value("To").unwrap()).unwrap();
        assert_eq!(to.count_addrs(), 2);
        match &to[0] {
            mailparse::MailAddr::Single(s) => {
                assert_eq!(s.display_name.as_deref(), Some("Bob \"B\" Example"))
            }
            _ => panic!(),
        }

        // Structure: mixed(alternative(plain, html), pdf, png)
        assert_eq!(parsed.ctype.mimetype, "multipart/mixed");
        assert_eq!(parsed.subparts.len(), 3);
        let alt = &parsed.subparts[0];
        assert_eq!(alt.ctype.mimetype, "multipart/alternative");
        assert_eq!(alt.subparts[0].ctype.mimetype, "text/plain");
        // Bodies come back with CRLF line endings (the wire form).
        assert_eq!(
            alt.subparts[0].get_body().unwrap(),
            m.body_text.replace('\n', "\r\n")
        );
        assert_eq!(alt.subparts[1].ctype.mimetype, "text/html");
        assert_eq!(alt.subparts[1].get_body().unwrap(), m.body_html);
        let pdf = &parsed.subparts[1];
        assert_eq!(pdf.ctype.mimetype, "application/pdf");
        assert_eq!(pdf.get_body_raw().unwrap(), m.attachments[0].data);
        let disp = pdf.get_content_disposition();
        assert_eq!(disp.disposition, mailparse::DispositionType::Attachment);
        assert_eq!(disp.params.get("filename").unwrap(), "report ინვოისი.pdf");
        let png = &parsed.subparts[2];
        assert_eq!(png.get_body_raw().unwrap(), m.attachments[1].data);
        assert_eq!(
            png.headers.get_first_value("Content-ID").unwrap(),
            "<logo@hashgram.io>"
        );
        assert_eq!(
            png.get_content_disposition().disposition,
            mailparse::DispositionType::Inline
        );

        // Our inbound parser understands our own output too.
        let back = super::super::inbound::parse(&r.bytes).unwrap();
        assert_eq!(back.body_text, m.body_text);
        assert_eq!(back.attachments.len(), 2);
        assert_eq!(
            back.message_id.as_deref(),
            Some("00112233445566778899aabbccddeeff@hashgram.io")
        );
    }

    #[test]
    fn plain_only_and_alternative_only() {
        let mut m = sample();
        m.attachments.clear();
        m.body_html.clear();
        let r = m.render(&[2u8; 16]);
        let parsed = mailparse::parse_mail(&r.bytes).unwrap();
        assert_eq!(parsed.ctype.mimetype, "text/plain");
        assert_eq!(
            parsed
                .get_body()
                .unwrap()
                .replace("\r\n", "\n")
                .trim_end_matches('\n'),
            m.body_text
        );
        assert!(r.header_names.contains(&"Content-Type".to_owned()));

        let mut m = sample();
        m.attachments.clear();
        let r = m.render(&[3u8; 16]);
        let parsed = mailparse::parse_mail(&r.bytes).unwrap();
        assert_eq!(parsed.ctype.mimetype, "multipart/alternative");
        assert_eq!(parsed.subparts.len(), 2);
    }

    #[test]
    fn rendering_is_deterministic() {
        let m = sample();
        assert_eq!(m.render(&[9u8; 16]).bytes, m.render(&[9u8; 16]).bytes);
        assert_ne!(m.render(&[9u8; 16]).bytes, m.render(&[8u8; 16]).bytes);
    }

    #[test]
    fn well_formed_checks() {
        assert!(well_formed(b"A: b\r\n\r\nx\r\n"));
        assert!(!well_formed(b"A: b\n\nx\n"));
        assert!(!well_formed(b"A: b\r\n\r\nx"));
        let long = format!("A: {}\r\n", "x".repeat(1000));
        assert!(!well_formed(long.as_bytes()));
    }
}

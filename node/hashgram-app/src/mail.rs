//! HashMail: the message model, its bounds, address forms and threading.
//!
//! A `MailMessage` is plaintext only inside the MLS group that carries it.
//! The rules here exist because a malicious *member* can send anything and
//! a reader must not crash, over-allocate or mis-thread on it, and because
//! bridging to Internet mail must be lossless (RFC 5322 threading fields).
//!
//! # Address forms
//!
//! | Form | Example | Authority |
//! | --- | --- | --- |
//! | account address | `hash1e0tl…vfl5` | the chain |
//! | username | `@alice` or `alice` | `x/username` lookup → owner address |
//! | mail address | `alice@hashgram.io` | local part is the username |
//!
//! Only the account address is an identity. The other two are resolved
//! through the chain by the SDK before a message is built, and are carried
//! in `MailAddress.username` as a hint that readers re-resolve.

use crate::ids::{
    now_ms, random_id, require_address, require_id, require_id_or_empty, require_str,
    require_str_max,
};
use crate::pb;
use crate::version::{check_body, MAIL_VERSION};
use crate::AppError;

/// Mail domain of Hashgram usernames.
pub const MAIL_DOMAIN: &str = "hashgram.io";

/// Bounds. Every reader enforces them; every builder refuses to exceed them.
pub const MAX_RECIPIENTS: usize = 100;
/// Subject bytes.
pub const MAX_SUBJECT: usize = 998;
/// Plain body bytes.
pub const MAX_BODY_TEXT: usize = 1024 * 1024;
/// HTML body bytes.
pub const MAX_BODY_HTML: usize = 1024 * 1024;
/// Attachments per message.
pub const MAX_ATTACHMENTS: usize = 64;
/// Inline attachment bytes.
pub const MAX_INLINE_ATTACHMENT: usize = 64 * 1024;
/// `references` entries.
pub const MAX_REFERENCES: usize = 64;
/// Labels per message.
pub const MAX_LABELS: usize = 32;
/// Display name / username bytes.
pub const MAX_NAME: usize = 128;
/// Attachment file name bytes.
pub const MAX_FILENAME: usize = 255;
/// MIME type bytes.
pub const MAX_MIME: usize = 128;
/// External header bytes.
pub const MAX_HEADER: usize = 998;

/// A parsed recipient/sender designator, before chain resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressForm {
    /// `hash1…`
    Address(String),
    /// `@alice`, `alice`, or `alice@hashgram.io`.
    Username(String),
    /// `someone@example.com` — not a Hashgram identity; needs the gateway.
    External(String),
}

/// Parses a user-typed designator.
pub fn parse_address(input: &str) -> Result<AddressForm, AppError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(AppError::Invalid("empty address".into()));
    }
    if crate::ids::is_address(s) {
        return Ok(AddressForm::Address(s.to_owned()));
    }
    if let Some(name) = s.strip_prefix('@') {
        return username(name).map(AddressForm::Username);
    }
    if let Some((local, domain)) = s.rsplit_once('@') {
        if domain.eq_ignore_ascii_case(MAIL_DOMAIN) {
            return username(local).map(AddressForm::Username);
        }
        if local.is_empty() || domain.is_empty() || !domain.contains('.') || s.len() > 254 {
            return Err(AppError::Invalid(format!("not a mail address: {s}")));
        }
        return Ok(AddressForm::External(s.to_ascii_lowercase()));
    }
    username(s).map(AddressForm::Username)
}

fn username(name: &str) -> Result<String, AppError> {
    let n = name.trim().to_lowercase();
    if n.len() < 2 || n.len() > 32 {
        return Err(AppError::Invalid(format!(
            "username length out of range: {name}"
        )));
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(AppError::Invalid(format!(
            "username has invalid characters: {name}"
        )));
    }
    Ok(n)
}

/// `alice` → `alice@hashgram.io`.
#[must_use]
pub fn mail_address_for(username: &str) -> String {
    format!("{username}@{MAIL_DOMAIN}")
}

/// Validates a `MailAddress` field.
pub fn validate_address(what: &str, a: &pb::MailAddress) -> Result<(), AppError> {
    require_address(&format!("{what}.address"), &a.address)?;
    require_str_max(&format!("{what}.username"), &a.username, MAX_NAME)?;
    require_str_max(&format!("{what}.display_name"), &a.display_name, MAX_NAME)?;
    Ok(())
}

/// Validates an attachment.
pub fn validate_attachment(i: usize, a: &pb::MailAttachment) -> Result<(), AppError> {
    let w = format!("attachments[{i}]");
    require_str(&format!("{w}.name"), &a.name, MAX_FILENAME)?;
    if a.name.contains('/') || a.name.contains('\\') || a.name == "." || a.name == ".." {
        return Err(AppError::Invalid(format!(
            "{w}.name contains a path separator"
        )));
    }
    require_str_max(&format!("{w}.mime"), &a.mime, MAX_MIME)?;
    require_str_max(&format!("{w}.content_id"), &a.content_id, MAX_NAME)?;
    if !a.plaintext_hash.is_empty() {
        crate::ids::require_hash(&format!("{w}.plaintext_hash"), &a.plaintext_hash)?;
    }
    match &a.source {
        Some(pb::mail_attachment::Source::InlineData(d)) => {
            if d.len() > MAX_INLINE_ATTACHMENT {
                return Err(AppError::Invalid(format!(
                    "{w} inline data is {} bytes, maximum {MAX_INLINE_ATTACHMENT}",
                    d.len()
                )));
            }
            if a.size != d.len() as u64 {
                return Err(AppError::Invalid(format!(
                    "{w}.size does not match inline data"
                )));
            }
        }
        Some(pb::mail_attachment::Source::Blob(b)) => {
            crate::ids::require_hash(&format!("{w}.blob.cid"), &b.cid)?;
            if b.key.len() != 32 || b.nonce.len() != 24 {
                return Err(AppError::Invalid(format!("{w}.blob key/nonce length")));
            }
        }
        Some(pb::mail_attachment::Source::Drive(c)) => {
            crate::drive::validate_capability(c)?;
        }
        None => return Err(AppError::Invalid(format!("{w} has no source"))),
    }
    Ok(())
}

/// Validates a received or about-to-be-sent `MailMessage`.
pub fn validate(m: &pb::MailMessage) -> Result<(), AppError> {
    check_body("MailMessage", m.version, MAIL_VERSION)?;
    require_id("message_id", &m.message_id)?;
    require_id("thread_id", &m.thread_id)?;
    let from = m
        .from
        .as_ref()
        .ok_or_else(|| AppError::Invalid("from is missing".into()))?;
    validate_address("from", from)?;
    if m.to.len() + m.cc.len() > MAX_RECIPIENTS {
        return Err(AppError::Invalid(format!(
            "{} recipients, maximum {MAX_RECIPIENTS}",
            m.to.len() + m.cc.len()
        )));
    }
    if m.to.is_empty() && m.cc.is_empty() {
        return Err(AppError::Invalid("no recipients".into()));
    }
    for (i, a) in m.to.iter().enumerate() {
        validate_address(&format!("to[{i}]"), a)?;
    }
    for (i, a) in m.cc.iter().enumerate() {
        validate_address(&format!("cc[{i}]"), a)?;
    }
    require_str_max("subject", &m.subject, MAX_SUBJECT)?;
    if m.subject.contains('\n') || m.subject.contains('\r') {
        return Err(AppError::Invalid("subject contains a line break".into()));
    }
    require_id_or_empty("in_reply_to", &m.in_reply_to)?;
    if m.references.len() > MAX_REFERENCES {
        return Err(AppError::Invalid(format!(
            "{} references, maximum {MAX_REFERENCES}",
            m.references.len()
        )));
    }
    for (i, r) in m.references.iter().enumerate() {
        require_id(&format!("references[{i}]"), r)?;
    }
    if m.body_text.len() > MAX_BODY_TEXT {
        return Err(AppError::Invalid("body_text too large".into()));
    }
    if m.body_html.len() > MAX_BODY_HTML {
        return Err(AppError::Invalid("body_html too large".into()));
    }
    if m.attachments.len() > MAX_ATTACHMENTS {
        return Err(AppError::Invalid(format!(
            "{} attachments, maximum {MAX_ATTACHMENTS}",
            m.attachments.len()
        )));
    }
    for (i, a) in m.attachments.iter().enumerate() {
        validate_attachment(i, a)?;
    }
    if m.labels.len() > MAX_LABELS {
        return Err(AppError::Invalid("too many labels".into()));
    }
    for l in &m.labels {
        require_str("label", l, 64)?;
    }
    if m.origin == pb::MailOrigin::ExternalGateway as i32 {
        let e = m
            .external
            .as_ref()
            .ok_or_else(|| AppError::Invalid("external origin without external meta".into()))?;
        require_address("external.gateway", &e.gateway)?;
        require_str_max("external.from_header", &e.from_header, MAX_HEADER)?;
        require_str_max(
            "external.message_id_header",
            &e.message_id_header,
            MAX_HEADER,
        )?;
        if e.auth_results.len() > 16 {
            return Err(AppError::Invalid("too many auth_results".into()));
        }
        for r in &e.auth_results {
            require_str("auth_result", r, 128)?;
        }
        if e.spam_score > 1000 {
            return Err(AppError::Invalid("spam_score out of range".into()));
        }
    } else if m.external.is_some() {
        return Err(AppError::Invalid(
            "external meta on a native message".into(),
        ));
    }
    Ok(())
}

/// Constructs messages with the invariants held.
#[derive(Debug, Clone, Default)]
pub struct Draft {
    /// Sender.
    pub from: pb::MailAddress,
    /// To.
    pub to: Vec<pb::MailAddress>,
    /// CC.
    pub cc: Vec<pb::MailAddress>,
    /// BCC — each gets a separate copy (see [`Draft::build_all`]).
    pub bcc: Vec<pb::MailAddress>,
    /// Subject.
    pub subject: String,
    /// Plain body.
    pub body_text: String,
    /// Optional HTML body.
    pub body_html: String,
    /// Attachments.
    pub attachments: Vec<pb::MailAttachment>,
    /// Reply target: the message being answered.
    pub in_reply_to: Option<ReplyTarget>,
    /// Advisory expiry.
    pub expire_after_secs: u32,
    /// Ask for a read receipt.
    pub request_read_receipt: bool,
    /// Importance.
    pub importance: pb::MailImportance,
    /// Labels.
    pub labels: Vec<String>,
}

/// What a reply points at.
#[derive(Debug, Clone)]
pub struct ReplyTarget {
    /// `message_id` of the message being answered.
    pub message_id: Vec<u8>,
    /// Its `thread_id`.
    pub thread_id: Vec<u8>,
    /// Its `references` (we append its id).
    pub references: Vec<Vec<u8>>,
}

/// The copies a draft expands to.
#[derive(Debug, Clone)]
pub struct Outgoing {
    /// The message for the To+CC group (absent if there are none and only
    /// BCC recipients exist).
    pub main: Option<pb::MailMessage>,
    /// One copy per BCC recipient, `bcc_copy = true`, addressed only to
    /// them at the transport layer. Same `message_id`.
    pub bcc_copies: Vec<(pb::MailAddress, pb::MailMessage)>,
}

impl Draft {
    /// Builds the message(s). One `message_id` is shared by every copy, so
    /// receipts and dedup line up across recipients.
    pub fn build_all(&self) -> Result<Outgoing, AppError> {
        if self.to.is_empty() && self.cc.is_empty() && self.bcc.is_empty() {
            return Err(AppError::Invalid("no recipients".into()));
        }
        if self.to.len() + self.cc.len() + self.bcc.len() > MAX_RECIPIENTS {
            return Err(AppError::Invalid("too many recipients".into()));
        }
        let message_id = random_id()?;
        let (thread_id, in_reply_to, references) = match &self.in_reply_to {
            Some(t) => {
                require_id("reply thread_id", &t.thread_id)?;
                require_id("reply message_id", &t.message_id)?;
                let mut refs = t.references.clone();
                refs.push(t.message_id.clone());
                while refs.len() > MAX_REFERENCES {
                    // Keep the root and the most recent ancestors.
                    refs.remove(1);
                }
                (t.thread_id.clone(), t.message_id.clone(), refs)
            }
            None => (message_id.clone(), Vec::new(), Vec::new()),
        };
        let base = pb::MailMessage {
            version: MAIL_VERSION,
            message_id,
            thread_id,
            from: Some(self.from.clone()),
            to: self.to.clone(),
            cc: self.cc.clone(),
            bcc_copy: false,
            created_at_ms: now_ms(),
            subject: self.subject.clone(),
            in_reply_to,
            references,
            body_text: self.body_text.clone(),
            body_html: self.body_html.clone(),
            attachments: self.attachments.clone(),
            expire_after_secs: self.expire_after_secs,
            origin: pb::MailOrigin::Native as i32,
            external: None,
            request_read_receipt: self.request_read_receipt,
            importance: self.importance as i32,
            labels: self.labels.clone(),
        };
        let main = if self.to.is_empty() && self.cc.is_empty() {
            None
        } else {
            validate(&base)?;
            Some(base.clone())
        };
        let mut bcc_copies = Vec::with_capacity(self.bcc.len());
        for r in &self.bcc {
            let mut copy = base.clone();
            copy.bcc_copy = true;
            if copy.to.is_empty() && copy.cc.is_empty() {
                // A pure-BCC message still needs a visible recipient line;
                // RFC 5322 practice is "undisclosed recipients". We put the
                // sender as To so `validate` holds and the reader shows it
                // as a BCC copy.
                copy.to = vec![self.from.clone()];
            }
            validate(&copy)?;
            bcc_copies.push((r.clone(), copy));
        }
        Ok(Outgoing { main, bcc_copies })
    }
}

/// Who a reply / reply-all goes to, given the local user's address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyRecipients {
    /// To line.
    pub to: Vec<pb::MailAddress>,
    /// CC line.
    pub cc: Vec<pb::MailAddress>,
}

/// Reply to the sender only.
#[must_use]
pub fn reply_recipients(m: &pb::MailMessage) -> ReplyRecipients {
    ReplyRecipients {
        to: m.from.clone().into_iter().collect(),
        cc: Vec::new(),
    }
}

/// Reply to everyone except us. A BCC copy replies to the sender only, so
/// the hidden recipient does not reveal itself to the visible ones.
#[must_use]
pub fn reply_all_recipients(m: &pb::MailMessage, me: &str) -> ReplyRecipients {
    if m.bcc_copy {
        return reply_recipients(m);
    }
    let mut to: Vec<pb::MailAddress> = m.from.clone().into_iter().collect();
    for a in &m.to {
        if a.address != me && !to.iter().any(|x| x.address == a.address) {
            to.push(a.clone());
        }
    }
    let mut cc = Vec::new();
    for a in &m.cc {
        if a.address != me
            && !to.iter().any(|x| x.address == a.address)
            && !cc.iter().any(|x: &pb::MailAddress| x.address == a.address)
        {
            cc.push(a.clone());
        }
    }
    ReplyRecipients { to, cc }
}

/// The participant set a message travels in (sender + To + CC), sorted and
/// de-duplicated. The SDK keys MLS groups by this.
#[must_use]
pub fn participant_set(m: &pb::MailMessage) -> Vec<String> {
    let mut s: Vec<String> = m
        .from
        .iter()
        .chain(m.to.iter())
        .chain(m.cc.iter())
        .map(|a| a.address.clone())
        .collect();
    s.sort();
    s.dedup();
    s
}

/// Subject normalisation for thread grouping in the UI (strips `Re:`/`Fwd:`
/// prefixes, case-insensitive, collapses whitespace).
#[must_use]
pub fn normalised_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let stripped = ["re:", "fwd:", "fw:", "aw:", "wg:"]
            .iter()
            .find_map(|p| lower.starts_with(p).then(|| s[p.len()..].trim_start()));
        match stripped {
            Some(rest) if rest.len() < s.len() => s = rest,
            _ => break,
        }
    }
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Forward: a new thread with the original as an inline quotation and its
/// attachments carried over unchanged (they are references, no re-upload).
#[must_use]
pub fn forward_draft(from: pb::MailAddress, original: &pb::MailMessage) -> Draft {
    let quoted = format!(
        "\n\n---------- Forwarded message ----------\nFrom: {}\nDate: {}\nSubject: {}\n\n{}",
        original
            .from
            .as_ref()
            .map(|a| if a.username.is_empty() {
                a.address.clone()
            } else {
                mail_address_for(&a.username)
            })
            .unwrap_or_default(),
        original.created_at_ms,
        original.subject,
        original.body_text
    );
    Draft {
        from,
        subject: if normalised_subject(&original.subject) == original.subject.to_lowercase() {
            format!("Fwd: {}", original.subject)
        } else {
            original.subject.clone()
        },
        body_text: quoted,
        attachments: original.attachments.clone(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(a: &str, u: &str) -> pb::MailAddress {
        pb::MailAddress {
            address: a.into(),
            username: u.into(),
            display_name: String::new(),
        }
    }
    const A: &str = "hash1e0tl2hff03hu4g3sawcjqa2p9tc4uh24e4vfl5";
    const B: &str = "hash178njz3gft77798ssh6lrqxh2hskw5dm68f2uh5";
    const C: &str = "hash1znsxwr000000000000000000000000000000000";

    #[test]
    fn parse_forms() {
        assert_eq!(parse_address(A).unwrap(), AddressForm::Address(A.into()));
        assert_eq!(
            parse_address("@Alice").unwrap(),
            AddressForm::Username("alice".into())
        );
        assert_eq!(
            parse_address("alice@HASHGRAM.io").unwrap(),
            AddressForm::Username("alice".into())
        );
        assert_eq!(
            parse_address("bob").unwrap(),
            AddressForm::Username("bob".into())
        );
        assert_eq!(
            parse_address("Someone@Example.com").unwrap(),
            AddressForm::External("someone@example.com".into())
        );
        assert!(parse_address("").is_err());
        assert!(parse_address("a").is_err());
        assert!(parse_address("has space").is_err());
        assert!(parse_address("@x/y").is_err());
    }

    #[test]
    fn build_thread_and_bcc() {
        let d = Draft {
            from: addr(A, "alice"),
            to: vec![addr(B, "bob")],
            bcc: vec![addr(C, "carol")],
            subject: "Hello".into(),
            body_text: "hi".into(),
            ..Default::default()
        };
        let out = d.build_all().unwrap();
        let main = out.main.unwrap();
        assert_eq!(main.thread_id, main.message_id);
        assert!(!main.bcc_copy);
        assert_eq!(out.bcc_copies.len(), 1);
        let (to, copy) = &out.bcc_copies[0];
        assert_eq!(to.address, C);
        assert!(copy.bcc_copy);
        assert_eq!(copy.message_id, main.message_id);
        // BCC copy shows the same To line (like SMTP) but its reply-all is
        // sender-only.
        assert_eq!(copy.to[0].address, B);
        let ra = reply_all_recipients(copy, C);
        assert_eq!(ra.to.len(), 1);
        assert_eq!(ra.to[0].address, A);

        // Reply keeps the thread and extends references.
        let reply = Draft {
            from: addr(B, "bob"),
            to: vec![addr(A, "alice")],
            subject: "Re: Hello".into(),
            body_text: "yo".into(),
            in_reply_to: Some(ReplyTarget {
                message_id: main.message_id.clone(),
                thread_id: main.thread_id.clone(),
                references: main.references.clone(),
            }),
            ..Default::default()
        }
        .build_all()
        .unwrap()
        .main
        .unwrap();
        assert_eq!(reply.thread_id, main.thread_id);
        assert_eq!(reply.in_reply_to, main.message_id);
        assert_eq!(reply.references, vec![main.message_id.clone()]);
        assert_eq!(participant_set(&reply), {
            let mut v = vec![A.to_string(), B.to_string()];
            v.sort();
            v
        });
    }

    #[test]
    fn reply_all_excludes_me_and_dedups() {
        let m = pb::MailMessage {
            from: Some(addr(A, "")),
            to: vec![addr(B, ""), addr(C, "")],
            cc: vec![addr(A, ""), addr(B, "")],
            ..Default::default()
        };
        let r = reply_all_recipients(&m, B);
        assert_eq!(
            r.to.iter().map(|a| a.address.as_str()).collect::<Vec<_>>(),
            vec![A, C]
        );
        assert!(r.cc.is_empty());
    }

    #[test]
    fn validation_bounds() {
        let mut d = Draft {
            from: addr(A, "alice"),
            to: vec![addr(B, "bob")],
            subject: "x".into(),
            ..Default::default()
        };
        assert!(d.build_all().is_ok());
        d.subject = "a\nb".into();
        assert!(d.build_all().is_err());
        d.subject = "ok".into();
        d.attachments = vec![pb::MailAttachment {
            name: "../evil".into(),
            source: Some(pb::mail_attachment::Source::InlineData(vec![1])),
            size: 1,
            ..Default::default()
        }];
        assert!(d.build_all().is_err());
        d.attachments = vec![pb::MailAttachment {
            name: "big.bin".into(),
            size: (MAX_INLINE_ATTACHMENT + 1) as u64,
            source: Some(pb::mail_attachment::Source::InlineData(vec![
                0;
                MAX_INLINE_ATTACHMENT
                    + 1
            ])),
            ..Default::default()
        }];
        assert!(d.build_all().is_err());
        d.attachments.clear();
        d.to = (0..MAX_RECIPIENTS + 1).map(|_| addr(B, "")).collect();
        assert!(d.build_all().is_err());
    }

    #[test]
    fn external_meta_consistency() {
        let mut m = Draft {
            from: addr(A, "alice"),
            to: vec![addr(B, "bob")],
            ..Default::default()
        }
        .build_all()
        .unwrap()
        .main
        .unwrap();
        m.external = Some(pb::ExternalMailMeta::default());
        assert!(validate(&m).is_err());
        m.origin = pb::MailOrigin::ExternalGateway as i32;
        assert!(validate(&m).is_err()); // gateway address missing
        m.external = Some(pb::ExternalMailMeta {
            gateway: C.into(),
            from_header: "x@example.com".into(),
            ..Default::default()
        });
        assert!(validate(&m).is_ok());
    }

    #[test]
    fn subject_normalisation() {
        assert_eq!(
            normalised_subject("Re: RE: Fwd:  Hello   World"),
            "hello world"
        );
        assert_eq!(normalised_subject("Hello"), "hello");
        assert_eq!(normalised_subject("Re:"), "");
    }

    #[test]
    fn forward_keeps_attachments() {
        let orig = pb::MailMessage {
            from: Some(addr(B, "bob")),
            subject: "Invoice".into(),
            body_text: "see attached".into(),
            attachments: vec![pb::MailAttachment {
                name: "inv.pdf".into(),
                size: 3,
                source: Some(pb::mail_attachment::Source::InlineData(vec![1, 2, 3])),
                ..Default::default()
            }],
            ..Default::default()
        };
        let d = forward_draft(addr(A, "alice"), &orig);
        assert_eq!(d.subject, "Fwd: Invoice");
        assert_eq!(d.attachments.len(), 1);
        assert!(d.body_text.contains("bob@hashgram.io"));
    }
}

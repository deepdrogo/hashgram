//! The `AppMessage` envelope: building, encoding into a `ChatMessage`, and
//! the reader-side gate that turns anything unknown into `Unsupported`.

use hashgram_proto::chat;
use hashgram_proto::limits::WIRE_VERSION;
use prost::Message;

use crate::ids::{now_ms, random_id, require_id};
use crate::pb;
use crate::version::{check, APP_ENVELOPE_MAX_READ, APP_ENVELOPE_VERSION};
use crate::AppError;

/// Largest encoded `AppMessage` a reader will decode. An MLS application
/// message is bounded by the envelope ciphertext limit (256 KiB) on the
/// wire, so this is a defence against a member sending a maximal one with
/// an absurd repeated field, not a transport bound.
pub const MAX_APP_MESSAGE_BYTES: usize = 256 * 1024;

/// Builds an `AppMessage` around a body with a fresh id and the current
/// envelope version.
pub fn wrap(body: pb::app_message::Body) -> Result<pb::AppMessage, AppError> {
    Ok(pb::AppMessage {
        version: APP_ENVELOPE_VERSION,
        min_reader_version: 0,
        id: random_id()?,
        sent_at_ms: now_ms(),
        body: Some(body),
    })
}

/// Puts an `AppMessage` into the `ChatMessage` shape the MLS transport
/// carries (`kind = CHAT_KIND_APP`). The chat-level `id` mirrors the app
/// id so receipts and dedup work at either layer.
#[must_use]
pub fn to_chat(app: pb::AppMessage) -> chat::ChatMessage {
    chat::ChatMessage {
        version: WIRE_VERSION,
        kind: chat::ChatKind::App as i32,
        id: app.id.clone(),
        timestamp_ms: app.sent_at_ms,
        app: Some(app),
        ..Default::default()
    }
}

/// Encoded bytes ready for `MlsClient::encrypt`.
pub fn encode_for_mls(app: pb::AppMessage) -> Result<Vec<u8>, AppError> {
    let bytes = to_chat(app).encode_to_vec();
    if bytes.len() > MAX_APP_MESSAGE_BYTES {
        return Err(AppError::Invalid(format!(
            "application message is {} bytes, maximum {MAX_APP_MESSAGE_BYTES}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

/// What a reader gets back from [`open`].
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Opened {
    /// Not an application message (a legacy chat line); the caller renders
    /// it through the chat path.
    Chat(chat::ChatMessage),
    /// A decoded, version-checked application message with a body.
    App(pb::AppMessage),
}

/// Decodes MLS application plaintext and applies the version gate.
///
/// Returns `Unsupported` for: an envelope version newer than this build, a
/// `min_reader_version` above this build, an unknown body arm, or a
/// malformed id. The caller must keep the ciphertext when it sees
/// `Unsupported` so a later build can decode it.
pub fn open(plaintext: &[u8]) -> Result<Opened, AppError> {
    if plaintext.len() > MAX_APP_MESSAGE_BYTES {
        return Err(AppError::Invalid("application message too large".into()));
    }
    let msg = chat::ChatMessage::decode(plaintext)?;
    if msg.kind != chat::ChatKind::App as i32 {
        return Ok(Opened::Chat(msg));
    }
    let app = msg
        .app
        .ok_or_else(|| AppError::Unsupported("CHAT_KIND_APP without an app payload".into()))?;
    // Envelope version 0 is read as 1 (see version::check doc).
    let v = if app.version == 0 { 1 } else { app.version };
    check(
        "AppMessage",
        v,
        app.min_reader_version,
        APP_ENVELOPE_MAX_READ,
    )?;
    require_id("AppMessage.id", &app.id)?;
    if app.body.is_none() {
        return Err(AppError::Unsupported(
            "application message body type is not known to this build".into(),
        ));
    }
    Ok(Opened::App(app))
}

/// A human-readable kind name for logging and UI grouping. Never includes
/// content.
#[must_use]
pub fn kind_name(app: &pb::AppMessage) -> &'static str {
    use pb::app_message::Body as B;
    match &app.body {
        Some(B::Mail(_)) => "mail",
        Some(B::MailReceipt(_)) => "mail_receipt",
        Some(B::DriveShare(_)) => "drive_share",
        Some(B::DriveShareUpdate(_)) => "drive_share_update",
        Some(B::DriveShareRevoke(_)) => "drive_share_revoke",
        Some(B::ContactRequest(_)) => "contact_request",
        Some(B::ContactResponse(_)) => "contact_response",
        Some(B::ProfileCard(_)) => "profile_card",
        Some(B::CircleEvent(_)) => "circle_event",
        Some(B::SpaceEvent(_)) => "space_event",
        Some(B::DeviceSync(_)) => "device_sync",
        None => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> pb::app_message::Body {
        pb::app_message::Body::MailReceipt(pb::MailReceipt {
            version: 1,
            message_id: vec![7; 16],
            kind: pb::MailReceiptKind::Delivered as i32,
            at_ms: 1,
        })
    }

    #[test]
    fn round_trip() {
        let app = wrap(receipt()).unwrap();
        let bytes = encode_for_mls(app.clone()).unwrap();
        match open(&bytes).unwrap() {
            Opened::App(a) => {
                assert_eq!(a.id, app.id);
                assert_eq!(kind_name(&a), "mail_receipt");
            }
            Opened::Chat(_) => panic!("expected app"),
        }
    }

    #[test]
    fn legacy_chat_passes_through() {
        let m = chat::ChatMessage {
            version: 1,
            kind: chat::ChatKind::Text as i32,
            id: vec![1; 16],
            text: "hi".into(),
            ..Default::default()
        };
        match open(&m.encode_to_vec()).unwrap() {
            Opened::Chat(c) => assert_eq!(c.text, "hi"),
            Opened::App(_) => panic!("expected chat"),
        }
    }

    #[test]
    fn newer_envelope_is_unsupported_not_partial() {
        let mut app = wrap(receipt()).unwrap();
        app.version = 99;
        let bytes = encode_for_mls(app).unwrap();
        assert!(matches!(open(&bytes), Err(AppError::Unsupported(_))));
        let mut app = wrap(receipt()).unwrap();
        app.min_reader_version = 2;
        let bytes = encode_for_mls(app).unwrap();
        assert!(matches!(open(&bytes), Err(AppError::Unsupported(_))));
    }

    #[test]
    fn unknown_body_is_unsupported() {
        // Simulate a future arm: encode an AppMessage with no body (what an
        // unknown oneof arm decodes to under proto3).
        let app = pb::AppMessage {
            version: 1,
            id: vec![3; 16],
            ..Default::default()
        };
        let bytes = to_chat(app).encode_to_vec();
        assert!(matches!(open(&bytes), Err(AppError::Unsupported(_))));
    }

    #[test]
    fn bad_id_rejected() {
        let mut app = wrap(receipt()).unwrap();
        app.id = vec![1, 2, 3];
        let bytes = encode_for_mls(app).unwrap();
        assert!(matches!(open(&bytes), Err(AppError::Invalid(_))));
    }
}

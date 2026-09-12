//! Frame and field bounds.
//!
//! Every bound here is checked before the corresponding bytes are allocated
//! or decoded. A node that decodes first and checks afterwards has already
//! spent the memory an attacker wanted it to spend.

/// Largest request or response frame on `/hashgram/rpc/1`, in bytes.
///
/// Sized to carry one blob chunk ([`CHUNK_SIZE`]) plus its envelope with
/// room to spare, and nothing larger: there is no legitimate single message
/// bigger than a chunk.
pub const MAX_RPC_FRAME: usize = CHUNK_SIZE + 64 * 1024;

/// Largest gossip message, in bytes. Social events carry text and
/// references, never media, so this is generous.
pub const MAX_GOSSIP_FRAME: usize = 64 * 1024;

/// Blob chunk size: 1 MiB. Every chunk but the last is exactly this long.
pub const CHUNK_SIZE: usize = 1024 * 1024;

/// Largest blob a single manifest may describe: 4 GiB.
pub const MAX_BLOB_SIZE: u64 = 4 * 1024 * 1024 * 1024;

/// Largest number of chunks in a manifest, derived from the two above.
#[allow(clippy::integer_division)] // exact: MAX_BLOB_SIZE is a multiple of CHUNK_SIZE
pub const MAX_CHUNKS: usize = (MAX_BLOB_SIZE / CHUNK_SIZE as u64) as usize;

/// Largest envelope ciphertext, in bytes. Messages, not media: attachments
/// travel as blobs and the envelope carries the reference and key.
pub const MAX_ENVELOPE_CIPHERTEXT: usize = 256 * 1024;

/// Longest a store node will hold an envelope, in seconds: 30 days.
pub const MAX_ENVELOPE_RETENTION_SECS: u64 = 30 * 24 * 3600;

/// Largest serialised MLS key package, in bytes.
pub const MAX_KEY_PACKAGE: usize = 8 * 1024;

/// Largest social event payload, in bytes.
pub const MAX_EVENT_PAYLOAD: usize = 32 * 1024;

/// Most media references one event may carry.
pub const MAX_EVENT_MEDIA: usize = 20;

/// Longest post or caption text, in bytes of UTF-8.
pub const MAX_TEXT_BYTES: usize = 10_000;

/// Longest single hashtag, mention or reaction, in bytes.
pub const MAX_TAG_BYTES: usize = 64;

/// Most hashtags or mentions per event.
pub const MAX_TAGS: usize = 30;

/// Longest a story may live, in seconds: 48 hours.
pub const MAX_STORY_SECS: u64 = 48 * 3600;

/// How far in the future a timestamp may be before the object is refused,
/// in seconds. Clock skew is real; an hour is not.
pub const MAX_FUTURE_SKEW_SECS: u64 = 300;

/// How old a signed request (mailbox fetch, ack, upload) may be before it is
/// refused as a replay, in seconds.
pub const MAX_REQUEST_AGE_SECS: u64 = 300;

/// Most addresses in a node announcement or bootstrap record.
pub const MAX_ANNOUNCE_ADDRS: usize = 16;

/// Longest a node announcement may claim to be valid, in seconds.
pub const MAX_ANNOUNCE_TTL_SECS: u64 = 24 * 3600;

/// Longest a multiaddr string may be.
pub const MAX_MULTIADDR_BYTES: usize = 256;

/// Most envelopes returned per mailbox fetch page.
pub const MAX_MAILBOX_PAGE: u32 = 100;

/// The `limit` a TURN credential request carries in the `MailboxFetch`
/// preimage it signs under the `mailbox-fetch` purpose.
///
/// Reusing the fetch purpose proves "I hold this device key right now"
/// without a fourteenth signing domain (a cross-language change). The price
/// is that the two preimages must never coincide: a real fetch with an empty
/// cursor and the default limit was byte-identical to a credential request,
/// so a store node could replay a fetch it had legitimately received to
/// obtain TURN credentials labelled with the victim's device. `u32::MAX` is
/// never a meaningful page size, so [`crate::validate::mailbox_fetch`]
/// refuses it outright and the TURN path requires it, which makes the two
/// preimage sets disjoint.
pub const TURN_SENTINEL_LIMIT: u32 = u32::MAX;

/// Most events returned per fetch.
pub const MAX_EVENT_PAGE: u32 = 200;

/// Most ids per attestation or event query.
pub const MAX_QUERY_IDS: usize = 100;

/// Length of a BLAKE3 hash, an ed25519 public key and an envelope id.
pub const HASH_LEN: usize = 32;

/// Length of an ed25519 signature.
pub const SIG_LEN: usize = 64;

/// Version every wire object carries today.
pub const WIRE_VERSION: u32 = 1;

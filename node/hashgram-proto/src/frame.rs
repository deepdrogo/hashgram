//! Length-prefixed framing for protobuf messages.
//!
//! The prefix is a big-endian `u32`. It is checked against a caller-supplied
//! bound before the body is read, so the bound is enforced at the cheapest
//! possible point: four bytes in.

use prost::Message;

/// Why a frame was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// The declared length exceeds the bound for this protocol.
    #[error("frame of {len} bytes exceeds the {max} byte bound")]
    TooLarge {
        /// Declared length.
        len: usize,
        /// The bound.
        max: usize,
    },
    /// Fewer bytes than the prefix promised.
    #[error("frame truncated: expected {expected} bytes, had {actual}")]
    Truncated {
        /// Declared length.
        expected: usize,
        /// Bytes available.
        actual: usize,
    },
    /// The body did not decode as the expected message.
    #[error("frame body did not decode: {0}")]
    Decode(String),
    /// Bytes remained after the frame.
    #[error("{0} trailing bytes after the frame")]
    Trailing(usize),
}

/// Encodes a message as `u32be(len) || body`.
///
/// Encoding cannot exceed the bound in practice for any message built by
/// this crate's constructors, but the check is here so that a caller who
/// bypasses them cannot produce a frame the peer will refuse.
pub fn encode_frame<M: Message>(msg: &M, max: usize) -> Result<Vec<u8>, FrameError> {
    let len = msg.encoded_len();
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    let mut out = Vec::with_capacity(4 + len);
    out.extend_from_slice(&(len as u32).to_be_bytes());
    msg.encode(&mut out)
        .map_err(|e| FrameError::Decode(e.to_string()))?;
    Ok(out)
}

/// Reads the length prefix and checks it against the bound.
///
/// Separated from [`decode_frame`] so a streaming reader can check the
/// prefix before reading the body from the socket.
pub fn read_prefix(prefix: [u8; 4], max: usize) -> Result<usize, FrameError> {
    let len = u32::from_be_bytes(prefix) as usize;
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    Ok(len)
}

/// Decodes exactly one frame from a buffer.
pub fn decode_frame<M: Message + Default>(buf: &[u8], max: usize) -> Result<M, FrameError> {
    let Some((prefix, rest)) = buf.split_first_chunk::<4>() else {
        return Err(FrameError::Truncated {
            expected: 4,
            actual: buf.len(),
        });
    };
    let len = read_prefix(*prefix, max)?;
    if rest.len() < len {
        return Err(FrameError::Truncated {
            expected: len,
            actual: rest.len(),
        });
    }
    let (body, trailing) = rest.split_at(len);
    if !trailing.is_empty() {
        return Err(FrameError::Trailing(trailing.len()));
    }
    M::decode(body).map_err(|e| FrameError::Decode(e.to_string()))
}

/// Decodes a bare message body with a bound, for gossip payloads that
/// carry no prefix because the transport already frames them.
pub fn decode_bounded<M: Message + Default>(body: &[u8], max: usize) -> Result<M, FrameError> {
    if body.len() > max {
        return Err(FrameError::TooLarge {
            len: body.len(),
            max,
        });
    }
    M::decode(body).map_err(|e| FrameError::Decode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pb;

    #[test]
    fn round_trips() {
        let hs = pb::Handshake {
            network_id: "hashgram-devnet".into(),
            ..Default::default()
        };
        let bytes = encode_frame(&hs, 1024).unwrap();
        let back: pb::Handshake = decode_frame(&bytes, 1024).unwrap();
        assert_eq!(back, hs);
    }

    #[test]
    fn an_oversized_prefix_is_refused_before_the_body() {
        let mut buf = (u32::MAX).to_be_bytes().to_vec();
        buf.push(0);
        let err = decode_frame::<pb::Handshake>(&buf, 1024).unwrap_err();
        assert!(matches!(err, FrameError::TooLarge { .. }));
    }

    #[test]
    fn a_truncated_body_is_refused() {
        let buf = [0, 0, 0, 10, 1, 2];
        let err = decode_frame::<pb::Handshake>(&buf, 1024).unwrap_err();
        assert!(matches!(err, FrameError::Truncated { .. }));
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let hs = pb::Handshake::default();
        let mut bytes = encode_frame(&hs, 1024).unwrap();
        bytes.push(0xff);
        let err = decode_frame::<pb::Handshake>(&bytes, 1024).unwrap_err();
        assert!(matches!(err, FrameError::Trailing(1)));
    }

    #[test]
    fn garbage_does_not_panic() {
        for len in 0..64usize {
            let buf = vec![0xffu8; len];
            let _ = decode_frame::<pb::Request>(&buf, 1024);
            let _ = decode_bounded::<pb::Gossip>(&buf, 1024);
        }
    }
}

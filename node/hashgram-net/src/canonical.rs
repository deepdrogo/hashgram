//! Canonical signing preimages.
//!
//! A port of Go's `app/canonical`, byte for byte. Protobuf is not a canonical
//! encoding — field order, varint padding and unknown fields all admit
//! several encodings of the same message — so every Hashgram signing preimage
//! is built by hand from the fields in a fixed order, and this module is the
//! one place in Rust that does it.
//!
//! # Framing
//!
//! Every variable-length field carries a big-endian `u32` length prefix.
//! Without prefixes, `(a="ab", b="c")` and `(a="a", b="bc")` produce the same
//! bytes and a signature for one verifies for the other.
//!
//! # Bounds
//!
//! A field longer than [`MAX_FIELD_BYTES`] cannot be framed unambiguously
//! with a 32-bit prefix, so the builder records an error and [`Buf::finish`]
//! returns it. Callers fail closed: a preimage that cannot be built is a
//! signature that cannot be verified.

/// The largest a single length-prefixed field may be: 16 MiB.
pub const MAX_FIELD_BYTES: usize = 16 << 20;

/// Returned when a field cannot be framed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    /// A field exceeded the 32-bit framing bound.
    #[error("canonical: field {field:?} is {len} bytes, above the {MAX_FIELD_BYTES} byte limit; it cannot be length-prefixed unambiguously")]
    FieldTooLong {
        /// Which field.
        field: &'static str,
        /// Its length.
        len: usize,
    },
}

/// Accumulates a bounded-field signing preimage. Mirrors Go's `canonical.Buf`.
#[derive(Debug, Default, Clone)]
pub struct Buf {
    out: Vec<u8>,
    err: Option<CanonicalError>,
}

impl Buf {
    /// A builder with capacity reserved for `hint` bytes.
    #[must_use]
    pub fn new(hint: usize) -> Self {
        Self {
            out: Vec::with_capacity(hint),
            err: None,
        }
    }

    /// Appends `u32be(len(v)) || v`.
    pub fn bytes(mut self, field: &'static str, v: &[u8]) -> Self {
        if self.err.is_some() {
            return self;
        }
        if v.len() > MAX_FIELD_BYTES {
            self.err = Some(CanonicalError::FieldTooLong {
                field,
                len: v.len(),
            });
            return self;
        }
        // Exact: bounded below 2^32 by the check above.
        self.out.extend_from_slice(&(v.len() as u32).to_be_bytes());
        self.out.extend_from_slice(v);
        self
    }

    /// Appends `u32be(len(s)) || s`.
    pub fn string(self, field: &'static str, s: &str) -> Self {
        self.bytes(field, s.as_bytes())
    }

    /// Appends a big-endian `u32`.
    pub fn u32(mut self, v: u32) -> Self {
        if self.err.is_none() {
            self.out.extend_from_slice(&v.to_be_bytes());
        }
        self
    }

    /// Appends a big-endian `u64`.
    pub fn u64(mut self, v: u64) -> Self {
        if self.err.is_none() {
            self.out.extend_from_slice(&v.to_be_bytes());
        }
        self
    }

    /// Appends a protobuf enum value as a big-endian `u32`, matching Go's
    /// two's-complement reinterpretation.
    pub fn enum_value(self, v: i32) -> Self {
        self.u32(v as u32)
    }

    /// Appends a block height as a big-endian `u64`, matching Go's `Height`.
    pub fn height(self, v: i64) -> Self {
        self.u64(v as u64)
    }

    /// Appends a count as a big-endian `u32`, refusing one above the bound.
    pub fn len(mut self, field: &'static str, v: usize) -> Self {
        if self.err.is_some() {
            return self;
        }
        if v > MAX_FIELD_BYTES {
            self.err = Some(CanonicalError::FieldTooLong { field, len: v });
            return self;
        }
        self.out.extend_from_slice(&(v as u32).to_be_bytes());
        self
    }

    /// Appends a length-prefixed list of length-prefixed byte strings: the
    /// count, then each element. Both are in the preimage so a list cannot be
    /// re-framed into a different list with the same signature.
    pub fn bytes_list<I, T>(mut self, field: &'static str, items: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: AsRef<[u8]>,
    {
        let items: Vec<T> = items.into_iter().collect();
        self = self.len(field, items.len());
        for item in &items {
            self = self.bytes(field, item.as_ref());
        }
        self
    }

    /// Appends a length-prefixed list of strings.
    pub fn string_list<'a, I>(self, field: &'static str, items: I) -> Self
    where
        I: IntoIterator<Item = &'a str>,
    {
        self.bytes_list(field, items.into_iter().map(str::as_bytes))
    }

    /// The preimage, or the first error.
    pub fn finish(self) -> Result<Vec<u8>, CanonicalError> {
        match self.err {
            Some(err) => Err(err),
            None => Ok(self.out),
        }
    }
}

/// Builds a preimage using only operations that cannot fail. Mirrors Go's
/// `canonical.Fixed`: 64-bit length prefixes for arbitrary-length payloads.
#[derive(Debug, Default, Clone)]
pub struct Fixed {
    out: Vec<u8>,
}

impl Fixed {
    /// A builder with capacity reserved for `hint` bytes.
    #[must_use]
    pub fn new(hint: usize) -> Self {
        Self {
            out: Vec::with_capacity(hint),
        }
    }

    /// Appends bytes with no length prefix. Only correct for a fixed-width
    /// field whose width is part of the format.
    #[must_use]
    pub fn raw(mut self, v: &[u8]) -> Self {
        self.out.extend_from_slice(v);
        self
    }

    /// Appends `u64be(len(v)) || v`.
    #[must_use]
    pub fn bytes(mut self, v: &[u8]) -> Self {
        self.out.extend_from_slice(&(v.len() as u64).to_be_bytes());
        self.out.extend_from_slice(v);
        self
    }

    /// Appends `u64be(len(s)) || s`.
    #[must_use]
    pub fn string(self, s: &str) -> Self {
        self.bytes(s.as_bytes())
    }

    /// Appends a count as a big-endian `u64`.
    #[must_use]
    pub fn count(self, v: usize) -> Self {
        self.u64(v as u64)
    }

    /// Appends a big-endian `u64`.
    #[must_use]
    pub fn u64(mut self, v: u64) -> Self {
        self.out.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Appends a big-endian `u32`.
    #[must_use]
    pub fn u32(mut self, v: u32) -> Self {
        self.out.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Appends a count-prefixed list of length-prefixed byte strings.
    #[must_use]
    pub fn bytes64_slice<I, T>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: AsRef<[u8]>,
    {
        let items: Vec<T> = items.into_iter().collect();
        self = self.count(items.len());
        for item in &items {
            self = self.bytes(item.as_ref());
        }
        self
    }

    /// The accumulated bytes.
    #[must_use]
    pub fn preimage(self) -> Vec<u8> {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buf_frames_with_u32_prefixes() {
        let pre = Buf::new(0)
            .string("a", "ab")
            .string("b", "c")
            .finish()
            .expect("small fields frame");
        assert_eq!(pre, [0, 0, 0, 2, b'a', b'b', 0, 0, 0, 1, b'c']);
    }

    #[test]
    fn buf_prevents_field_confusion() {
        let a = Buf::new(0).string("a", "ab").string("b", "c").finish();
        let b = Buf::new(0).string("a", "a").string("b", "bc").finish();
        assert_ne!(a, b);
    }

    #[test]
    fn buf_refuses_an_oversized_field() {
        let big = vec![0u8; MAX_FIELD_BYTES + 1];
        let err = Buf::new(0)
            .bytes("big", &big)
            .u32(1)
            .finish()
            .expect_err("oversized field framed");
        assert!(matches!(
            err,
            CanonicalError::FieldTooLong { field: "big", .. }
        ));
    }

    #[test]
    fn buf_lists_commit_to_count_and_lengths() {
        let one = Buf::new(0)
            .bytes_list("l", [b"ab".as_slice(), b"c"])
            .finish();
        let two = Buf::new(0)
            .bytes_list("l", [b"a".as_slice(), b"bc"])
            .finish();
        let three = Buf::new(0).bytes_list("l", [b"abc".as_slice()]).finish();
        assert_ne!(one, two);
        assert_ne!(one, three);
    }

    #[test]
    fn enum_and_height_reinterpret_like_go() {
        let pre = Buf::new(0)
            .enum_value(-1)
            .height(-1)
            .finish()
            .expect("frames");
        assert_eq!(
            pre,
            [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );
    }

    #[test]
    fn fixed_uses_u64_prefixes() {
        let pre = Fixed::new(0).raw(b"HGM1").string("x").preimage();
        assert_eq!(pre, [b'H', b'G', b'M', b'1', 0, 0, 0, 0, 0, 0, 0, 1, b'x']);
    }

    #[test]
    fn fixed_slice_commits_to_count() {
        let a = Fixed::new(0)
            .bytes64_slice([b"a".as_slice(), b"b"])
            .preimage();
        let b = Fixed::new(0).bytes64_slice([b"ab".as_slice()]).preimage();
        assert_ne!(a, b);
    }
}

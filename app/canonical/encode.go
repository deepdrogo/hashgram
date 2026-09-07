// Package canonical builds the byte strings Hashgram signs.
//
// Protobuf is not a canonical encoding. Field ordering, varint padding and
// unknown fields all admit several encodings of the same logical message, so a
// signature over "whatever proto produced" is a signature over something the
// verifier may not reproduce. Every Hashgram signing preimage is therefore
// built by hand, and this package is the one place that does it.
//
// Four modules previously each had their own copy of the same framing logic:
// x/welcome for eligibility attestations, x/serviceproof for service
// receipts, and x/identity for device certificates and root key rotations.
// Four copies of a signing preimage encoder is four places for the framing to
// drift, and framing drift in signing code is signature confusion, which is
// the class of bug where a signature over one object verifies against a
// different one.
//
// # Framing
//
// Every variable-length field carries a big-endian uint32 length prefix.
// Without prefixes, (method="ab", extra="c") and (method="a", extra="bc")
// produce identical bytes, and a signature for one verifies for the other.
//
// # Bounds
//
// The length prefix is 32 bits, so a field longer than MaxFieldBytes cannot be
// framed unambiguously. Rather than let the conversion wrap silently, which is
// exactly how the ambiguity above gets reintroduced, the encoder records an
// error and Finish returns it. Callers fail closed: a preimage that cannot be
// built is a signature that cannot be verified, which is the correct outcome.
//
// In practice no legitimate field comes close: addresses are under a hundred
// bytes, public keys are 32 or 33, and every module's ValidateBasic bounds its
// own string fields. The check is here so that the property is enforced rather
// than assumed, and so that a future field added without a length bound
// surfaces as a verification failure in a test rather than as a forgery in
// production.
package canonical

import (
	"encoding/binary"
	"fmt"
)

// MaxFieldBytes is the largest a single length-prefixed field may be.
//
// 16 MiB is far above any legitimate Hashgram field and far below the 4 GiB
// the uint32 prefix could express. The gap is deliberate: a field approaching
// this size is a bug worth failing on, not a case worth supporting.
const MaxFieldBytes = 16 << 20

// Buf accumulates a signing preimage.
//
// Errors are recorded rather than returned per call, so that a preimage reads
// as a sequence of fields rather than as a sequence of error checks. Finish
// reports the first error.
type Buf struct {
	out []byte
	err error
}

// New returns a Buf with space reserved for hint bytes.
func New(hint int) *Buf {
	if hint < 0 {
		hint = 0
	}
	return &Buf{out: make([]byte, 0, hint)}
}

// Bytes appends uint32(len(b)) || b.
func (b *Buf) Bytes(field string, v []byte) *Buf {
	if b.err != nil {
		return b
	}
	if len(v) > MaxFieldBytes {
		b.err = fmt.Errorf(
			"canonical: field %q is %d bytes, above the %d byte limit; it cannot be "+
				"length-prefixed unambiguously",
			field, len(v), MaxFieldBytes)
		return b
	}
	// #nosec G115 -- len is non-negative and the MaxFieldBytes check above
	// bounds it well below 2^32, so the conversion is exact. This is the one
	// place in the tree that performs it, which is why the check is here.
	b.out = binary.BigEndian.AppendUint32(b.out, uint32(len(v)))
	b.out = append(b.out, v...)
	return b
}

// String appends uint32(len(s)) || s.
func (b *Buf) String(field, v string) *Buf {
	return b.Bytes(field, []byte(v))
}

// Uint32 appends a big-endian uint32.
func (b *Buf) Uint32(v uint32) *Buf {
	if b.err != nil {
		return b
	}
	b.out = binary.BigEndian.AppendUint32(b.out, v)
	return b
}

// Uint64 appends a big-endian uint64.
func (b *Buf) Uint64(v uint64) *Buf {
	if b.err != nil {
		return b
	}
	b.out = binary.BigEndian.AppendUint64(b.out, v)
	return b
}

// Enum appends a protobuf enum value as a big-endian uint32.
//
// Generated enums are int32. The two's-complement reinterpretation is exact
// and round-trips, and enum values are small non-negative constants in every
// Hashgram proto, so the encoding is stable. Having one method for this makes
// the intent explicit at every call site instead of leaving a bare cast for a
// reader to evaluate.
func (b *Buf) Enum(v int32) *Buf {
	// #nosec G115 -- exact two's-complement reinterpretation. Every int32
	// maps to exactly one uint32 and back, so the preimage stays unambiguous.
	return b.Uint32(uint32(v))
}

// Height appends a block height as a big-endian uint64.
//
// Heights are int64 in the SDK. Negative heights are rejected by every
// caller's ValidateBasic, and the two's-complement reinterpretation is exact
// either way, so the preimage is unambiguous: one int64 maps to exactly one
// uint64 and back.
func (b *Buf) Height(v int64) *Buf {
	// #nosec G115 -- exact two's-complement reinterpretation, as for Enum.
	return b.Uint64(uint64(v))
}

// Len appends a non-negative count as a big-endian uint32.
//
// Used for counts derived from len() or from a bounded parameter. Rejects a
// negative value rather than wrapping it, because a negative count in a
// signing preimage means the caller's own accounting is wrong and continuing
// would sign a value nobody can reproduce.
func (b *Buf) Len(field string, v int) *Buf {
	if b.err != nil {
		return b
	}
	if v < 0 {
		b.err = fmt.Errorf("canonical: count %q is negative (%d)", field, v)
		return b
	}
	if v > MaxFieldBytes {
		b.err = fmt.Errorf(
			"canonical: count %q is %d, above the %d limit", field, v, MaxFieldBytes)
		return b
	}
	// #nosec G115 -- bounded non-negative by the two checks above.
	return b.Uint32(uint32(v))
}

// Finish returns the preimage, or the first error encountered.
func (b *Buf) Finish() ([]byte, error) {
	if b.err != nil {
		return nil, b.err
	}
	return b.out, nil
}

// Err reports the first error encountered, without consuming the buffer.
func (b *Buf) Err() error { return b.err }

// ---------------------------------------------------------------------------
// Fixed
// ---------------------------------------------------------------------------

// Fixed builds a preimage using only operations that cannot fail.
//
// It exists because the outer layers of Hashgram's signing scheme frame
// arbitrary-length payloads rather than bounded fields. The domain-separation
// wrapper in app/params wraps a whole inner preimage, and a Merkle challenge
// response wraps a proof path; neither has a natural small bound, so a 32-bit
// length prefix would be the wrong shape and a bound check on it would be an
// error that can never be handled meaningfully.
//
// Fixed uses 64-bit length prefixes instead. A Go len() is a non-negative
// int, and every non-negative int converts to uint64 exactly on both 32-bit
// and 64-bit platforms, so no length can overflow and there is no error to
// return. Bytes therefore has no error result, and that is a guarantee of the
// type rather than a claim in a comment.
//
// Use Buf for bounded fields, where a 32-bit prefix is compact and the bound
// check catches a genuine bug. Use Fixed for arbitrary-length payloads.
type Fixed struct {
	out []byte
}

// NewFixed returns a Fixed with space reserved for hint bytes.
func NewFixed(hint int) *Fixed {
	if hint < 0 {
		hint = 0
	}
	return &Fixed{out: make([]byte, 0, hint)}
}

// Raw appends bytes with no length prefix.
//
// Only correct for a fixed-width field whose width is part of the format, such
// as the four-byte network magic. Using it for a variable-length field
// reintroduces the field confusion the prefixes exist to prevent.
func (f *Fixed) Raw(v []byte) *Fixed {
	f.out = append(f.out, v...)
	return f
}

// Bytes appends uint64(len(v)) || v.
func (f *Fixed) Bytes(v []byte) *Fixed {
	f.out = binary.BigEndian.AppendUint64(f.out, uint64(len(v)))
	f.out = append(f.out, v...)
	return f
}

// String appends uint64(len(s)) || s.
func (f *Fixed) String(v string) *Fixed { return f.Bytes([]byte(v)) }

// Count appends a non-negative count as a big-endian uint64.
func (f *Fixed) Count(v int) *Fixed {
	if v < 0 {
		// Unreachable from a len(), which is the only source used. Clamping
		// rather than wrapping keeps the encoding total: a negative count
		// would otherwise become an enormous positive one.
		v = 0
	}
	return f.Uint64(uint64(v))
}

// Uint64 appends a big-endian uint64.
func (f *Fixed) Uint64(v uint64) *Fixed {
	f.out = binary.BigEndian.AppendUint64(f.out, v)
	return f
}

// Uint32 appends a big-endian uint32.
func (f *Fixed) Uint32(v uint32) *Fixed {
	f.out = binary.BigEndian.AppendUint32(f.out, v)
	return f
}

// Bytes64Slice appends a length-prefixed sequence of length-prefixed slices.
//
// Used for a Merkle proof path: the number of elements and each element's
// length are both in the preimage, so a proof cannot be re-framed into a
// different one with the same signature.
func (f *Fixed) Bytes64Slice(vs [][]byte) *Fixed {
	f.Count(len(vs))
	for _, v := range vs {
		f.Bytes(v)
	}
	return f
}

// Preimage returns the accumulated bytes. There is no error result because no
// operation on Fixed can fail.
func (f *Fixed) Preimage() []byte { return f.out }

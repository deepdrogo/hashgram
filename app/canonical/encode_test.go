package canonical_test

import (
	"bytes"
	"strings"
	"testing"

	"github.com/hashgram/hashgram/app/canonical"
)

// The property the whole package exists for: two different field splits must
// not produce the same preimage. Without length prefixes,
// ("ab", "c") and ("a", "bc") encode identically and a signature over one
// verifies against the other.
func TestLengthPrefixesPreventFieldConfusion(t *testing.T) {
	first, err := canonical.New(0).
		String("a", "ab").
		String("b", "c").
		Finish()
	if err != nil {
		t.Fatalf("encoding: %v", err)
	}

	second, err := canonical.New(0).
		String("a", "a").
		String("b", "bc").
		Finish()
	if err != nil {
		t.Fatalf("encoding: %v", err)
	}

	if bytes.Equal(first, second) {
		t.Fatal("(\"ab\",\"c\") and (\"a\",\"bc\") produced the same preimage; " +
			"a signature over one would verify against the other")
	}
}

// The same inputs must always produce the same bytes, or verification depends
// on which node did the encoding.
func TestEncodingIsDeterministic(t *testing.T) {
	build := func() []byte {
		out, err := canonical.New(64).
			String("addr", "hash1abcdef").
			Bytes("pubkey", []byte{1, 2, 3, 4}).
			Uint32(7).
			Uint64(1<<40).
			Enum(3).
			Height(123456).
			Len("count", 9).
			Finish()
		if err != nil {
			t.Fatalf("encoding: %v", err)
		}
		return out
	}

	if !bytes.Equal(build(), build()) {
		t.Fatal("two encodings of the same input differ")
	}
}

// Byte layout, asserted explicitly. A change to the framing is a consensus
// break and a signature break for every previously issued object, so it
// should require editing this test rather than passing silently.
func TestByteLayoutIsExact(t *testing.T) {
	out, err := canonical.New(0).
		String("s", "hi").
		Uint32(0x01020304).
		Uint64(0x0102030405060708).
		Finish()
	if err != nil {
		t.Fatalf("encoding: %v", err)
	}

	want := []byte{
		0, 0, 0, 2, // uint32 length prefix, big-endian
		'h', 'i',
		1, 2, 3, 4, // uint32, big-endian
		1, 2, 3, 4, 5, 6, 7, 8, // uint64, big-endian
	}
	if !bytes.Equal(out, want) {
		t.Fatalf("layout changed:\n got %v\nwant %v", out, want)
	}
}

// An empty field is distinct from an absent one, because the prefix is still
// written. This matters: it is what stops an optional field being dropped to
// forge a shorter object with the same signature.
func TestEmptyFieldStillCarriesItsPrefix(t *testing.T) {
	withEmpty, err := canonical.New(0).String("a", "").String("b", "x").Finish()
	if err != nil {
		t.Fatalf("encoding: %v", err)
	}
	withoutField, err := canonical.New(0).String("b", "x").Finish()
	if err != nil {
		t.Fatalf("encoding: %v", err)
	}

	if bytes.Equal(withEmpty, withoutField) {
		t.Fatal("an empty field encoded identically to an omitted one")
	}
	if len(withEmpty) != len(withoutField)+4 {
		t.Fatalf("empty field cost %d bytes, expected 4 (the prefix alone)",
			len(withEmpty)-len(withoutField))
	}
}

// The bound must be an error rather than a wrapped length. A wrapped uint32
// prefix reintroduces exactly the field confusion the prefixes prevent.
func TestOversizedFieldIsRefused(t *testing.T) {
	huge := strings.Repeat("x", canonical.MaxFieldBytes+1)

	out, err := canonical.New(0).String("huge", huge).Finish()
	if err == nil {
		t.Fatal("an oversized field was accepted; the length prefix would wrap")
	}
	if out != nil {
		t.Fatal("bytes were returned alongside an error; callers must fail closed")
	}
	if !strings.Contains(err.Error(), "huge") {
		t.Errorf("the error does not name the offending field: %v", err)
	}
}

// A field exactly at the limit is legal. An off-by-one here would reject
// valid objects, which is a liveness bug rather than a safety one but is
// still a bug.
func TestFieldExactlyAtTheLimitIsAccepted(t *testing.T) {
	atLimit := bytes.Repeat([]byte("y"), canonical.MaxFieldBytes)

	out, err := canonical.New(0).Bytes("at_limit", atLimit).Finish()
	if err != nil {
		t.Fatalf("a field of exactly MaxFieldBytes was refused: %v", err)
	}
	if len(out) != canonical.MaxFieldBytes+4 {
		t.Fatalf("got %d bytes, want %d", len(out), canonical.MaxFieldBytes+4)
	}
}

// Once an error is recorded, later fields must not be appended. Continuing
// would build a preimage that silently omits a field.
func TestErrorIsStickyAndSuppressesLaterFields(t *testing.T) {
	b := canonical.New(0).
		String("huge", strings.Repeat("x", canonical.MaxFieldBytes+1)).
		String("after", "this must not be appended").
		Uint64(42)

	if b.Err() == nil {
		t.Fatal("the error did not persist across later calls")
	}
	if _, err := b.Finish(); err == nil {
		t.Fatal("Finish returned no error after a failed field")
	}
}

// A negative count is the caller's own accounting being wrong. Wrapping it
// into a large uint32 would sign a value nobody can reproduce.
func TestNegativeCountIsRefused(t *testing.T) {
	if _, err := canonical.New(0).Len("count", -1).Finish(); err == nil {
		t.Fatal("a negative count was accepted")
	}
}

// Heights round-trip through two's complement exactly, including negatives,
// so one int64 maps to exactly one preimage.
func TestHeightsAreUnambiguous(t *testing.T) {
	seen := make(map[string]int64)

	for _, h := range []int64{0, 1, -1, 1 << 62, -(1 << 62), 1234567890} {
		out, err := canonical.New(8).Height(h).Finish()
		if err != nil {
			t.Fatalf("height %d: %v", h, err)
		}
		if len(out) != 8 {
			t.Fatalf("height %d encoded to %d bytes, want 8", h, len(out))
		}
		key := string(out)
		if prev, dup := seen[key]; dup {
			t.Fatalf("heights %d and %d encoded identically", prev, h)
		}
		seen[key] = h
	}
}

// New must tolerate a nonsensical size hint rather than panicking: the hint
// is an allocation optimisation, not an input contract.
func TestNegativeSizeHintIsHarmless(t *testing.T) {
	out, err := canonical.New(-100).Uint32(1).Finish()
	if err != nil {
		t.Fatalf("negative hint caused an error: %v", err)
	}
	if len(out) != 4 {
		t.Fatalf("got %d bytes, want 4", len(out))
	}
}

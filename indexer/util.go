package indexer

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"math"

	"lukechampine.com/blake3"
)

func base64Decode(s string) ([]byte, error) {
	return base64.StdEncoding.DecodeString(s)
}

func sha256Hex(b []byte) string {
	h := sha256.Sum256(b)
	return hex.EncodeToString(h[:])
}

func blake3Sum(b []byte) []byte {
	h := blake3.Sum256(b)
	return h[:]
}

// i64 converts a network-supplied uint64 (timestamps, sequences, expiries)
// to the int64 PostgreSQL stores, saturating instead of wrapping. A peer that
// sends 2^64-1 as a timestamp must not turn into a negative time in the index.
func i64(u uint64) int64 {
	if u > math.MaxInt64 {
		return math.MaxInt64
	}
	return int64(u)
}

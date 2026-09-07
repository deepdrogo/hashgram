package indexer

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"

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

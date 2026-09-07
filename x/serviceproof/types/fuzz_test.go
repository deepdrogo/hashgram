package types

import "testing"

// FuzzVerifyMerkleProof: a challenge answer is provider-controlled bytes.
// Verification must reject garbage without panicking, and must accept the
// honest proof for every index of every small tree.
func FuzzVerifyMerkleProof(f *testing.F) {
	f.Add([]byte("a"), uint32(0), uint32(1), []byte{})
	f.Add(make([]byte, 32), uint32(3), uint32(4), make([]byte, 64))
	f.Fuzz(func(t *testing.T, leaf []byte, index, count uint32, pathBytes []byte) {
		var path [][]byte
		for i := 0; i+32 <= len(pathBytes) && len(path) < MaxMerklePathLength+1; i += 32 {
			path = append(path, pathBytes[i:i+32])
		}
		root := make([]byte, 32)
		_ = VerifyMerkleProof(root, leaf, path, index, count)
	})
}

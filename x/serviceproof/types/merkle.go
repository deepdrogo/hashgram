package types

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
)

// Storage proofs use a binary Merkle tree over fixed-size chunk hashes.
//
// Domain separation on both leaves and internal nodes is not decoration. A
// tree that hashes leaves and internal nodes identically admits second
// preimage attacks: an attacker can present an internal node as if it were a
// leaf, or reinterpret a subtree root as a chunk hash, and produce a valid
// proof for data they do not hold. The prefixes below make leaf hashes and
// node hashes disjoint.
//
// The scheme is deliberately conventional. Hashgram invents no cryptography;
// this is RFC 6962-style prefixed hashing with SHA-256.
var (
	leafPrefix = []byte{0x00}
	nodePrefix = []byte{0x01}
)

// ChunkHashSize is the size of a chunk hash in bytes.
const ChunkHashSize = sha256.Size

// MaxMerklePathLength bounds a proof.
//
// A 64-element path corresponds to a tree with 2^64 leaves, which no real
// blob will approach. The bound exists so that a malicious proof cannot force
// unbounded hashing work inside a transaction.
const MaxMerklePathLength = 64

// LeafHash returns the domain-separated hash of a chunk's contents.
func LeafHash(chunk []byte) [32]byte {
	h := sha256.New()
	h.Write(leafPrefix)
	h.Write(chunk)
	var out [32]byte
	copy(out[:], h.Sum(nil))
	return out
}

// NodeHash returns the domain-separated hash of two child hashes.
func NodeHash(left, right []byte) [32]byte {
	h := sha256.New()
	h.Write(nodePrefix)
	h.Write(left)
	h.Write(right)
	var out [32]byte
	copy(out[:], h.Sum(nil))
	return out
}

// MerkleRoot builds the root over a list of chunk contents.
//
// Odd levels duplicate the last node, the common convention. Note that this
// makes the tree shape a function of the leaf count, so chunk_count is part
// of the assignment and is checked during verification: without it, a proof
// for a differently shaped tree with the same root prefix could be accepted.
func MerkleRoot(chunks [][]byte) [32]byte {
	if len(chunks) == 0 {
		return sha256.Sum256(nil)
	}

	level := make([][32]byte, len(chunks))
	for i, c := range chunks {
		level[i] = LeafHash(c)
	}

	for len(level) > 1 {
		next := make([][32]byte, 0, (len(level)+1)/2)
		for i := 0; i < len(level); i += 2 {
			left := level[i]
			right := left
			if i+1 < len(level) {
				right = level[i+1]
			}
			next = append(next, NodeHash(left[:], right[:]))
		}
		level = next
	}
	return level[0]
}

// MerkleProof returns the sibling path from a leaf to the root, bottom first.
func MerkleProof(chunks [][]byte, index int) [][]byte {
	if index < 0 || index >= len(chunks) {
		return nil
	}

	level := make([][32]byte, len(chunks))
	for i, c := range chunks {
		level[i] = LeafHash(c)
	}

	var path [][]byte
	idx := index

	for len(level) > 1 {
		sibling := idx ^ 1
		if sibling >= len(level) {
			// Duplicated last node: the sibling is the node itself.
			sibling = idx
		}
		s := level[sibling]
		path = append(path, append([]byte(nil), s[:]...))

		next := make([][32]byte, 0, (len(level)+1)/2)
		for i := 0; i < len(level); i += 2 {
			left := level[i]
			right := left
			if i+1 < len(level) {
				right = level[i+1]
			}
			next = append(next, NodeHash(left[:], right[:]))
		}
		level = next
		idx /= 2
	}
	return path
}

// VerifyMerkleProof checks a leaf hash and sibling path against a root.
//
// leafHash is the already-hashed chunk (LeafHash output), because the chain
// never sees chunk contents: a provider proving it holds a 4 MiB chunk must
// not have to put 4 MiB in a transaction. The signature on the challenge
// response is what ties this hash to the node that was asked.
func VerifyMerkleProof(root, leafHash []byte, path [][]byte, index uint32, leafCount uint32) error {
	if len(root) != ChunkHashSize {
		return ErrInvalidMerkleProof.Wrapf("root is %d bytes, expected %d", len(root), ChunkHashSize)
	}
	if len(leafHash) != ChunkHashSize {
		return ErrInvalidMerkleProof.Wrapf("leaf hash is %d bytes, expected %d", len(leafHash), ChunkHashSize)
	}
	if leafCount == 0 {
		return ErrInvalidMerkleProof.Wrap("leaf count must be positive")
	}
	if index >= leafCount {
		return ErrInvalidMerkleProof.Wrapf("index %d is out of range for %d leaves", index, leafCount)
	}
	if len(path) > MaxMerklePathLength {
		return ErrInvalidMerkleProof.Wrapf(
			"path has %d elements, maximum is %d", len(path), MaxMerklePathLength)
	}

	// The path length is determined by the tree shape, which is determined by
	// the leaf count. Checking it rejects proofs padded with extra levels.
	if want := expectedPathLength(leafCount); len(path) != want {
		return ErrInvalidMerkleProof.Wrapf(
			"path has %d elements but a tree with %d leaves needs %d", len(path), leafCount, want)
	}

	cur := make([]byte, ChunkHashSize)
	copy(cur, leafHash)

	idx := index
	width := leafCount

	for _, sibling := range path {
		if len(sibling) != ChunkHashSize {
			return ErrInvalidMerkleProof.Wrapf(
				"path element is %d bytes, expected %d", len(sibling), ChunkHashSize)
		}
		var next [32]byte
		if idx%2 == 0 {
			next = NodeHash(cur, sibling)
		} else {
			next = NodeHash(sibling, cur)
		}
		cur = next[:]
		idx /= 2
		width = (width + 1) / 2
	}

	if !bytes.Equal(cur, root) {
		return ErrInvalidMerkleProof.Wrap("computed root does not match the assignment root")
	}
	if width != 1 {
		return ErrInvalidMerkleProof.Wrapf("path did not reach the root (width %d)", width)
	}
	return nil
}

func expectedPathLength(leafCount uint32) int {
	n := 0
	width := leafCount
	for width > 1 {
		width = (width + 1) / 2
		n++
	}
	return n
}

// ChallengeChunkIndex derives which chunk a provider must prove, from entropy
// the provider cannot control.
//
// The inputs are the block's app hash, the challenge id, the blob id and the
// replica index. A provider deciding whether to actually store data cannot
// predict which chunk will be asked for, so keeping a subset of the data is
// not a viable strategy: the expected reward falls in proportion to the
// fraction discarded, while the risk of a fraud score does not.
func ChallengeChunkIndex(appHash []byte, challengeID uint64, blobID []byte, replicaIndex uint32, chunkCount uint32) uint32 {
	if chunkCount == 0 {
		return 0
	}

	h := sha256.New()
	h.Write([]byte("hashgram/storage-challenge-index"))
	h.Write(appHash)
	_ = binary.Write(h, binary.BigEndian, challengeID)
	h.Write(blobID)
	_ = binary.Write(h, binary.BigEndian, replicaIndex)

	sum := h.Sum(nil)
	// Modulo bias is negligible here: chunkCount is bounded by uint32 while
	// the source is 64 bits of hash output.
	n := binary.BigEndian.Uint64(sum[:8])
	return uint32(n % uint64(chunkCount))
}

// CanonicalChallengeResponseBytes returns the bytes a provider's node key
// signs when answering a challenge.
//
// Length-prefixed, like every other signed object in Hashgram. Without the
// signature, a Merkle proof observed on chain could be replayed by anyone;
// the signature ties the answer to the node that was actually challenged.
func CanonicalChallengeResponseBytes(challengeID uint64, chunkHash []byte, path [][]byte) []byte {
	out := make([]byte, 0, 8+4+len(chunkHash)+4+len(path)*(4+ChunkHashSize))

	out = binary.BigEndian.AppendUint64(out, challengeID)

	out = binary.BigEndian.AppendUint32(out, uint32(len(chunkHash)))
	out = append(out, chunkHash...)

	out = binary.BigEndian.AppendUint32(out, uint32(len(path)))
	for _, p := range path {
		out = binary.BigEndian.AppendUint32(out, uint32(len(p)))
		out = append(out, p...)
	}
	return out
}

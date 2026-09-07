package types_test

import (
	"bytes"
	"crypto/sha256"
	"fmt"
	"testing"

	"github.com/stretchr/testify/require"

	"github.com/hashgram/hashgram/x/serviceproof/types"
)

func makeChunks(n int) [][]byte {
	out := make([][]byte, n)
	for i := range out {
		out[i] = []byte(fmt.Sprintf("hashgram test chunk %d with some padding to make it longer", i))
	}
	return out
}

// TestMerkleProofRoundTrip verifies a proof for every leaf, across tree
// shapes that include the awkward cases: single leaf, powers of two, and odd
// counts where the last node is duplicated.
func TestMerkleProofRoundTrip(t *testing.T) {
	for _, n := range []int{1, 2, 3, 4, 5, 7, 8, 9, 16, 17, 31, 32, 33, 100, 1000} {
		t.Run(fmt.Sprintf("%d leaves", n), func(t *testing.T) {
			chunks := makeChunks(n)
			root := types.MerkleRoot(chunks)

			for i := 0; i < n; i++ {
				leaf := types.LeafHash(chunks[i])
				path := types.MerkleProof(chunks, i)

				err := types.VerifyMerkleProof(root[:], leaf[:], path, uint32(i), uint32(n))
				require.NoError(t, err, "valid proof for leaf %d of %d rejected", i, n)
			}
		})
	}
}

// TestMerkleProofRejectsWrongChunk is the property that makes a storage
// challenge mean something: a provider that discarded the challenged chunk
// cannot produce a passing proof.
func TestMerkleProofRejectsWrongChunk(t *testing.T) {
	chunks := makeChunks(16)
	root := types.MerkleRoot(chunks)

	// Prove leaf 3 but present leaf 4's hash.
	path := types.MerkleProof(chunks, 3)
	wrong := types.LeafHash(chunks[4])

	err := types.VerifyMerkleProof(root[:], wrong[:], path, 3, 16)
	require.Error(t, err, "a proof for the wrong chunk was accepted")
	require.True(t, types.ErrInvalidMerkleProof.Is(err), "got %v", err)
}

// TestMerkleProofRejectsWrongIndex: a provider cannot answer a challenge for
// chunk 3 with a valid proof for chunk 7.
func TestMerkleProofRejectsWrongIndex(t *testing.T) {
	chunks := makeChunks(16)
	root := types.MerkleRoot(chunks)

	leaf := types.LeafHash(chunks[7])
	path := types.MerkleProof(chunks, 7)

	err := types.VerifyMerkleProof(root[:], leaf[:], path, 3, 16)
	require.Error(t, err, "a proof for a different index was accepted")
}

// TestMerkleProofRejectsTamperedPath
func TestMerkleProofRejectsTamperedPath(t *testing.T) {
	chunks := makeChunks(8)
	root := types.MerkleRoot(chunks)
	leaf := types.LeafHash(chunks[2])

	for i := range types.MerkleProof(chunks, 2) {
		path := types.MerkleProof(chunks, 2)
		path[i] = append([]byte(nil), path[i]...)
		path[i][0] ^= 0xff

		err := types.VerifyMerkleProof(root[:], leaf[:], path, 2, 8)
		require.Error(t, err, "a proof with path element %d corrupted was accepted", i)
	}
}

// TestMerkleProofRejectsWrongRoot: a proof against a different blob's root
// must fail, which is what ties a challenge to a specific assignment.
func TestMerkleProofRejectsWrongRoot(t *testing.T) {
	chunks := makeChunks(8)
	other := makeChunks(9)

	leaf := types.LeafHash(chunks[0])
	path := types.MerkleProof(chunks, 0)
	otherRoot := types.MerkleRoot(other)

	err := types.VerifyMerkleProof(otherRoot[:], leaf[:], path, 0, 8)
	require.Error(t, err, "a proof verified against another blob's root")
}

// TestMerkleProofRejectsPaddedPath: extra levels must not be accepted, or a
// prover could reshape the tree to reach an arbitrary root.
func TestMerkleProofRejectsPaddedPath(t *testing.T) {
	chunks := makeChunks(8)
	root := types.MerkleRoot(chunks)
	leaf := types.LeafHash(chunks[0])

	path := types.MerkleProof(chunks, 0)
	extra := sha256.Sum256([]byte("extra level"))
	padded := append(path, extra[:])

	err := types.VerifyMerkleProof(root[:], leaf[:], padded, 0, 8)
	require.Error(t, err, "a path with an extra level was accepted")
	require.Contains(t, err.Error(), "needs")
}

// TestMerkleProofRejectsTruncatedPath
func TestMerkleProofRejectsTruncatedPath(t *testing.T) {
	chunks := makeChunks(8)
	root := types.MerkleRoot(chunks)
	leaf := types.LeafHash(chunks[0])

	path := types.MerkleProof(chunks, 0)
	require.Len(t, path, 3)

	err := types.VerifyMerkleProof(root[:], leaf[:], path[:2], 0, 8)
	require.Error(t, err, "a truncated path was accepted")
}

// TestMerkleProofRejectsOutOfRangeIndex
func TestMerkleProofRejectsOutOfRangeIndex(t *testing.T) {
	chunks := makeChunks(8)
	root := types.MerkleRoot(chunks)
	leaf := types.LeafHash(chunks[0])
	path := types.MerkleProof(chunks, 0)

	require.Error(t, types.VerifyMerkleProof(root[:], leaf[:], path, 8, 8))
	require.Error(t, types.VerifyMerkleProof(root[:], leaf[:], path, 0, 0))
}

func TestMerkleProofRejectsMalformedInputs(t *testing.T) {
	chunks := makeChunks(4)
	root := types.MerkleRoot(chunks)
	leaf := types.LeafHash(chunks[0])
	path := types.MerkleProof(chunks, 0)

	require.Error(t, types.VerifyMerkleProof(root[:31], leaf[:], path, 0, 4), "short root accepted")
	require.Error(t, types.VerifyMerkleProof(root[:], leaf[:31], path, 0, 4), "short leaf accepted")

	shortPath := [][]byte{path[0][:16], path[1]}
	require.Error(t, types.VerifyMerkleProof(root[:], leaf[:], shortPath, 0, 4), "short path element accepted")

	long := make([][]byte, types.MaxMerklePathLength+1)
	for i := range long {
		long[i] = make([]byte, types.ChunkHashSize)
	}
	require.Error(t, types.VerifyMerkleProof(root[:], leaf[:], long, 0, 4), "oversize path accepted")
}

// TestLeafAndNodeHashesAreDomainSeparated is the second-preimage defence. A
// tree that hashes leaves and internal nodes identically lets an attacker
// present an internal node as a leaf and prove data they do not hold.
func TestLeafAndNodeHashesAreDomainSeparated(t *testing.T) {
	// A 64-byte "chunk" that is exactly two concatenated hashes: with no
	// domain separation, its leaf hash would equal the node hash of those two
	// children.
	left := sha256.Sum256([]byte("left"))
	right := sha256.Sum256([]byte("right"))

	concat := append(append([]byte(nil), left[:]...), right[:]...)

	leaf := types.LeafHash(concat)
	node := types.NodeHash(left[:], right[:])

	require.False(t, bytes.Equal(leaf[:], node[:]),
		"leaf and node hashing are not domain separated; the tree admits second preimages")
}

// TestChallengeChunkIndexIsDeterministic: every validator must derive the
// same index from the same state.
func TestChallengeChunkIndexIsDeterministic(t *testing.T) {
	appHash := []byte("deterministic app hash")
	blobID := []byte("blob-1")

	a := types.ChallengeChunkIndex(appHash, 42, blobID, 0, 1000)
	b := types.ChallengeChunkIndex(appHash, 42, blobID, 0, 1000)
	require.Equal(t, a, b)
	require.Less(t, a, uint32(1000))
}

// TestChallengeChunkIndexVariesWithEveryInput: the index must depend on the
// block entropy, the challenge id, the blob and the replica, so a provider
// cannot predict what will be asked when deciding what to keep.
func TestChallengeChunkIndexVariesWithEveryInput(t *testing.T) {
	base := types.ChallengeChunkIndex([]byte("hash-a"), 1, []byte("blob"), 0, 100_000)

	require.NotEqual(t, base, types.ChallengeChunkIndex([]byte("hash-b"), 1, []byte("blob"), 0, 100_000),
		"the index does not depend on block entropy")
	require.NotEqual(t, base, types.ChallengeChunkIndex([]byte("hash-a"), 2, []byte("blob"), 0, 100_000),
		"the index does not depend on the challenge id")
	require.NotEqual(t, base, types.ChallengeChunkIndex([]byte("hash-a"), 1, []byte("blob2"), 0, 100_000),
		"the index does not depend on the blob id")
	require.NotEqual(t, base, types.ChallengeChunkIndex([]byte("hash-a"), 1, []byte("blob"), 1, 100_000),
		"the index does not depend on the replica index")
}

// TestChallengeChunkIndexIsWellDistributed: a badly distributed index would
// let a provider keep a small subset of chunks and pass most challenges.
func TestChallengeChunkIndexIsWellDistributed(t *testing.T) {
	const chunks = 64
	const samples = 20_000

	buckets := make([]int, chunks)
	for i := 0; i < samples; i++ {
		idx := types.ChallengeChunkIndex([]byte("app hash"), uint64(i), []byte("blob"), 0, chunks)
		buckets[idx]++
	}

	expected := samples / chunks
	for i, count := range buckets {
		require.Greater(t, count, expected/2,
			"chunk %d was selected %d times, expected around %d; distribution is skewed", i, count, expected)
		require.Less(t, count, expected*2,
			"chunk %d was selected %d times, expected around %d; distribution is skewed", i, count, expected)
	}
}

func TestChallengeChunkIndexHandlesZeroChunks(t *testing.T) {
	require.Equal(t, uint32(0), types.ChallengeChunkIndex([]byte("h"), 1, []byte("b"), 0, 0))
}

// TestCanonicalChallengeResponseBytesIsUnambiguous: without length prefixes,
// different (chunkHash, path) pairs could serialise identically.
func TestCanonicalChallengeResponseBytesIsUnambiguous(t *testing.T) {
	h1 := sha256.Sum256([]byte("a"))
	h2 := sha256.Sum256([]byte("b"))

	a := types.CanonicalChallengeResponseBytes(1, h1[:], [][]byte{h2[:]})
	b := types.CanonicalChallengeResponseBytes(1, h2[:], [][]byte{h1[:]})
	require.NotEqual(t, a, b, "swapping the chunk hash and the path element produced the same bytes")

	c := types.CanonicalChallengeResponseBytes(2, h1[:], [][]byte{h2[:]})
	require.NotEqual(t, a, c, "the challenge id is not committed to")

	d := types.CanonicalChallengeResponseBytes(1, h1[:], [][]byte{h2[:], h2[:]})
	require.NotEqual(t, a, d, "the path length is not committed to")
}

// TestAssignmentValidationTiesSizeToChunking: a provider must not be able to
// claim byte-hours for a large blob while proving a small one.
func TestAssignmentValidationTiesSizeToChunking(t *testing.T) {
	root := sha256.Sum256([]byte("root"))
	base := func() types.StorageAssignment {
		return types.StorageAssignment{
			BlobId:     []byte("blob-1"),
			Provider:   "hash1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqhpxlyc",
			SizeBytes:  4 * 1024 * 1024,
			ChunkSize:  1024 * 1024,
			ChunkCount: 4,
			MerkleRoot: root[:],
		}
	}

	// The provider address above must be a valid bech32 string for the rest
	// of validation to be reached; build one properly instead.
	valid := base()
	valid.Provider = testProviderAddress()
	require.NoError(t, valid.Validate())

	// Claiming 1 GiB while providing only 4 one-MiB chunks.
	inflated := valid
	inflated.SizeBytes = 1024 * 1024 * 1024
	err := inflated.Validate()
	require.Error(t, err, "a size inconsistent with the chunking was accepted")
	require.Contains(t, err.Error(), "inconsistent")

	// Understating size below the previous chunk boundary is also rejected.
	deflated := valid
	deflated.SizeBytes = 1024
	require.Error(t, deflated.Validate())

	// The last chunk may be partial, which is the normal case.
	partial := valid
	partial.SizeBytes = 3*1024*1024 + 1
	require.NoError(t, partial.Validate(), "a partial final chunk was rejected")
}

// SelectIndex must always land inside the slice it is choosing from. An
// out-of-range pick here would panic inside BeginBlock, which is a chain halt
// rather than a bad selection.
func TestSelectIndexStaysInRange(t *testing.T) {
	appHash := []byte("app-hash-for-selection")
	operator := []byte("hash1operator")

	for n := 1; n <= 64; n++ {
		for round := 0; round < 8; round++ {
			for id := uint64(1); id <= 4; id++ {
				got := types.SelectIndex(appHash, id, operator, round, n)
				if got < 0 || got >= n {
					t.Fatalf("SelectIndex(n=%d, round=%d, id=%d) = %d, out of range",
						n, round, id, got)
				}
			}
		}
	}
}

// An empty list must yield zero rather than panicking on a modulo by zero.
func TestSelectIndexHandlesEmptyList(t *testing.T) {
	for _, n := range []int{0, -1} {
		if got := types.SelectIndex([]byte("h"), 1, []byte("op"), 0, n); got != 0 {
			t.Fatalf("SelectIndex with n=%d = %d, want 0", n, got)
		}
	}
}

// The selection must be deterministic: every validator computes the same
// challenge from the same state, or they disagree on the block.
func TestSelectIndexIsDeterministic(t *testing.T) {
	a := types.SelectIndex([]byte("hash"), 7, []byte("op"), 3, 100)
	b := types.SelectIndex([]byte("hash"), 7, []byte("op"), 3, 100)
	if a != b {
		t.Fatalf("two calls with identical inputs gave %d and %d", a, b)
	}
}

// Changing any input must be able to change the selection, otherwise a
// provider could predict which assignment is challenged and keep only that
// data on disk.
func TestSelectIndexRespondsToEveryInput(t *testing.T) {
	base := types.SelectIndex([]byte("hash"), 7, []byte("op"), 3, 1_000)

	cases := map[string]int{
		"different app hash":  types.SelectIndex([]byte("other"), 7, []byte("op"), 3, 1_000),
		"different challenge": types.SelectIndex([]byte("hash"), 8, []byte("op"), 3, 1_000),
		"different operator":  types.SelectIndex([]byte("hash"), 7, []byte("op2"), 3, 1_000),
		"different round":     types.SelectIndex([]byte("hash"), 7, []byte("op"), 4, 1_000),
	}

	same := 0
	for _, got := range cases {
		if got == base {
			same++
		}
	}
	// With a thousand possible values, all four colliding with the base would
	// mean an input is being ignored rather than being unlucky.
	if same == len(cases) {
		t.Fatalf("all four varied inputs produced the same selection (%d); "+
			"an input is not reaching the hash", base)
	}
}

// SelectIndex and ChallengeChunkIndex must be domain-separated, so that the
// assignment chosen and the chunk chosen within it are not correlated.
func TestSelectIndexIsDomainSeparatedFromChunkIndex(t *testing.T) {
	appHash := []byte("shared-app-hash")
	operator := []byte("hash1operator")

	const n = 4096
	collisions := 0
	for id := uint64(1); id <= 32; id++ {
		sel := types.SelectIndex(appHash, id, operator, 0, n)
		chunk := int(types.ChallengeChunkIndex(appHash, id, operator, 0, n))
		if sel == chunk {
			collisions++
		}
	}
	// Over 32 trials with 4096 outcomes, the expected number of coincidental
	// collisions is well under one. More than a handful means the two are
	// computing the same value from the same inputs.
	if collisions > 3 {
		t.Fatalf("%d of 32 selections matched the chunk index; the two functions "+
			"do not appear to be domain-separated", collisions)
	}
}

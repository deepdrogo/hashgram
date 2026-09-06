package keeper_test

import (
	"fmt"
	"testing"

	"github.com/stretchr/testify/require"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/serviceproof/types"
)

// blob is a DEVNET-only test blob with a real Merkle tree over it.
type blob struct {
	id     []byte
	chunks [][]byte
	root   [32]byte
}

func newBlob(label string, chunkCount int, chunkSize int) blob {
	chunks := make([][]byte, chunkCount)
	for i := range chunks {
		c := make([]byte, chunkSize)
		copy(c, fmt.Sprintf("%s-chunk-%d-", label, i))
		chunks[i] = c
	}
	return blob{
		id:     []byte(label),
		chunks: chunks,
		root:   types.MerkleRoot(chunks),
	}
}

func (b blob) assignment(provider string, replica uint32) types.StorageAssignment {
	chunkSize := len(b.chunks[0])
	return types.StorageAssignment{
		BlobId:       b.id,
		Provider:     provider,
		ReplicaIndex: replica,
		SizeBytes:    uint64(len(b.chunks) * chunkSize),
		ChunkSize:    uint32(chunkSize),
		ChunkCount:   uint32(len(b.chunks)),
		MerkleRoot:   b.root[:],
	}
}

// registerAssigner adds an assigner through the governance path.
func (f *fixture) registerAssigner(t *testing.T, addr sdk.AccAddress) {
	t.Helper()
	require.NoError(t, f.keeper.UpdateParams(f.ctx, f.gov, f.params(t), []string{addr.String()}))
}

func assignerAddress() sdk.AccAddress {
	b := make([]byte, 20)
	copy(b, "DEVNET-assigner")
	return sdk.AccAddress(b)
}

// answer produces a valid, node-signed challenge response.
func (f *fixture) answer(t *testing.T, n node, b blob, ch types.StorageChallenge) types.ChallengeResponse {
	t.Helper()

	leaf := types.LeafHash(b.chunks[ch.ChunkIndex])
	path := types.MerkleProof(b.chunks, int(ch.ChunkIndex))

	resp := types.ChallengeResponse{
		ChallengeId: ch.Id,
		ChunkHash:   leaf[:],
		MerklePath:  path,
	}

	digest, err := f.network.SigningDigest(f.ctx, hgparams.PurposeStorageChallenge,
		types.CanonicalChallengeResponseBytes(resp.ChallengeId, resp.ChunkHash, resp.MerklePath))
	require.NoError(t, err)

	sig, err := n.priv.Sign(digest[:])
	require.NoError(t, err)
	resp.NodeSignature = sig

	return resp
}

// ---------------------------------------------------------------------------
// Assignment: what turns declared disk into payable disk
// ---------------------------------------------------------------------------

// TestProvidersCannotAssignWorkToThemselves is the reason the assigner set
// exists. Self-assignment would be self-reporting under a different name.
func TestProvidersCannotAssignWorkToThemselves(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	f.register(t, n, types.SERVICE_ROLE_STORAGE)

	b := newBlob("blob-1", 8, 1024)

	err := f.keeper.AssignStorage(f.ctx, n.operator.String(), b.assignment(n.operator.String(), 0))
	require.Error(t, err)
	require.True(t, types.ErrNotAnAssigner.Is(err), "got %v", err)
}

// TestNoAssignersMeansNoStorageAssignments: the genesis default must pay no
// storage rewards at all.
func TestNoAssignersMeansNoStorageAssignments(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	f.register(t, n, types.SERVICE_ROLE_STORAGE)

	assigners, err := f.keeper.Assigners(f.ctx)
	require.NoError(t, err)
	require.Empty(t, assigners)

	b := newBlob("blob-1", 8, 1024)
	err = f.keeper.AssignStorage(f.ctx, assignerAddress().String(), b.assignment(n.operator.String(), 0))
	require.Error(t, err)
	require.True(t, types.ErrNotAnAssigner.Is(err), "got %v", err)
}

func TestAssignStorageRecordsResponsibility(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	f.register(t, n, types.SERVICE_ROLE_STORAGE)
	f.registerAssigner(t, assignerAddress())

	b := newBlob("blob-1", 8, 1024)
	require.NoError(t, f.keeper.AssignStorage(f.ctx, assignerAddress().String(),
		b.assignment(n.operator.String(), 0)))

	assigned, err := f.keeper.AssignedBytes(f.ctx, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, uint64(8*1024), assigned)

	// A duplicate assignment is rejected.
	err = f.keeper.AssignStorage(f.ctx, assignerAddress().String(),
		b.assignment(n.operator.String(), 0))
	require.Error(t, err)
	require.True(t, types.ErrAssignmentExists.Is(err), "got %v", err)
}

// TestAssignmentRespectsDeclaredCapacity
func TestAssignmentRespectsDeclaredCapacity(t *testing.T) {
	f := setup(t)
	n := newNode("a")

	bond := f.params(t).MinBond
	f.bank.Fund(n.operator, bond)
	require.NoError(t, f.keeper.RegisterProvider(f.ctx, &types.MsgRegisterProvider{
		Operator:             n.operator.String(),
		RewardAddress:        n.reward.String(),
		NodePubkey:           n.priv.PubKey().Bytes(),
		NodeKeyType:          types.KEY_TYPE_ED25519,
		Roles:                []types.ServiceRole{types.SERVICE_ROLE_STORAGE},
		Bond:                 bond,
		DeclaredStorageBytes: 4096, // deliberately tiny
	}))
	f.registerAssigner(t, assignerAddress())

	b := newBlob("blob-1", 8, 1024) // 8192 bytes, twice the declared capacity
	err := f.keeper.AssignStorage(f.ctx, assignerAddress().String(),
		b.assignment(n.operator.String(), 0))
	require.Error(t, err)
	require.True(t, types.ErrCapacityExceeded.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// Challenges
// ---------------------------------------------------------------------------

func (f *fixture) issueAndFetch(t *testing.T, n node, epoch uint64) []types.StorageChallenge {
	t.Helper()
	require.NoError(t, f.keeper.IssueChallenges(f.ctx, epoch, f.params(t)))
	open, err := f.keeper.OpenChallenges(f.ctx, n.operator.String())
	require.NoError(t, err)
	return open
}

func (f *fixture) storageProvider(t *testing.T, label string, b blob) node {
	t.Helper()
	n := newNode(label)
	f.register(t, n, types.SERVICE_ROLE_STORAGE)
	f.registerAssigner(t, assignerAddress())
	require.NoError(t, f.keeper.AssignStorage(f.ctx, assignerAddress().String(),
		b.assignment(n.operator.String(), 0)))
	return n
}

func TestValidMerkleAnswerPassesChallenge(t *testing.T) {
	f := setup(t)
	b := newBlob("blob-1", 16, 1024)
	n := f.storageProvider(t, "a", b)

	open := f.issueAndFetch(t, n, 0)
	require.NotEmpty(t, open, "no challenges were issued to a provider with assignments")

	ch := open[0]
	passed, err := f.keeper.AnswerChallenge(f.ctx, n.operator.String(), f.answer(t, n, b, ch))
	require.NoError(t, err)
	require.True(t, passed)

	credit, err := f.keeper.GetCredit(f.ctx, 0, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, uint64(1), credit.ChallengesPassed)
	require.Equal(t, uint64(len(open)), credit.ChallengesIssued)
}

// TestWrongChunkFailsChallengeAndScoresFraud is the property that makes a
// storage challenge worth anything: a provider that discarded the challenged
// chunk cannot pass.
func TestWrongChunkFailsChallengeAndScoresFraud(t *testing.T) {
	f := setup(t)
	b := newBlob("blob-1", 16, 1024)
	n := f.storageProvider(t, "a", b)

	open := f.issueAndFetch(t, n, 0)
	ch := open[0]

	// Answer with a different chunk's hash but the correct path.
	resp := f.answer(t, n, b, ch)
	wrongIndex := (ch.ChunkIndex + 1) % uint32(len(b.chunks))
	wrong := types.LeafHash(b.chunks[wrongIndex])
	resp.ChunkHash = wrong[:]

	// Re-sign so the failure is the proof, not the signature.
	digest, err := f.network.SigningDigest(f.ctx, hgparams.PurposeStorageChallenge,
		types.CanonicalChallengeResponseBytes(resp.ChallengeId, resp.ChunkHash, resp.MerklePath))
	require.NoError(t, err)
	sig, err := n.priv.Sign(digest[:])
	require.NoError(t, err)
	resp.NodeSignature = sig

	passed, err := f.keeper.AnswerChallenge(f.ctx, n.operator.String(), resp)
	require.Error(t, err)
	require.False(t, passed)
	require.True(t, types.ErrInvalidMerkleProof.Is(err), "got %v", err)

	p, err := f.keeper.GetProvider(f.ctx, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, types.FraudScoreForChallengeFailure, p.FraudScore)

	reports, err := f.keeper.FraudReports(f.ctx, n.operator.String())
	require.NoError(t, err)
	require.Len(t, reports, 1)
	require.Equal(t, types.FraudReasonChallengeFailed, reports[0].Reason)
}

// TestUnsignedChallengeAnswerIsRejected: without the node signature, a proof
// observed on chain could be replayed by anyone.
func TestUnsignedChallengeAnswerIsRejected(t *testing.T) {
	f := setup(t)
	b := newBlob("blob-1", 16, 1024)
	n := f.storageProvider(t, "a", b)

	open := f.issueAndFetch(t, n, 0)
	resp := f.answer(t, n, b, open[0])

	// A signature from a different key: what a peer replaying an observed
	// proof would have.
	other := newNode("thief")
	digest, err := f.network.SigningDigest(f.ctx, hgparams.PurposeStorageChallenge,
		types.CanonicalChallengeResponseBytes(resp.ChallengeId, resp.ChunkHash, resp.MerklePath))
	require.NoError(t, err)
	sig, err := other.priv.Sign(digest[:])
	require.NoError(t, err)
	resp.NodeSignature = sig

	_, err = f.keeper.AnswerChallenge(f.ctx, n.operator.String(), resp)
	require.Error(t, err)
	require.True(t, types.ErrInvalidSignature.Is(err), "got %v", err)
}

// TestChallengeCannotBeAnsweredTwice
func TestChallengeCannotBeAnsweredTwice(t *testing.T) {
	f := setup(t)
	b := newBlob("blob-1", 16, 1024)
	n := f.storageProvider(t, "a", b)

	open := f.issueAndFetch(t, n, 0)
	resp := f.answer(t, n, b, open[0])

	_, err := f.keeper.AnswerChallenge(f.ctx, n.operator.String(), resp)
	require.NoError(t, err)

	_, err = f.keeper.AnswerChallenge(f.ctx, n.operator.String(), resp)
	require.Error(t, err)
	require.True(t, types.ErrChallengeAnswered.Is(err), "got %v", err)
}

// TestChallengeCannotBeAnsweredByAnotherProvider
func TestChallengeCannotBeAnsweredByAnotherProvider(t *testing.T) {
	f := setup(t)
	b := newBlob("blob-1", 16, 1024)
	n := f.storageProvider(t, "a", b)

	other := newNode("b")
	f.register(t, other, types.SERVICE_ROLE_STORAGE)

	open := f.issueAndFetch(t, n, 0)
	_, err := f.keeper.AnswerChallenge(f.ctx, other.operator.String(), f.answer(t, n, b, open[0]))
	require.Error(t, err)
	require.True(t, types.ErrChallengeNotFound.Is(err), "got %v", err)
}

// TestLateChallengeAnswerIsRejected
func TestLateChallengeAnswerIsRejected(t *testing.T) {
	f := setup(t)
	b := newBlob("blob-1", 16, 1024)
	n := f.storageProvider(t, "a", b)

	open := f.issueAndFetch(t, n, 0)
	ch := open[0]
	resp := f.answer(t, n, b, ch)

	late := f.ctx.WithBlockHeight(ch.DeadlineHeight + 1)
	_, err := f.keeper.AnswerChallenge(late, n.operator.String(), resp)
	require.Error(t, err)
	require.True(t, types.ErrChallengeExpired.Is(err), "got %v", err)
}

// TestMissedChallengeCountsAsFailure: silence must not be free, or a provider
// could ignore every challenge and keep earning.
func TestMissedChallengeCountsAsFailure(t *testing.T) {
	f := setup(t)
	b := newBlob("blob-1", 16, 1024)
	n := f.storageProvider(t, "a", b)

	open := f.issueAndFetch(t, n, 0)
	require.NotEmpty(t, open)

	past := f.ctx.WithBlockHeight(open[0].DeadlineHeight + 1)
	require.NoError(t, f.keeper.ExpireChallenges(past, f.params(t)))

	p, err := f.keeper.GetProvider(past, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, types.FraudScoreForChallengeFailure*uint32(len(open)), p.FraudScore)

	reports, err := f.keeper.FraudReports(past, n.operator.String())
	require.NoError(t, err)
	require.Len(t, reports, len(open))
	require.Equal(t, types.FraudReasonChallengeMissed, reports[0].Reason)

	// The challenges are now closed and no longer open.
	stillOpen, err := f.keeper.OpenChallenges(past, n.operator.String())
	require.NoError(t, err)
	require.Empty(t, stillOpen)
}

// TestRepeatedFailuresJailAndSlash
func TestRepeatedFailuresJailAndSlash(t *testing.T) {
	f := setup(t)
	b := newBlob("blob-1", 16, 1024)
	n := f.storageProvider(t, "a", b)

	bondBefore := f.keeper.BondedTotal(f.ctx)
	reserveBefore := f.keeper.ReserveRemaining(f.ctx)

	// Four missed challenges at 25 points each reach the threshold of 100.
	params := f.params(t)
	require.Equal(t, uint32(100), params.FraudScoreJailThreshold)
	require.Equal(t, uint32(25), types.FraudScoreForChallengeFailure)

	ctx := f.ctx
	for round := 0; round < 4; round++ {
		require.NoError(t, f.keeper.IssueChallenges(ctx, 0, params))
		open, err := f.keeper.OpenChallenges(ctx, n.operator.String())
		require.NoError(t, err)
		if len(open) == 0 {
			break
		}
		ctx = ctx.WithBlockHeight(open[0].DeadlineHeight + 1)
		require.NoError(t, f.keeper.ExpireChallenges(ctx, params))

		p, err := f.keeper.GetProvider(ctx, n.operator.String())
		require.NoError(t, err)
		if p.Jailed {
			break
		}
	}

	p, err := f.keeper.GetProvider(ctx, n.operator.String())
	require.NoError(t, err)
	require.True(t, p.Jailed, "a provider that failed every challenge was not jailed")
	require.Equal(t, ctx.BlockHeight()+params.JailDurationBlocks, p.JailedUntilHeight)

	// 5% of the bond was slashed and returned to the reserve, not burned.
	require.True(t, p.Bond.AmountOf(hgparams.BaseCoinDenom).
		LT(bondBefore.AmountOf(hgparams.BaseCoinDenom)),
		"the bond was not slashed")
	require.True(t, f.keeper.ReserveRemaining(ctx).AmountOf(hgparams.BaseCoinDenom).
		GT(reserveBefore.AmountOf(hgparams.BaseCoinDenom)),
		"slashed bond did not return to the reserve")

	reserve, err := f.keeper.GetReserve(ctx)
	require.NoError(t, err)
	require.False(t, reserve.TotalSlashed.IsZero())
	require.NoError(t, reserve.Validate())
}

// TestJailedProviderCannotEarnOrBeAssigned
func TestJailedProviderCannotEarnOrBeAssigned(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	f.register(t, n, types.SERVICE_ROLE_RELAY, types.SERVICE_ROLE_STORAGE)
	f.registerAssigner(t, assignerAddress())

	p, err := f.keeper.GetProvider(f.ctx, n.operator.String())
	require.NoError(t, err)
	p.Jailed = true
	p.JailedUntilHeight = f.ctx.BlockHeight() + 1000
	require.NoError(t, f.keeper.SetProvider(f.ctx, p))

	// No receipts.
	r := f.signReceipt(t, newClient(), n.operator, types.SERVICE_ROLE_RELAY, 0, 1, 10*oneGiB)
	_, rejected, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(1), rejected)
	require.Contains(t, reasons[0], "jailed")

	// No new assignments.
	b := newBlob("blob-1", 8, 1024)
	err = f.keeper.AssignStorage(f.ctx, assignerAddress().String(), b.assignment(n.operator.String(), 0))
	require.Error(t, err)

	// Unjailing before the period elapses is refused.
	err = f.keeper.Unjail(f.ctx, n.operator.String())
	require.Error(t, err)
	require.True(t, types.ErrJailPeriodActive.Is(err), "got %v", err)

	// After the period, unjailing succeeds and resets the score.
	later := f.ctx.WithBlockHeight(p.JailedUntilHeight)
	require.NoError(t, f.keeper.Unjail(later, n.operator.String()))
	p, err = f.keeper.GetProvider(later, n.operator.String())
	require.NoError(t, err)
	require.False(t, p.Jailed)
	require.Equal(t, uint32(0), p.FraudScore)
}

// ---------------------------------------------------------------------------
// Storage credit: declared vs assigned vs verified
// ---------------------------------------------------------------------------

// TestEmptyDeclaredDiskEarnsNothing is §28 and §116.21: a provider that
// declares a petabyte and is assigned nothing earns nothing.
func TestEmptyDeclaredDiskEarnsNothing(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	f.register(t, n, types.SERVICE_ROLE_STORAGE) // declares 1 TiB

	p, err := f.keeper.GetProvider(f.ctx, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, uint64(1<<40), p.DeclaredStorageBytes, "the node declared a terabyte")

	assigned, err := f.keeper.AssignedBytes(f.ctx, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, uint64(0), assigned, "nothing was assigned")

	// No challenges are issued, because there is nothing to challenge.
	require.NoError(t, f.keeper.IssueChallenges(f.ctx, 0, f.params(t)))
	open, err := f.keeper.OpenChallenges(f.ctx, n.operator.String())
	require.NoError(t, err)
	require.Empty(t, open)

	require.NoError(t, f.keeper.AccrueStorageCredit(f.ctx, 0, f.params(t)))
	credit, err := f.keeper.GetCredit(f.ctx, 0, n.operator.String())
	require.NoError(t, err)
	require.True(t, credit.StorageCredit.IsZero(),
		"a terabyte of declared but unassigned disk earned %s credit", credit.StorageCredit)

	// And settlement pays nothing.
	require.NoError(t, f.keeper.SettleEpoch(f.ctx, 0, f.ctx.BlockHeight(), f.params(t)))
	require.True(t, f.bank.GetAllBalances(f.ctx, n.reward).IsZero(),
		"empty declared disk was paid")
}

// TestAssignedStorageWithoutChallengesEarnsNothing: unchallenged storage is
// self-reported storage.
func TestAssignedStorageWithoutChallengesEarnsNothing(t *testing.T) {
	f := setup(t)
	b := newBlob("blob-1", 4, 1024*1024*1024) // 4 GiB
	n := f.storageProvider(t, "a", b)

	// Move to the next epoch without ever issuing a challenge.
	require.NoError(t, f.keeper.AccrueStorageCredit(f.ctx, 1, f.params(t)))

	credit, err := f.keeper.GetCredit(f.ctx, 1, n.operator.String())
	require.NoError(t, err)
	require.True(t, credit.StorageCredit.IsZero(),
		"storage with no challenges issued earned %s credit", credit.StorageCredit)
}

// TestStorageCreditScalesWithChallengeSuccess
func TestStorageCreditScalesWithChallengeSuccess(t *testing.T) {
	f := setup(t)
	b := newBlob("blob-1", 4, 1024*1024*1024) // 4 GiB in 4 chunks
	n := f.storageProvider(t, "a", b)

	params := f.params(t)

	// Fabricate a half-passed challenge record for epoch 1 and mark the
	// assignment as held since epoch 0.
	credit, err := f.keeper.GetCredit(f.ctx, 1, n.operator.String())
	require.NoError(t, err)
	credit.ChallengesIssued = 4
	credit.ChallengesPassed = 2
	require.NoError(t, f.keeper.SetCredit(f.ctx, credit))

	require.NoError(t, f.keeper.AccrueStorageCredit(f.ctx, 1, params))

	credit, err = f.keeper.GetCredit(f.ctx, 1, n.operator.String())
	require.NoError(t, err)

	// 4 GiB * 100 credit per GiB-epoch * (2/4) = 200.
	require.Equal(t, "200", credit.StorageCredit.String())
}

func TestStorageCreditIsNotCountedTwiceForOneEpoch(t *testing.T) {
	f := setup(t)
	b := newBlob("blob-1", 4, 1024*1024*1024)
	n := f.storageProvider(t, "a", b)

	credit, err := f.keeper.GetCredit(f.ctx, 1, n.operator.String())
	require.NoError(t, err)
	credit.ChallengesIssued = 1
	credit.ChallengesPassed = 1
	require.NoError(t, f.keeper.SetCredit(f.ctx, credit))

	require.NoError(t, f.keeper.AccrueStorageCredit(f.ctx, 1, f.params(t)))
	first, err := f.keeper.GetCredit(f.ctx, 1, n.operator.String())
	require.NoError(t, err)

	require.NoError(t, f.keeper.AccrueStorageCredit(f.ctx, 1, f.params(t)))
	second, err := f.keeper.GetCredit(f.ctx, 1, n.operator.String())
	require.NoError(t, err)

	require.Equal(t, first.StorageCredit.String(), second.StorageCredit.String(),
		"running accrual twice for the same epoch doubled the credit")
}

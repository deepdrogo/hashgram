package keeper_test

import (
	"fmt"
	"os"
	"testing"

	"github.com/stretchr/testify/require"

	"cosmossdk.io/log"
	storetypes "cosmossdk.io/store/types"

	"github.com/cosmos/cosmos-sdk/codec"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	"github.com/cosmos/cosmos-sdk/crypto/keys/ed25519"
	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	cryptotypes "github.com/cosmos/cosmos-sdk/crypto/types"
	"github.com/cosmos/cosmos-sdk/runtime"
	"github.com/cosmos/cosmos-sdk/testutil"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	hgbank "github.com/hashgram/hashgram/x/founder/keeper/testutil"
	networkkeeper "github.com/hashgram/hashgram/x/network/keeper"
	networktypes "github.com/hashgram/hashgram/x/network/types"
	"github.com/hashgram/hashgram/x/welcome/keeper"
	"github.com/hashgram/hashgram/x/welcome/types"
)

func TestMain(m *testing.M) {
	hgparams.SetSDKConfig()
	os.Exit(m.Run())
}

// attestorKey is a DEVNET ONLY signing identity for an attestor.
type attestorKey struct {
	priv cryptotypes.PrivKey
	reg  types.Attestor
}

func newSecp256k1Attestor(t *testing.T, name, method string, cap uint64) attestorKey {
	t.Helper()
	priv := secp256k1.GenPrivKey()
	pub := priv.PubKey()
	return attestorKey{
		priv: priv,
		reg: types.Attestor{
			Address:           sdk.AccAddress(pub.Address()).String(),
			Pubkey:            pub.Bytes(),
			KeyType:           types.KEY_TYPE_SECP256K1,
			Name:              name,
			Method:            method,
			Enabled:           true,
			MaxClaimsPerEpoch: cap,
		},
	}
}

func newEd25519Attestor(t *testing.T, name, method string, cap uint64) attestorKey {
	t.Helper()
	priv := ed25519.GenPrivKey()
	pub := priv.PubKey()
	return attestorKey{
		priv: priv,
		reg: types.Attestor{
			Address:           sdk.AccAddress(pub.Address()).String(),
			Pubkey:            pub.Bytes(),
			KeyType:           types.KEY_TYPE_ED25519,
			Name:              name,
			Method:            method,
			Enabled:           true,
			MaxClaimsPerEpoch: cap,
		},
	}
}

type fixture struct {
	ctx      sdk.Context
	keeper   keeper.Keeper
	network  networkkeeper.Keeper
	bank     *hgbank.Bank
	gov      string
	attestor attestorKey
}

const testMethod = "device-attestation"

func setup(t *testing.T, attestors ...types.Attestor) *fixture {
	t.Helper()

	welcomeKey := storetypes.NewKVStoreKey(types.StoreKey)
	networkKey := storetypes.NewKVStoreKey(networktypes.StoreKey)

	testCtx := testutil.DefaultContextWithDB(t, welcomeKey, storetypes.NewTransientStoreKey("transient_test"))
	cms := testCtx.CMS
	cms.MountStoreWithDB(networkKey, storetypes.StoreTypeIAVL, testCtx.DB)
	require.NoError(t, cms.LoadLatestVersion())

	ctx := testCtx.Ctx.
		WithMultiStore(cms).
		WithChainID(hgparams.ChainIDMainnet).
		WithBlockHeight(100)

	cdc := codec.NewProtoCodec(codectypes.NewInterfaceRegistry())
	bank := hgbank.NewBank()
	accounts := hgbank.NewAccounts(types.ModuleName, authtypes.FeeCollectorName)
	gov := authtypes.NewModuleAddress("gov").String()

	nk := networkkeeper.NewKeeper(cdc, runtime.NewKVStoreService(networkKey), log.NewNopLogger())
	require.NoError(t, nk.InitGenesis(ctx, *networktypes.MainnetGenesis()))

	k := keeper.NewKeeper(cdc, runtime.NewKVStoreService(welcomeKey),
		accounts, bank, nk, gov, log.NewNopLogger())

	gs := types.DefaultGenesis()
	gs.Params.Attestors = attestors
	require.NoError(t, k.InitGenesis(ctx, *gs))

	// Fund the welcome pool as Mainnet genesis does, out of the Growth
	// allocation.
	bank.FundModule(types.ModuleName, types.MaxPool())

	return &fixture{ctx: ctx, keeper: k, network: nk, bank: bank, gov: gov}
}

func setupWithAttestor(t *testing.T) *fixture {
	t.Helper()
	at := newSecp256k1Attestor(t, "hashgram-device-attestation-v1", testMethod, 1_000)
	f := setup(t, at.reg)
	f.attestor = at
	return f
}

func subjectAddr(label string) sdk.AccAddress {
	b := make([]byte, 20)
	copy(b, label)
	return sdk.AccAddress(b)
}

// sign produces a valid attestation for a subject.
func (f *fixture) sign(t *testing.T, at attestorKey, subject sdk.AccAddress, nonce uint64) types.EligibilityAttestation {
	t.Helper()

	att := types.EligibilityAttestation{
		Subject:      subject.String(),
		Attestor:     at.reg.Address,
		Nonce:        nonce,
		ExpiryHeight: f.ctx.BlockHeight() + 1_000,
		Method:       at.reg.Method,
		Confidence:   9_000,
	}

	digest, err := f.keeper.AttestationDigest(f.ctx, att)
	require.NoError(t, err)

	sig, err := at.priv.Sign(digest[:])
	require.NoError(t, err)
	att.Signature = sig

	return att
}

// ---------------------------------------------------------------------------
// The core anti-Sybil property
// ---------------------------------------------------------------------------

// TestKeyCreationAloneEarnsNothing is §20 and §116 at the module level: an
// address with a freshly generated key and no attestation gets nothing. The
// only thing standing between "I made a keypair" and "free HASH" is a
// registered attestor's signature.
func TestKeyCreationAloneEarnsNothing(t *testing.T) {
	f := setupWithAttestor(t)
	subject := subjectAddr("DEVNET-fresh-key")

	// Well-formed attestation, but with a signature from a key nobody
	// registered: exactly what a user with a fresh keypair could produce for
	// themselves.
	rogue := newSecp256k1Attestor(t, "rogue", testMethod, 1_000)
	att := f.sign(t, rogue, subject, 1)

	_, err := f.keeper.Claim(f.ctx, att)
	require.Error(t, err)
	require.True(t, types.ErrUnknownAttestor.Is(err), "got %v", err)
	require.True(t, f.bank.GetAllBalances(f.ctx, subject).IsZero())
}

// TestNoAttestorsMeansNoClaims: the default configuration must pay nothing at
// all.
func TestNoAttestorsMeansNoClaims(t *testing.T) {
	f := setup(t) // no attestors registered
	at := newSecp256k1Attestor(t, "would-be", testMethod, 1_000)
	subject := subjectAddr("DEVNET-subject-1")

	att := f.sign(t, at, subject, 1)
	_, err := f.keeper.Claim(f.ctx, att)
	require.Error(t, err)
	require.True(t, types.ErrUnknownAttestor.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// The happy path
// ---------------------------------------------------------------------------

func TestFirstClaimIsSequenceOneAndFiftyHash(t *testing.T) {
	f := setupWithAttestor(t)
	subject := subjectAddr("DEVNET-subject-1")

	poolBefore := f.keeper.PoolRemaining(f.ctx)

	rec, err := f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subject, 1))
	require.NoError(t, err)

	require.Equal(t, uint64(1), rec.Sequence, "the first eligible user must be user number 1")
	require.Equal(t, "50000000uhash", rec.Amount.String())
	require.Equal(t, subject.String(), rec.Subject)
	require.Equal(t, f.attestor.reg.Address, rec.Attestor)
	require.Equal(t, testMethod, rec.Method)
	require.Equal(t, int64(100), rec.Height)

	require.Equal(t, "50000000uhash", f.bank.GetAllBalances(f.ctx, subject).String())

	// The pool paid for it; nothing was minted.
	expectedPool, _ := poolBefore.SafeSub(rec.Amount...)
	require.Equal(t, expectedPool.String(), f.keeper.PoolRemaining(f.ctx).String())

	next, err := f.keeper.NextSequence(f.ctx)
	require.NoError(t, err)
	require.Equal(t, uint64(2), next)

	paid, err := f.keeper.TotalPaid(f.ctx)
	require.NoError(t, err)
	require.Equal(t, "50000000uhash", paid.String())

	count, err := f.keeper.ClaimsPaid(f.ctx)
	require.NoError(t, err)
	require.Equal(t, uint64(1), count)
}

// TestSequencesAreAssignedInOrder: distinct subjects get consecutive
// positions. Cosmos executes transactions in a block sequentially, so the
// counter needs no locking; this test pins that the counter actually advances
// once per successful claim and not per attempt.
func TestSequencesAreAssignedInOrder(t *testing.T) {
	f := setupWithAttestor(t)

	for i := uint64(1); i <= 5; i++ {
		subject := subjectAddr(fmt.Sprintf("DEVNET-subject-%d", i))
		rec, err := f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subject, i))
		require.NoError(t, err)
		require.Equal(t, i, rec.Sequence)
		require.Equal(t, "50000000uhash", rec.Amount.String())
	}

	next, err := f.keeper.NextSequence(f.ctx)
	require.NoError(t, err)
	require.Equal(t, uint64(6), next)

	paid, err := f.keeper.TotalPaid(f.ctx)
	require.NoError(t, err)
	require.Equal(t, "250000000uhash", paid.String(), "5 x 50 HASH")
}

// TestFailedClaimsDoNotBurnSequenceNumbers: a rejected attempt must not
// advance the counter, otherwise an attacker could push real users into a
// lower tier for free.
func TestFailedClaimsDoNotBurnSequenceNumbers(t *testing.T) {
	f := setupWithAttestor(t)
	rogue := newSecp256k1Attestor(t, "rogue", testMethod, 1_000)

	for i := uint64(1); i <= 10; i++ {
		_, err := f.keeper.Claim(f.ctx, f.sign(t, rogue, subjectAddr(fmt.Sprintf("DEVNET-x-%d", i)), i))
		require.Error(t, err)
	}

	next, err := f.keeper.NextSequence(f.ctx)
	require.NoError(t, err)
	require.Equal(t, uint64(1), next, "rejected claims advanced the sequence counter")

	// A genuine claim still gets position 1.
	rec, err := f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subjectAddr("DEVNET-real"), 1))
	require.NoError(t, err)
	require.Equal(t, uint64(1), rec.Sequence)
}

func TestEd25519AttestorWorks(t *testing.T) {
	at := newEd25519Attestor(t, "hashgram-device-key-v1", testMethod, 100)
	f := setup(t, at.reg)
	f.attestor = at

	rec, err := f.keeper.Claim(f.ctx, f.sign(t, at, subjectAddr("DEVNET-ed25519"), 1))
	require.NoError(t, err)
	require.Equal(t, uint64(1), rec.Sequence)
}

// ---------------------------------------------------------------------------
// Double claiming
// ---------------------------------------------------------------------------

// TestDoubleClaimIsRejected: one address receives at most one welcome reward
// for the lifetime of the chain, even with a fresh valid attestation.
func TestDoubleClaimIsRejected(t *testing.T) {
	f := setupWithAttestor(t)
	subject := subjectAddr("DEVNET-greedy")

	_, err := f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subject, 1))
	require.NoError(t, err)

	// A brand-new attestation with a fresh nonce for the same subject.
	_, err = f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subject, 2))
	require.Error(t, err)
	require.True(t, types.ErrAlreadyClaimed.Is(err), "got %v", err)

	require.Equal(t, "50000000uhash", f.bank.GetAllBalances(f.ctx, subject).String(),
		"the subject was paid twice")

	count, err := f.keeper.ClaimsPaid(f.ctx)
	require.NoError(t, err)
	require.Equal(t, uint64(1), count)
}

// TestDoubleClaimAcrossDifferentAttestors: registering a second attestor must
// not create a second bite at the same address.
func TestDoubleClaimAcrossDifferentAttestors(t *testing.T) {
	a1 := newSecp256k1Attestor(t, "attestor-1", testMethod, 100)
	a2 := newSecp256k1Attestor(t, "attestor-2", testMethod, 100)
	f := setup(t, a1.reg, a2.reg)

	subject := subjectAddr("DEVNET-shared")

	_, err := f.keeper.Claim(f.ctx, f.sign(t, a1, subject, 1))
	require.NoError(t, err)

	_, err = f.keeper.Claim(f.ctx, f.sign(t, a2, subject, 1))
	require.Error(t, err)
	require.True(t, types.ErrAlreadyClaimed.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

// TestAttestationReplayIsRejected: submitting the identical attestation twice
// must fail on the nonce, before the double-claim guard, so that the replay
// guard is exercised independently.
func TestAttestationReplayIsRejected(t *testing.T) {
	f := setupWithAttestor(t)

	att := f.sign(t, f.attestor, subjectAddr("DEVNET-subject-1"), 42)
	_, err := f.keeper.Claim(f.ctx, att)
	require.NoError(t, err)

	// Byte-identical resubmission.
	_, err = f.keeper.Claim(f.ctx, att)
	require.Error(t, err)
	require.True(t, types.ErrNonceReused.Is(err), "got %v", err)
}

// TestNonceIsPerAttestor: two attestors may independently use nonce 1.
func TestNonceIsPerAttestor(t *testing.T) {
	a1 := newSecp256k1Attestor(t, "attestor-1", testMethod, 100)
	a2 := newSecp256k1Attestor(t, "attestor-2", testMethod, 100)
	f := setup(t, a1.reg, a2.reg)

	_, err := f.keeper.Claim(f.ctx, f.sign(t, a1, subjectAddr("DEVNET-s1"), 1))
	require.NoError(t, err)

	_, err = f.keeper.Claim(f.ctx, f.sign(t, a2, subjectAddr("DEVNET-s2"), 1))
	require.NoError(t, err, "nonce 1 from a different attestor was treated as a replay")
}

// TestNonceReuseForADifferentSubjectIsRejected: an attestor cannot recycle a
// nonce to attest a different address.
func TestNonceReuseForADifferentSubjectIsRejected(t *testing.T) {
	f := setupWithAttestor(t)

	_, err := f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subjectAddr("DEVNET-s1"), 7))
	require.NoError(t, err)

	_, err = f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subjectAddr("DEVNET-s2"), 7))
	require.Error(t, err)
	require.True(t, types.ErrNonceReused.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// Signature forgery
// ---------------------------------------------------------------------------

// TestTamperingWithAnyFieldInvalidatesTheSignature: every field is inside the
// signed preimage, so changing any of them must break verification. In
// particular, changing the subject must not let an attacker redirect somebody
// else's attestation to their own address.
func TestTamperingWithAnyFieldInvalidatesTheSignature(t *testing.T) {
	f := setupWithAttestor(t)
	valid := f.sign(t, f.attestor, subjectAddr("DEVNET-victim"), 1)

	for name, tamper := range map[string]func(*types.EligibilityAttestation){
		"redirect the subject": func(a *types.EligibilityAttestation) {
			a.Subject = subjectAddr("DEVNET-attacker").String()
		},
		"change the nonce":       func(a *types.EligibilityAttestation) { a.Nonce = 999 },
		"extend the expiry":      func(a *types.EligibilityAttestation) { a.ExpiryHeight += 1 },
		"inflate the confidence": func(a *types.EligibilityAttestation) { a.Confidence = 10_000 },
		"flip a signature byte":  func(a *types.EligibilityAttestation) { a.Signature[0] ^= 0xff },
	} {
		t.Run(name, func(t *testing.T) {
			att := valid
			att.Signature = append([]byte(nil), valid.Signature...)
			tamper(&att)

			_, err := f.keeper.Claim(f.ctx, att)
			require.Error(t, err, "tampered attestation was accepted")
			require.True(t, types.ErrInvalidSignature.Is(err), "got %v", err)
		})
	}
}

// TestChangingTheMethodIsCaughtBeforeTheSignature: the method must match the
// registered attestor, which is checked before signature verification, so the
// error names the real problem.
func TestChangingTheMethodIsCaughtBeforeTheSignature(t *testing.T) {
	f := setupWithAttestor(t)
	att := f.sign(t, f.attestor, subjectAddr("DEVNET-subject-1"), 1)
	att.Method = "some-other-method"

	_, err := f.keeper.Claim(f.ctx, att)
	require.Error(t, err)
	require.True(t, types.ErrMethodMismatch.Is(err), "got %v", err)
}

// TestAttestationFromAForeignNetworkIsRejected is the §10 replay-protection
// property applied to attestations: a signature produced under the devnet
// signing domain must not verify on Mainnet.
func TestAttestationFromAForeignNetworkIsRejected(t *testing.T) {
	f := setupWithAttestor(t)
	subject := subjectAddr("DEVNET-cross-network")

	att := types.EligibilityAttestation{
		Subject:      subject.String(),
		Attestor:     f.attestor.reg.Address,
		Nonce:        1,
		ExpiryHeight: f.ctx.BlockHeight() + 100,
		Method:       testMethod,
		Confidence:   9_000,
	}

	// Sign under the DEVNET domain rather than Mainnet's.
	devnet := hgparams.DevnetIdentity("")
	payload, err := types.CanonicalAttestationBytes(att)
	require.NoError(t, err)
	digest := devnet.SigningDigest(hgparams.PurposeEligibility, payload)
	sig, err := f.attestor.priv.Sign(digest[:])
	require.NoError(t, err)
	att.Signature = sig

	_, err = f.keeper.Claim(f.ctx, att)
	require.Error(t, err, "an attestation signed for another network was accepted")
	require.True(t, types.ErrInvalidSignature.Is(err), "got %v", err)
}

// TestAttestationSignedForAnotherPurposeIsRejected: the same bytes signed
// under a different purpose must not work here.
func TestAttestationSignedForAnotherPurposeIsRejected(t *testing.T) {
	f := setupWithAttestor(t)

	att := types.EligibilityAttestation{
		Subject:      subjectAddr("DEVNET-purpose").String(),
		Attestor:     f.attestor.reg.Address,
		Nonce:        1,
		ExpiryHeight: f.ctx.BlockHeight() + 100,
		Method:       testMethod,
		Confidence:   9_000,
	}

	id := hgparams.MainnetIdentity("")
	payload, err := types.CanonicalAttestationBytes(att)
	require.NoError(t, err)
	digest := id.SigningDigest(hgparams.PurposeServiceReceipt, payload)
	sig, err := f.attestor.priv.Sign(digest[:])
	require.NoError(t, err)
	att.Signature = sig

	_, err = f.keeper.Claim(f.ctx, att)
	require.Error(t, err)
	require.True(t, types.ErrInvalidSignature.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// Expiry
// ---------------------------------------------------------------------------

func TestExpiredAttestationIsRejected(t *testing.T) {
	f := setupWithAttestor(t)
	att := f.sign(t, f.attestor, subjectAddr("DEVNET-late"), 1)

	late := f.ctx.WithBlockHeight(att.ExpiryHeight + 1)
	_, err := f.keeper.Claim(late, att)
	require.Error(t, err)
	require.True(t, types.ErrAttestationExpired.Is(err), "got %v", err)
}

func TestAttestationValidExactlyAtExpiryHeight(t *testing.T) {
	f := setupWithAttestor(t)
	att := f.sign(t, f.attestor, subjectAddr("DEVNET-onthedot"), 1)

	atExpiry := f.ctx.WithBlockHeight(att.ExpiryHeight)
	_, err := f.keeper.Claim(atExpiry, att)
	require.NoError(t, err, "an attestation must still be valid at its stated expiry height")
}

// TestOverlongAttestationIsRejected: an attestation that never expires is a
// bearer token that outlives the check behind it.
func TestOverlongAttestationIsRejected(t *testing.T) {
	f := setupWithAttestor(t)

	att := types.EligibilityAttestation{
		Subject:      subjectAddr("DEVNET-forever").String(),
		Attestor:     f.attestor.reg.Address,
		Nonce:        1,
		ExpiryHeight: f.ctx.BlockHeight() + types.DefaultMaxAttestationAgeBlocks + 1,
		Method:       testMethod,
		Confidence:   9_000,
	}
	digest, err := f.keeper.AttestationDigest(f.ctx, att)
	require.NoError(t, err)
	sig, err := f.attestor.priv.Sign(digest[:])
	require.NoError(t, err)
	att.Signature = sig

	_, err = f.keeper.Claim(f.ctx, att)
	require.Error(t, err)
	require.True(t, types.ErrAttestationTooLong.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// Confidence and attestor state
// ---------------------------------------------------------------------------

func TestLowConfidenceIsRejected(t *testing.T) {
	f := setupWithAttestor(t)

	att := types.EligibilityAttestation{
		Subject:      subjectAddr("DEVNET-unsure").String(),
		Attestor:     f.attestor.reg.Address,
		Nonce:        1,
		ExpiryHeight: f.ctx.BlockHeight() + 100,
		Method:       testMethod,
		Confidence:   types.DefaultMinConfidence - 1,
	}
	digest, err := f.keeper.AttestationDigest(f.ctx, att)
	require.NoError(t, err)
	sig, err := f.attestor.priv.Sign(digest[:])
	require.NoError(t, err)
	att.Signature = sig

	_, err = f.keeper.Claim(f.ctx, att)
	require.Error(t, err)
	require.True(t, types.ErrConfidenceTooLow.Is(err), "got %v", err)
}

func TestSuspendedAttestorIsRejected(t *testing.T) {
	at := newSecp256k1Attestor(t, "suspended", testMethod, 100)
	at.reg.Enabled = false
	f := setup(t, at.reg)

	_, err := f.keeper.Claim(f.ctx, f.sign(t, at, subjectAddr("DEVNET-s1"), 1))
	require.Error(t, err)
	require.True(t, types.ErrAttestorDisabled.Is(err), "got %v", err)
}

// TestAttestorEpochCapBoundsBlastRadius: a fully compromised attestor can
// issue at most its per-epoch cap of undeserved rewards.
func TestAttestorEpochCapBoundsBlastRadius(t *testing.T) {
	at := newSecp256k1Attestor(t, "capped", testMethod, 3)
	f := setup(t, at.reg)
	f.attestor = at

	for i := uint64(1); i <= 3; i++ {
		_, err := f.keeper.Claim(f.ctx, f.sign(t, at, subjectAddr(fmt.Sprintf("DEVNET-c%d", i)), i))
		require.NoError(t, err, "claim %d within the cap was rejected", i)
	}

	_, err := f.keeper.Claim(f.ctx, f.sign(t, at, subjectAddr("DEVNET-c4"), 4))
	require.Error(t, err)
	require.True(t, types.ErrAttestorCapReached.Is(err), "got %v", err)

	// The cap resets in the next epoch.
	nextEpoch := f.ctx.WithBlockHeight(f.ctx.BlockHeight() + int64(types.DefaultEpochBlocks))
	att := types.EligibilityAttestation{
		Subject:      subjectAddr("DEVNET-c5").String(),
		Attestor:     at.reg.Address,
		Nonce:        5,
		ExpiryHeight: nextEpoch.BlockHeight() + 100,
		Method:       testMethod,
		Confidence:   9_000,
	}
	digest, err := f.keeper.AttestationDigest(nextEpoch, att)
	require.NoError(t, err)
	sig, err := at.priv.Sign(digest[:])
	require.NoError(t, err)
	att.Signature = sig

	_, err = f.keeper.Claim(nextEpoch, att)
	require.NoError(t, err, "the per-epoch cap did not reset")
}

// ---------------------------------------------------------------------------
// Programme controls
// ---------------------------------------------------------------------------

func TestDisabledProgrammeRejectsClaims(t *testing.T) {
	f := setupWithAttestor(t)

	params, err := f.keeper.GetParams(f.ctx)
	require.NoError(t, err)
	params.Enabled = false
	require.NoError(t, f.keeper.UpdateParams(f.ctx, f.gov, params))

	_, err = f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subjectAddr("DEVNET-s1"), 1))
	require.Error(t, err)
	require.True(t, types.ErrDisabled.Is(err), "got %v", err)
}

func TestUpdateParamsRequiresGovernance(t *testing.T) {
	f := setupWithAttestor(t)

	params, err := f.keeper.GetParams(f.ctx)
	require.NoError(t, err)
	params.Enabled = false

	// Not even the attestor can pause or reconfigure the programme.
	err = f.keeper.UpdateParams(f.ctx, f.attestor.reg.Address, params)
	require.Error(t, err)
	require.True(t, types.ErrInvalidAuthority.Is(err), "got %v", err)

	require.NoError(t, f.keeper.UpdateParams(f.ctx, f.gov, params))
}

// TestExhaustedProgrammeRejectsClaims: past the last funded position, claims
// fail and the sequence counter stops growing.
func TestExhaustedProgrammeRejectsClaims(t *testing.T) {
	f := setupWithAttestor(t)
	require.NoError(t, f.keeper.SetNextSequence(f.ctx, hgparams.WelcomeTier3Limit+1))

	_, err := f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subjectAddr("DEVNET-toolate"), 1))
	require.Error(t, err)
	require.True(t, types.ErrExhausted.Is(err), "got %v", err)

	next, err := f.keeper.NextSequence(f.ctx)
	require.NoError(t, err)
	require.Equal(t, hgparams.WelcomeTier3Limit+1, next,
		"an exhausted programme kept growing the sequence counter")
}

// TestLastFundedPositionStillPays: sequence 1,000,000 must pay 1 HASH.
func TestLastFundedPositionStillPays(t *testing.T) {
	f := setupWithAttestor(t)
	require.NoError(t, f.keeper.SetNextSequence(f.ctx, hgparams.WelcomeTier3Limit))

	rec, err := f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subjectAddr("DEVNET-millionth"), 1))
	require.NoError(t, err)
	require.Equal(t, hgparams.WelcomeTier3Limit, rec.Sequence)
	require.Equal(t, "1000000uhash", rec.Amount.String())
}

// TestTierTransitionAtTenThousand drives a real claim on either side of the
// first boundary through the full keeper path, not just the tier function.
func TestTierTransitionAtTenThousand(t *testing.T) {
	f := setupWithAttestor(t)

	require.NoError(t, f.keeper.SetNextSequence(f.ctx, 10_000))
	rec, err := f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subjectAddr("DEVNET-10000"), 1))
	require.NoError(t, err)
	require.Equal(t, uint64(10_000), rec.Sequence)
	require.Equal(t, "50000000uhash", rec.Amount.String(), "sequence 10,000 must still receive 50 HASH")

	rec, err = f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subjectAddr("DEVNET-10001"), 2))
	require.NoError(t, err)
	require.Equal(t, uint64(10_001), rec.Sequence)
	require.Equal(t, "5000000uhash", rec.Amount.String(), "sequence 10,001 must drop to 5 HASH")
}

// TestPoolCannotBeOverdrawn: if the pool is short, the claim fails cleanly
// rather than producing a confusing bank error.
func TestPoolCannotBeOverdrawn(t *testing.T) {
	at := newSecp256k1Attestor(t, "a", testMethod, 100)
	f := setup(t, at.reg)
	f.attestor = at

	// Drain the pool to below one tier-1 reward.
	pool := f.keeper.PoolRemaining(f.ctx)
	require.NoError(t, f.bank.SendCoinsFromModuleToAccount(f.ctx, types.ModuleName,
		subjectAddr("DEVNET-sink"), pool))
	f.bank.FundModule(types.ModuleName, sdk.NewCoins(sdk.NewInt64Coin(hgparams.BaseCoinDenom, 1)))

	_, err := f.keeper.Claim(f.ctx, f.sign(t, at, subjectAddr("DEVNET-broke"), 1))
	require.Error(t, err)
	require.True(t, types.ErrPoolInsufficient.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// Genesis
// ---------------------------------------------------------------------------

func TestExportGenesisRoundTrips(t *testing.T) {
	f := setupWithAttestor(t)

	_, err := f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subjectAddr("DEVNET-s1"), 1))
	require.NoError(t, err)
	_, err = f.keeper.Claim(f.ctx, f.sign(t, f.attestor, subjectAddr("DEVNET-s2"), 2))
	require.NoError(t, err)

	out, err := f.keeper.ExportGenesis(f.ctx)
	require.NoError(t, err)
	require.NoError(t, out.Validate())

	require.Equal(t, uint64(3), out.NextSequence)
	require.Len(t, out.Claims, 2)
	require.Len(t, out.UsedNonces, 2)
	require.Equal(t, uint64(2), out.ClaimsPaid)
	require.Equal(t, "100000000uhash", out.TotalPaid.Amount.String())
}

// TestGenesisRejectsRewrittenHistory: a restored chain must not be able to
// claim that an early position was paid more than the schedule allows.
func TestGenesisRejectsRewrittenHistory(t *testing.T) {
	gs := types.DefaultGenesis()
	gs.NextSequence = 2
	gs.Claims = []types.ClaimRecord{{
		Subject:  subjectAddr("DEVNET-liar").String(),
		Sequence: 1,
		Amount:   sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(5_000))),
		Height:   1,
	}}

	err := gs.Validate()
	require.Error(t, err)
	require.Contains(t, err.Error(), "the schedule pays")
}

func TestGenesisRejectsDuplicateSubjects(t *testing.T) {
	subject := subjectAddr("DEVNET-dup").String()
	gs := types.DefaultGenesis()
	gs.NextSequence = 3
	gs.Claims = []types.ClaimRecord{
		{Subject: subject, Sequence: 1, Amount: types.TierAmount(1), Height: 1},
		{Subject: subject, Sequence: 2, Amount: types.TierAmount(2), Height: 2},
	}

	err := gs.Validate()
	require.Error(t, err)
	require.True(t, types.ErrAlreadyClaimed.Is(err), "got %v", err)
}

func TestGenesisRejectsSequenceZero(t *testing.T) {
	gs := types.DefaultGenesis()
	gs.NextSequence = 0
	require.Error(t, gs.Validate())
}

// ---------------------------------------------------------------------------
// Attestor registration validation
// ---------------------------------------------------------------------------

// TestAttestorAddressMustMatchPubkey blocks registering somebody else's
// address against your own key.
func TestAttestorAddressMustMatchPubkey(t *testing.T) {
	at := newSecp256k1Attestor(t, "mismatched", testMethod, 100)
	at.reg.Address = subjectAddr("DEVNET-someone-else").String()

	err := at.reg.Validate()
	require.Error(t, err)
	require.True(t, types.ErrInvalidAttestor.Is(err), "got %v", err)
	require.Contains(t, err.Error(), "derived from pubkey")
}

// TestUncappedAttestorIsRejected: an attestor with no cap has unbounded blast
// radius if compromised.
func TestUncappedAttestorIsRejected(t *testing.T) {
	at := newSecp256k1Attestor(t, "uncapped", testMethod, 0)
	err := at.reg.Validate()
	require.Error(t, err)
	require.Contains(t, err.Error(), "blast radius")
}

func TestAttestorRejectsWrongPubkeyLength(t *testing.T) {
	at := newSecp256k1Attestor(t, "short", testMethod, 100)
	at.reg.Pubkey = at.reg.Pubkey[:16]
	require.Error(t, at.reg.Validate())
}

func TestDuplicateAttestorRegistrationIsRejected(t *testing.T) {
	at := newSecp256k1Attestor(t, "dup", testMethod, 100)
	p := types.DefaultParams()
	p.Attestors = []types.Attestor{at.reg, at.reg}
	require.Error(t, p.Validate())
}

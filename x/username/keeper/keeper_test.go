package keeper_test

import (
	"context"
	"os"
	"testing"

	"github.com/stretchr/testify/require"

	"cosmossdk.io/log"
	storetypes "cosmossdk.io/store/types"

	"github.com/cosmos/cosmos-sdk/codec"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	"github.com/cosmos/cosmos-sdk/runtime"
	"github.com/cosmos/cosmos-sdk/testutil"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	feeroutertypes "github.com/hashgram/hashgram/x/feerouter/types"
	"github.com/hashgram/hashgram/x/username/keeper"
	"github.com/hashgram/hashgram/x/username/types"
)

func TestMain(m *testing.M) {
	hgparams.SetSDKConfig()
	os.Exit(m.Run())
}

// feeRecorder is a fake fee router that records what was charged.
//
// A recording fake rather than a mock: these tests care that the fee is
// charged exactly once and only on success, which is a property of the
// sequence of calls rather than of any single one.
type feeRecorder struct {
	charged []sdk.Coins
	kinds   []feeroutertypes.ServiceKind
	fail    bool
}

func (f *feeRecorder) CollectServiceFee(_ context.Context, _ sdk.AccAddress, amount sdk.Coins, kind feeroutertypes.ServiceKind) error {
	if f.fail {
		return types.ErrInvalidParams.Wrap("simulated insufficient funds")
	}
	f.charged = append(f.charged, amount)
	f.kinds = append(f.kinds, kind)
	return nil
}

func (f *feeRecorder) total() int { return len(f.charged) }

type fixture struct {
	ctx    sdk.Context
	keeper keeper.Keeper
	fees   *feeRecorder
	gov    string
}

func setup(t *testing.T) *fixture {
	t.Helper()

	key := storetypes.NewKVStoreKey(types.StoreKey)
	testCtx := testutil.DefaultContextWithDB(t, key, storetypes.NewTransientStoreKey("transient_test"))
	ctx := testCtx.Ctx.WithBlockHeight(1_000)

	cdc := codec.NewProtoCodec(codectypes.NewInterfaceRegistry())
	fees := &feeRecorder{}
	gov := authtypes.NewModuleAddress("gov").String()

	k := keeper.NewKeeper(cdc, runtime.NewKVStoreService(key), fees, gov, log.NewNopLogger())
	require.NoError(t, k.InitGenesis(ctx, *types.DefaultGenesis()))

	return &fixture{ctx: ctx, keeper: k, fees: fees, gov: gov}
}

func addr(label string) sdk.AccAddress {
	b := make([]byte, 20)
	copy(b, label)
	return sdk.AccAddress(b)
}

var (
	alice = addr("DEVNET-alice")
	bob   = addr("DEVNET-bob")
)

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

func TestRegisterStoresNormalisedName(t *testing.T) {
	f := setup(t)

	// Registered with a sigil and mixed case; stored normalised.
	reg, err := f.keeper.Register(f.ctx, alice, "@Alice")
	require.NoError(t, err)

	require.Equal(t, "alice", reg.Name)
	require.Equal(t, alice.String(), reg.Owner)
	require.Equal(t, types.Skeleton("alice"), reg.Skeleton)
	require.True(t, reg.Transferable)
	require.Equal(t, f.ctx.BlockHeight()+types.DefaultRegistrationPeriodBlocks, reg.ExpiryHeight)

	// And resolvable by any equivalent form.
	for _, form := range []string{"alice", "Alice", "@alice", "@ALICE"} {
		found, ok, err := f.keeper.Lookup(f.ctx, form)
		require.NoError(t, err)
		require.True(t, ok, "%q did not resolve", form)
		require.Equal(t, alice.String(), found.Owner)
	}

	require.Equal(t, 1, f.fees.total())
	require.Equal(t, feeroutertypes.SERVICE_KIND_USERNAME, f.fees.kinds[0])
}

func TestRegisterRejectsTakenName(t *testing.T) {
	f := setup(t)

	_, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)

	_, err = f.keeper.Register(f.ctx, bob, "alice")
	require.Error(t, err)
	require.True(t, types.ErrNameTaken.Is(err), "got %v", err)

	// The failed registration charged nothing.
	require.Equal(t, 1, f.fees.total(), "a rejected registration charged a fee")
}

// TestRegisterRejectsConfusableName is the homograph defence at the keeper
// layer: "he11o" folds to the same skeleton as the registered "hello".
func TestRegisterRejectsConfusableName(t *testing.T) {
	f := setup(t)

	_, err := f.keeper.Register(f.ctx, alice, "hello")
	require.NoError(t, err)

	_, err = f.keeper.Register(f.ctx, bob, "he11o")
	require.Error(t, err)
	require.True(t, types.ErrConfusable.Is(err), "got %v", err)
	require.Contains(t, err.Error(), "hello",
		"the error should name the registration the request looks like")

	require.Equal(t, 1, f.fees.total())
}

func TestRegisterRejectsReservedName(t *testing.T) {
	f := setup(t)

	for _, name := range []string{"admin", "hashgram", "support", "adm1n"} {
		_, err := f.keeper.Register(f.ctx, alice, name)
		require.Error(t, err, "%q was registrable", name)
		require.True(t, types.ErrNameReserved.Is(err), "%q: got %v", name, err)
	}
	require.Equal(t, 0, f.fees.total())
}

// TestRegisterChargesOnlyOnSuccess: a user must not pay for a rejected
// registration.
func TestRegisterChargesOnlyOnSuccess(t *testing.T) {
	f := setup(t)

	// A rejected name.
	_, err := f.keeper.Register(f.ctx, alice, "ab") // too short
	require.Error(t, err)
	require.Equal(t, 0, f.fees.total())

	// A fee-collection failure must not leave a registration behind.
	f.fees.fail = true
	_, err = f.keeper.Register(f.ctx, alice, "alice")
	require.Error(t, err)
	f.fees.fail = false

	_, ok, err := f.keeper.GetRegistration(f.ctx, "alice")
	require.NoError(t, err)
	require.False(t, ok, "a registration survived a failed fee collection")
}

func TestAvailabilityExplainsWhy(t *testing.T) {
	f := setup(t)
	_, err := f.keeper.Register(f.ctx, alice, "hello")
	require.NoError(t, err)

	for _, tc := range []struct {
		name       string
		wantReason string
		wantConf   string
	}{
		{"hello", types.ReasonTaken, "hello"},
		{"he11o", types.ReasonConfusableWith, "hello"},
		{"admin", types.ReasonReserved, ""},
		{"ab", types.ReasonTooShort, ""},
		{"ali ce", types.ReasonInvalid, ""},
		{"\u043f\u0440\u0438\u0432", types.ReasonNonASCIINotAllowed, ""},
	} {
		a, err := f.keeper.CheckAvailability(f.ctx, tc.name)
		require.NoError(t, err)
		require.False(t, a.Available, "%q reported available", tc.name)
		require.Equal(t, tc.wantReason, a.Reason, "%q", tc.name)
		if tc.wantConf != "" {
			require.Equal(t, tc.wantConf, a.ConflictingName, "%q", tc.name)
		}
	}

	a, err := f.keeper.CheckAvailability(f.ctx, "carol")
	require.NoError(t, err)
	require.True(t, a.Available)
	require.Equal(t, "carol", a.Normalized)
}

// ---------------------------------------------------------------------------
// Expiry and renewal
// ---------------------------------------------------------------------------

func TestLookupStopsResolvingAfterGracePeriod(t *testing.T) {
	f := setup(t)
	reg, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)

	// Still resolves at expiry and within the grace period: the owner has an
	// exclusive right to renew and the name should keep working meanwhile.
	atExpiry := f.ctx.WithBlockHeight(reg.ExpiryHeight)
	_, ok, err := f.keeper.Lookup(atExpiry, "alice")
	require.NoError(t, err)
	require.True(t, ok, "the name stopped resolving at expiry, before the grace period")

	inGrace := f.ctx.WithBlockHeight(reg.ExpiryHeight + types.DefaultGracePeriodBlocks)
	_, ok, err = f.keeper.Lookup(inGrace, "alice")
	require.NoError(t, err)
	require.True(t, ok, "the name stopped resolving inside the grace period")

	// Past the grace period it stops resolving: pointing at the former owner
	// would be actively misleading.
	after := f.ctx.WithBlockHeight(reg.ExpiryHeight + types.DefaultGracePeriodBlocks + 1)
	_, ok, err = f.keeper.Lookup(after, "alice")
	require.NoError(t, err)
	require.False(t, ok)
}

// TestGracePeriodProtectsTheOwnerFromSniping
func TestGracePeriodProtectsTheOwnerFromSniping(t *testing.T) {
	f := setup(t)
	reg, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)

	inGrace := f.ctx.WithBlockHeight(reg.ExpiryHeight + 1)

	// Somebody else cannot take it during the grace period.
	_, err = f.keeper.Register(inGrace, bob, "alice")
	require.Error(t, err)
	require.True(t, types.ErrNameTaken.Is(err), "got %v", err)

	// The owner can still renew.
	newExpiry, err := f.keeper.Renew(inGrace, alice, "alice")
	require.NoError(t, err)
	require.Greater(t, newExpiry, reg.ExpiryHeight)
}

func TestNameIsClaimableAfterGracePeriod(t *testing.T) {
	f := setup(t)
	reg, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)

	after := f.ctx.WithBlockHeight(reg.ExpiryHeight + types.DefaultGracePeriodBlocks + 1)

	newReg, err := f.keeper.Register(after, bob, "alice")
	require.NoError(t, err)
	require.Equal(t, bob.String(), newReg.Owner)

	// The old owner's reverse lookup no longer lists it: leaving a stale
	// index entry would make the previous owner appear to still hold it.
	aliceNames, err := f.keeper.OwnedNames(after, alice.String())
	require.NoError(t, err)
	require.Empty(t, aliceNames, "the previous owner still lists the reassigned name")

	bobNames, err := f.keeper.OwnedNames(after, bob.String())
	require.NoError(t, err)
	require.Len(t, bobNames, 1)
}

// TestRenewingEarlyDoesNotForfeitTime
func TestRenewingEarlyDoesNotForfeitTime(t *testing.T) {
	f := setup(t)
	reg, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)

	// Renew immediately, long before expiry.
	newExpiry, err := f.keeper.Renew(f.ctx, alice, "alice")
	require.NoError(t, err)
	require.Equal(t, reg.ExpiryHeight+types.DefaultRegistrationPeriodBlocks, newExpiry,
		"renewing early forfeited the remaining term")
}

func TestOnlyOwnerCanRenew(t *testing.T) {
	f := setup(t)
	_, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)

	_, err = f.keeper.Renew(f.ctx, bob, "alice")
	require.Error(t, err)
	require.True(t, types.ErrNotOwner.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// Transfer
// ---------------------------------------------------------------------------

func TestTransferMovesOwnershipAndIndexes(t *testing.T) {
	f := setup(t)
	_, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)

	require.NoError(t, f.keeper.Transfer(f.ctx, alice, "alice", bob))

	reg, ok, err := f.keeper.GetRegistration(f.ctx, "alice")
	require.NoError(t, err)
	require.True(t, ok)
	require.Equal(t, bob.String(), reg.Owner)

	aliceNames, err := f.keeper.OwnedNames(f.ctx, alice.String())
	require.NoError(t, err)
	require.Empty(t, aliceNames, "the old owner index entry was not removed")

	bobNames, err := f.keeper.OwnedNames(f.ctx, bob.String())
	require.NoError(t, err)
	require.Len(t, bobNames, 1)
	require.Equal(t, "alice", bobNames[0].Name)
}

func TestOnlyOwnerCanTransfer(t *testing.T) {
	f := setup(t)
	_, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)

	err = f.keeper.Transfer(f.ctx, bob, "alice", bob)
	require.Error(t, err)
	require.True(t, types.ErrNotOwner.Is(err), "got %v", err)
}

// TestLockedNameCannotBeTransferred is the key-compromise defence: an
// attacker with the owner key still cannot move a locked name.
func TestLockedNameCannotBeTransferred(t *testing.T) {
	f := setup(t)
	_, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)

	require.NoError(t, f.keeper.SetTransferable(f.ctx, alice, "alice", false))

	err = f.keeper.Transfer(f.ctx, alice, "alice", bob)
	require.Error(t, err)
	require.True(t, types.ErrNotTransferable.Is(err), "got %v", err)

	// Unlocking is itself a transaction the owner can see on chain.
	require.NoError(t, f.keeper.SetTransferable(f.ctx, alice, "alice", true))
	require.NoError(t, f.keeper.Transfer(f.ctx, alice, "alice", bob))
}

// ---------------------------------------------------------------------------
// Release
// ---------------------------------------------------------------------------

func TestReleaseReturnsNameToCirculation(t *testing.T) {
	f := setup(t)
	_, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)

	require.NoError(t, f.keeper.Release(f.ctx, alice, "alice"))

	_, ok, err := f.keeper.GetRegistration(f.ctx, "alice")
	require.NoError(t, err)
	require.False(t, ok)

	// Including the skeleton, so a confusable variant is registrable again.
	a, err := f.keeper.CheckAvailability(f.ctx, "he11o")
	require.NoError(t, err)
	require.True(t, a.Available)

	newReg, err := f.keeper.Register(f.ctx, bob, "alice")
	require.NoError(t, err)
	require.Equal(t, bob.String(), newReg.Owner)
}

func TestOnlyOwnerCanRelease(t *testing.T) {
	f := setup(t)
	_, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)

	err = f.keeper.Release(f.ctx, bob, "alice")
	require.Error(t, err)
	require.True(t, types.ErrNotOwner.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// Governance and genesis
// ---------------------------------------------------------------------------

func TestUpdateParamsRequiresGovernance(t *testing.T) {
	f := setup(t)

	next := types.DefaultParams()
	next.AllowNonAscii = true

	err := f.keeper.UpdateParams(f.ctx, alice.String(), next)
	require.Error(t, err)
	require.True(t, types.ErrInvalidAuthority.Is(err), "got %v", err)

	require.NoError(t, f.keeper.UpdateParams(f.ctx, f.gov, next))
}

func TestExportGenesisRoundTrips(t *testing.T) {
	f := setup(t)
	_, err := f.keeper.Register(f.ctx, alice, "alice")
	require.NoError(t, err)
	_, err = f.keeper.Register(f.ctx, bob, "bob")
	require.NoError(t, err)

	out, err := f.keeper.ExportGenesis(f.ctx)
	require.NoError(t, err)
	require.Len(t, out.Registrations, 2)
	require.NoError(t, out.Validate())
}

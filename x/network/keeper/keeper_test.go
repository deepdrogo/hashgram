package keeper_test

import (
	"errors"
	"testing"

	"github.com/stretchr/testify/require"

	"cosmossdk.io/log"
	storetypes "cosmossdk.io/store/types"

	"github.com/cosmos/cosmos-sdk/codec"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	"github.com/cosmos/cosmos-sdk/runtime"
	"github.com/cosmos/cosmos-sdk/testutil"
	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/network/keeper"
	"github.com/hashgram/hashgram/x/network/types"
)

// fixture is a minimal x/network keeper over an in-memory store.
type fixture struct {
	ctx    sdk.Context
	keeper keeper.Keeper
}

func setup(t *testing.T, chainID string) *fixture {
	t.Helper()

	key := storetypes.NewKVStoreKey(types.StoreKey)
	testCtx := testutil.DefaultContextWithDB(t, key, storetypes.NewTransientStoreKey("transient_test"))
	ctx := testCtx.Ctx.WithChainID(chainID)

	cdc := codec.NewProtoCodec(codectypes.NewInterfaceRegistry())
	k := keeper.NewKeeper(cdc, runtime.NewKVStoreService(key), log.NewNopLogger())

	return &fixture{ctx: ctx, keeper: k}
}

func mainnetGenesisHash() string {
	return "9f2c1e5b4a3d8f7061524334455667788990aabbccddeeff00112233445566aa"
}

// ---------------------------------------------------------------------------
// Genesis
// ---------------------------------------------------------------------------

func TestInitGenesisMainnet(t *testing.T) {
	f := setup(t, hgparams.ChainIDMainnet)

	gs := types.MainnetGenesis()
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *gs))

	info, err := f.keeper.NetworkInfo(f.ctx)
	require.NoError(t, err)
	require.Equal(t, "Hashgram Mainnet", info.NetworkName)
	require.Equal(t, "hashgram-mainnet", info.NetworkId)
	require.Equal(t, "hashgram-1", info.ChainId)
	require.Equal(t, "HGM1", string(info.NetworkMagic))
	require.Equal(t, uint32(1), info.ProtocolMajorVersion)

	policy, err := f.keeper.ForkIsolation(f.ctx)
	require.NoError(t, err)
	require.True(t, policy.RequireNetworkIdMatch)
	require.True(t, policy.RequireChainIdMatch)
	require.True(t, policy.RequireNetworkMagicMatch)
	require.True(t, policy.RequireGenesisHashMatch)
	require.True(t, policy.RequireProtocolMajorMatch)
}

// TestDefaultGenesisIsDevnetNotMainnet: `hashgramd init` must not accidentally
// produce a chain that claims to be Mainnet.
func TestDefaultGenesisIsDevnetNotMainnet(t *testing.T) {
	gs := types.DefaultGenesis()
	require.Equal(t, hgparams.NetworkIDDevnet, gs.Info.NetworkId)
	require.NotEqual(t, hgparams.NetworkIDMainnet, gs.Info.NetworkId)
	require.Equal(t, "HGD1", string(gs.Info.NetworkMagic))
	require.Contains(t, gs.Info.NetworkName, "DEVNET ONLY")
}

// TestInitGenesisRejectsChainIDMismatch is the check that turns "somebody
// handed me a genesis.json" into a verifiable claim: a genesis file whose
// x/network chain_id has been edited to say hashgram-1 while CometBFT was
// started with a different chain-id must abort chain start.
func TestInitGenesisRejectsChainIDMismatch(t *testing.T) {
	f := setup(t, "attacker-chain-7")

	gs := types.MainnetGenesis()
	err := f.keeper.InitGenesis(f.ctx, *gs)

	require.Error(t, err)
	require.True(t, errors.Is(err, types.ErrChainIDMismatch), "got %v", err)
	require.Contains(t, err.Error(), "hashgram-1")
	require.Contains(t, err.Error(), "attacker-chain-7")
}

// TestInitGenesisIsNotRepeatable: a second InitGenesis must fail rather than
// silently rewrite the network's identity.
func TestInitGenesisIsNotRepeatable(t *testing.T) {
	f := setup(t, hgparams.ChainIDMainnet)

	gs := types.MainnetGenesis()
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *gs))

	err := f.keeper.InitGenesis(f.ctx, *gs)
	require.Error(t, err)
	require.True(t, errors.Is(err, types.ErrNetworkInfoImmutable), "got %v", err)
}

func TestExportGenesisRoundTrips(t *testing.T) {
	f := setup(t, hgparams.ChainIDMainnet)
	in := types.MainnetGenesis()
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *in))

	out, err := f.keeper.ExportGenesis(f.ctx)
	require.NoError(t, err)
	require.Equal(t, *in, *out)
}

// TestMissingForkIsolationFailsClosed: if the policy is absent from state
// (corruption, or a migration gap), the keeper must report the strictest
// policy, not the most permissive one.
func TestMissingForkIsolationFailsClosed(t *testing.T) {
	f := setup(t, hgparams.ChainIDMainnet)

	policy, err := f.keeper.ForkIsolation(f.ctx)
	require.NoError(t, err)
	require.Equal(t, types.StrictForkIsolation(), policy)
}

func TestNetworkInfoNotSet(t *testing.T) {
	f := setup(t, hgparams.ChainIDMainnet)

	_, err := f.keeper.NetworkInfo(f.ctx)
	require.Error(t, err)
	require.True(t, errors.Is(err, types.ErrNetworkInfoNotSet), "got %v", err)
}

// ---------------------------------------------------------------------------
// Genesis validation
// ---------------------------------------------------------------------------

// TestGenesisRejectsPartialMainnetClaims blocks publishing a genesis that
// says network_id "hashgram-mainnet" while carrying a foreign chain_id,
// magic, or protocol version.
func TestGenesisRejectsPartialMainnetClaims(t *testing.T) {
	base := func() types.NetworkInfo {
		gs := types.MainnetGenesis()
		return gs.Info
	}

	tests := map[string]func(*types.NetworkInfo){
		"foreign chain_id":   func(n *types.NetworkInfo) { n.ChainId = "hashgram-999" },
		"foreign magic":      func(n *types.NetworkInfo) { n.NetworkMagic = []byte("XXXX") },
		"foreign protocol":   func(n *types.NetworkInfo) { n.ProtocolMajorVersion = 7 },
		"empty network name": func(n *types.NetworkInfo) { n.NetworkName = "" },
		"short magic":        func(n *types.NetworkInfo) { n.NetworkMagic = []byte("HG") },
		"zero magic":         func(n *types.NetworkInfo) { n.NetworkMagic = []byte{0, 0, 0, 0} },
		"zero protocol":      func(n *types.NetworkInfo) { n.ProtocolMajorVersion = 0 },
		"empty network_id":   func(n *types.NetworkInfo) { n.NetworkId = "" },
		"empty chain_id":     func(n *types.NetworkInfo) { n.ChainId = "" },
	}

	for name, mutate := range tests {
		t.Run(name, func(t *testing.T) {
			info := base()
			mutate(&info)
			gs := types.GenesisState{Info: info, ForkIsolation: types.StrictForkIsolation()}
			require.Error(t, gs.Validate(), "invalid genesis was accepted")
		})
	}
}

// TestMainnetCannotWeakenForkIsolation: shipping a "Mainnet" genesis with a
// disabled check would be an effective way to merge a fork into Mainnet.
func TestMainnetCannotWeakenForkIsolation(t *testing.T) {
	for name, mutate := range map[string]func(*types.ForkIsolationPolicy){
		"network id":     func(p *types.ForkIsolationPolicy) { p.RequireNetworkIdMatch = false },
		"chain id":       func(p *types.ForkIsolationPolicy) { p.RequireChainIdMatch = false },
		"network magic":  func(p *types.ForkIsolationPolicy) { p.RequireNetworkMagicMatch = false },
		"genesis hash":   func(p *types.ForkIsolationPolicy) { p.RequireGenesisHashMatch = false },
		"protocol major": func(p *types.ForkIsolationPolicy) { p.RequireProtocolMajorMatch = false },
	} {
		t.Run(name, func(t *testing.T) {
			gs := types.MainnetGenesis()
			mutate(&gs.ForkIsolation)

			err := gs.Validate()
			require.Error(t, err)
			require.True(t, errors.Is(err, types.ErrForkIsolationWeakened), "got %v", err)
		})
	}
}

// TestDevnetMayWeakenForkIsolation: local development needs the freedom to
// relax checks; Mainnet does not.
func TestDevnetMayWeakenForkIsolation(t *testing.T) {
	gs := types.DefaultGenesis()
	gs.ForkIsolation.RequireGenesisHashMatch = false
	require.NoError(t, gs.Validate())
}

// ---------------------------------------------------------------------------
// Pinned genesis hash
// ---------------------------------------------------------------------------

func TestPinnedGenesisHash(t *testing.T) {
	f := setup(t, hgparams.ChainIDMainnet)

	require.Equal(t, "", f.keeper.PinnedGenesisHash())

	require.NoError(t, f.keeper.SetPinnedGenesisHash(mainnetGenesisHash()))
	require.Equal(t, mainnetGenesisHash(), f.keeper.PinnedGenesisHash())

	// Malformed values must be rejected, not silently ignored: an operator
	// who typed a broken hash intended to pin something.
	for _, bad := range []string{"deadbeef", "not-hex", mainnetGenesisHash() + "00"} {
		require.Error(t, f.keeper.SetPinnedGenesisHash(bad), "accepted malformed hash %q", bad)
	}
}

// ---------------------------------------------------------------------------
// Peer verification (§89 foreign node test, at the state-machine layer)
// ---------------------------------------------------------------------------

// TestForeignForkIsRejectedUsingOnChainIdentity wires the on-chain identity
// into the peer check and confirms a fork with a different genesis is
// rejected.
func TestForeignForkIsRejectedUsingOnChainIdentity(t *testing.T) {
	f := setup(t, hgparams.ChainIDMainnet)
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *types.MainnetGenesis()))
	require.NoError(t, f.keeper.SetPinnedGenesisHash(mainnetGenesisHash()))

	local, err := f.keeper.Identity(f.ctx)
	require.NoError(t, err)
	require.True(t, local.IsMainnet())

	// A fork: same software, same chain-id, different genesis file.
	fork := local
	fork.GenesisHash = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"

	err = local.VerifyPeer(fork)
	require.Error(t, err, "REJECTED expected but the fork was accepted")
	require.True(t, errors.Is(err, hgparams.ErrForeignNetwork))
	require.Contains(t, err.Error(), "genesis_hash")

	// A genuine peer is accepted, so the check is not vacuously strict.
	require.NoError(t, local.VerifyPeer(local))
}

// TestNodeWithoutPinnedGenesisRefusesPeers: a node that has not pinned its
// genesis must refuse all Hashgram peers rather than accept all of them.
func TestNodeWithoutPinnedGenesisRefusesPeers(t *testing.T) {
	f := setup(t, hgparams.ChainIDMainnet)
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *types.MainnetGenesis()))

	local, err := f.keeper.Identity(f.ctx)
	require.NoError(t, err)
	require.Equal(t, "", local.GenesisHash)

	remote := local
	remote.GenesisHash = mainnetGenesisHash()

	err = local.VerifyPeer(remote)
	require.Error(t, err)
	require.Contains(t, err.Error(), "not pinned")
}

// ---------------------------------------------------------------------------
// Signing domains
// ---------------------------------------------------------------------------

func TestSigningDomainFromChainState(t *testing.T) {
	f := setup(t, hgparams.ChainIDMainnet)
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *types.MainnetGenesis()))

	domain, err := f.keeper.SigningDomain(f.ctx, hgparams.PurposeServiceReceipt)
	require.NoError(t, err)
	require.Equal(t, "hashgram/v1/hashgram-mainnet/service-receipt", domain)
}

// TestSigningDigestIsDomainSeparated: one keeper, many purposes, no collisions.
func TestSigningDigestIsDomainSeparated(t *testing.T) {
	f := setup(t, hgparams.ChainIDMainnet)
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *types.MainnetGenesis()))

	payload := []byte("identical payload for every purpose")
	seen := map[[32]byte]hgparams.SigningPurpose{}

	for _, p := range types.SigningPurposes() {
		d, err := f.keeper.SigningDigest(f.ctx, p, payload)
		require.NoError(t, err)
		prev, dup := seen[d]
		require.False(t, dup, "purposes %q and %q collide", p, prev)
		seen[d] = p
	}
	require.Len(t, seen, len(types.SigningPurposes()))
}

func TestParseSigningPurpose(t *testing.T) {
	p, err := types.ParseSigningPurpose("social-event")
	require.NoError(t, err)
	require.Equal(t, hgparams.PurposeSocialEvent, p)

	_, err = types.ParseSigningPurpose("social-events")
	require.Error(t, err)
	require.True(t, errors.Is(err, types.ErrUnknownSigningPurpose))
	// The error must list the valid options: a caller who typo'd needs to see
	// the closed set, not just "invalid".
	require.Contains(t, err.Error(), "service-receipt")
}

// TestDevnetAndMainnetDigestsDiffer is the replay-protection property from
// §10: a signed event from a devnet or a fork must not verify on Mainnet.
func TestDevnetAndMainnetDigestsDiffer(t *testing.T) {
	mainnet := setup(t, hgparams.ChainIDMainnet)
	require.NoError(t, mainnet.keeper.InitGenesis(mainnet.ctx, *types.MainnetGenesis()))

	devnet := setup(t, hgparams.ChainIDDevnet)
	require.NoError(t, devnet.keeper.InitGenesis(devnet.ctx, *types.DefaultGenesis()))

	payload := []byte("post: hello world")

	m, err := mainnet.keeper.SigningDigest(mainnet.ctx, hgparams.PurposeSocialEvent, payload)
	require.NoError(t, err)
	d, err := devnet.keeper.SigningDigest(devnet.ctx, hgparams.PurposeSocialEvent, payload)
	require.NoError(t, err)

	require.NotEqual(t, m, d, "devnet and mainnet digests are identical; events would replay across networks")
}

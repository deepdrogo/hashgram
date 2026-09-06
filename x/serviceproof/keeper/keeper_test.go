package keeper_test

import (
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
	"github.com/hashgram/hashgram/x/serviceproof/keeper"
	"github.com/hashgram/hashgram/x/serviceproof/types"
)

func TestMain(m *testing.M) {
	hgparams.SetSDKConfig()
	os.Exit(m.Run())
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

type fixture struct {
	ctx     sdk.Context
	keeper  keeper.Keeper
	network networkkeeper.Keeper
	bank    *hgbank.Bank
	gov     string
}

func setup(t *testing.T) *fixture {
	t.Helper()

	svcKey := storetypes.NewKVStoreKey(types.StoreKey)
	netKey := storetypes.NewKVStoreKey(networktypes.StoreKey)

	testCtx := testutil.DefaultContextWithDB(t, svcKey, storetypes.NewTransientStoreKey("transient_test"))
	cms := testCtx.CMS
	cms.MountStoreWithDB(netKey, storetypes.StoreTypeIAVL, testCtx.DB)
	require.NoError(t, cms.LoadLatestVersion())

	ctx := testCtx.Ctx.
		WithMultiStore(cms).
		WithChainID(hgparams.ChainIDMainnet).
		WithBlockHeight(100)

	cdc := codec.NewProtoCodec(codectypes.NewInterfaceRegistry())
	bank := hgbank.NewBank()
	accounts := hgbank.NewAccounts(types.ModuleName, types.BondPoolName, authtypes.FeeCollectorName)
	gov := authtypes.NewModuleAddress("gov").String()

	nk := networkkeeper.NewKeeper(cdc, runtime.NewKVStoreService(netKey), log.NewNopLogger())
	require.NoError(t, nk.InitGenesis(ctx, *networktypes.MainnetGenesis()))

	k := keeper.NewKeeper(cdc, runtime.NewKVStoreService(svcKey),
		accounts, bank, nk, gov, log.NewNopLogger())
	require.NoError(t, k.InitGenesis(ctx, *types.DefaultGenesis()))

	// Fund the reserve as Mainnet genesis does.
	bank.FundModule(types.ModuleName, types.InitialReserve())

	return &fixture{ctx: ctx, keeper: k, network: nk, bank: bank, gov: gov}
}

// node is a DEVNET-only provider identity.
type node struct {
	operator sdk.AccAddress
	reward   sdk.AccAddress
	priv     cryptotypes.PrivKey
}

func newNode(label string) node {
	op := make([]byte, 20)
	copy(op, "DEVNET-op-"+label)
	rw := make([]byte, 20)
	copy(rw, "DEVNET-rw-"+label)
	return node{
		operator: sdk.AccAddress(op),
		reward:   sdk.AccAddress(rw),
		priv:     ed25519.GenPrivKey(),
	}
}

// client is a DEVNET-only client identity that signs receipts.
type client struct {
	priv cryptotypes.PrivKey
}

func newClient() client { return client{priv: secp256k1.GenPrivKey()} }

func (c client) address() sdk.AccAddress { return sdk.AccAddress(c.priv.PubKey().Address()) }

func hash(n int64) sdk.Coins {
	return sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(n)))
}

func (f *fixture) params(t *testing.T) types.Params {
	t.Helper()
	p, err := f.keeper.GetParams(f.ctx)
	require.NoError(t, err)
	return p
}

// register registers a provider with the given roles.
func (f *fixture) register(t *testing.T, n node, roles ...types.ServiceRole) {
	t.Helper()

	bond := f.params(t).MinBond
	f.bank.Fund(n.operator, bond)

	require.NoError(t, f.keeper.RegisterProvider(f.ctx, &types.MsgRegisterProvider{
		Operator:             n.operator.String(),
		RewardAddress:        n.reward.String(),
		NodePubkey:           n.priv.PubKey().Bytes(),
		NodeKeyType:          types.KEY_TYPE_ED25519,
		Roles:                roles,
		Bond:                 bond,
		DeclaredStorageBytes: 1 << 40, // 1 TiB
		DeclaredBandwidthBps: 100_000_000,
		Moniker:              "devnet-node",
	}))
}

// signReceipt produces a valid client-signed receipt.
func (f *fixture) signReceipt(t *testing.T, c client, provider sdk.AccAddress, role types.ServiceRole, epoch, nonce, units uint64) types.ServiceReceipt {
	t.Helper()

	r := types.ServiceReceipt{
		Provider:      provider.String(),
		Role:          role,
		ClientPubkey:  c.priv.PubKey().Bytes(),
		ClientKeyType: types.KEY_TYPE_SECP256K1,
		Epoch:         epoch,
		Nonce:         nonce,
		Units:         units,
		ExpiryHeight:  f.ctx.BlockHeight() + 1_000,
	}

	digest, err := f.network.SigningDigest(f.ctx, hgparams.PurposeServiceReceipt,
		types.CanonicalReceiptBytes(r))
	require.NoError(t, err)

	sig, err := c.priv.Sign(digest[:])
	require.NoError(t, err)
	r.ClientSignature = sig

	return r
}

const oneGiB = uint64(1024 * 1024 * 1024)

// ---------------------------------------------------------------------------
// Registration and bonding
// ---------------------------------------------------------------------------

func TestRegisterProviderEscrowsBond(t *testing.T) {
	f := setup(t)
	n := newNode("a")

	f.register(t, n, types.SERVICE_ROLE_RELAY)

	p, err := f.keeper.GetProvider(f.ctx, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, n.reward.String(), p.RewardAddress)
	require.True(t, p.HasRole(types.SERVICE_ROLE_RELAY))
	require.True(t, p.IsActive())

	// The bond left the operator and sits in the dedicated bond pool, not in
	// the reward reserve.
	require.True(t, f.bank.GetAllBalances(f.ctx, n.operator).IsZero())
	require.Equal(t, f.params(t).MinBond.String(), f.keeper.BondedTotal(f.ctx).String())
	require.Equal(t, types.InitialReserve().String(), f.keeper.ReserveRemaining(f.ctx).String(),
		"the bond was mixed into the reward reserve")
}

func TestRegisterProviderRejectsInsufficientBond(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	f.bank.Fund(n.operator, hash(1))

	err := f.keeper.RegisterProvider(f.ctx, &types.MsgRegisterProvider{
		Operator:    n.operator.String(),
		NodePubkey:  n.priv.PubKey().Bytes(),
		NodeKeyType: types.KEY_TYPE_ED25519,
		Roles:       []types.ServiceRole{types.SERVICE_ROLE_RELAY},
		Bond:        hash(1),
	})
	require.Error(t, err)
	require.True(t, types.ErrInsufficientBond.Is(err), "got %v", err)
}

func TestRegisterProviderRejectsDuplicates(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	f.bank.Fund(n.operator, f.params(t).MinBond)
	err := f.keeper.RegisterProvider(f.ctx, &types.MsgRegisterProvider{
		Operator:    n.operator.String(),
		NodePubkey:  n.priv.PubKey().Bytes(),
		NodeKeyType: types.KEY_TYPE_ED25519,
		Roles:       []types.ServiceRole{types.SERVICE_ROLE_RELAY},
		Bond:        f.params(t).MinBond,
	})
	require.Error(t, err)
	require.True(t, types.ErrProviderExists.Is(err), "got %v", err)
}

// TestBondCannotBeWithdrawnEarly: the unbonding delay must outlast the window
// in which fraud could still be discovered.
func TestBondCannotBeWithdrawnEarly(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	_, err := f.keeper.WithdrawBond(f.ctx, n.operator.String())
	require.Error(t, err)
	require.True(t, types.ErrNotUnbonding.Is(err), "got %v", err)

	completion, err := f.keeper.BeginUnbonding(f.ctx, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, f.ctx.BlockHeight()+f.params(t).UnbondingBlocks, completion)

	_, err = f.keeper.WithdrawBond(f.ctx, n.operator.String())
	require.Error(t, err)
	require.True(t, types.ErrUnbondingIncomplete.Is(err), "got %v", err)

	later := f.ctx.WithBlockHeight(completion)
	returned, err := f.keeper.WithdrawBond(later, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, f.params(t).MinBond.String(), returned.String())
	require.Equal(t, f.params(t).MinBond.String(), f.bank.GetAllBalances(later, n.operator).String())

	// The registration is gone: no bond means nothing at stake.
	_, err = f.keeper.GetProvider(later, n.operator.String())
	require.Error(t, err)
	require.True(t, types.ErrProviderNotFound.Is(err), "got %v", err)
}

// TestUnbondingProviderStopsEarning
func TestUnbondingProviderStopsEarning(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	c := newClient()
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	_, err := f.keeper.BeginUnbonding(f.ctx, n.operator.String())
	require.NoError(t, err)

	r := f.signReceipt(t, c, n.operator, types.SERVICE_ROLE_RELAY, 0, 1, 10*oneGiB)
	accepted, rejected, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(0), accepted)
	require.Equal(t, uint64(1), rejected)
	require.Contains(t, reasons[0], "unbonding")
}

// ---------------------------------------------------------------------------
// Receipts: the core "never trust self-reported values" property
// ---------------------------------------------------------------------------

func TestValidReceiptCreditsProvider(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	c := newClient()
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	r := f.signReceipt(t, c, n.operator, types.SERVICE_ROLE_RELAY, 0, 1, 10*oneGiB)
	accepted, rejected, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(1), accepted, "reasons: %v", reasons)
	require.Equal(t, uint64(0), rejected)

	credit, err := f.keeper.GetCredit(f.ctx, 0, n.operator.String())
	require.NoError(t, err)
	// 10 GiB * 200 credit per GiB.
	require.Equal(t, "2000", credit.RelayCredit.String())
}

// TestSelfTrafficIsRejectedAndScored is §29: a provider serving itself is not
// evidence of service. This is the trivial form of fake traffic and is
// rejected outright rather than merely discounted.
func TestSelfTrafficIsRejectedAndScored(t *testing.T) {
	f := setup(t)

	// Build a provider whose operator key is also the receipt client key.
	priv := secp256k1.GenPrivKey()
	operator := sdk.AccAddress(priv.PubKey().Address())
	bond := f.params(t).MinBond
	f.bank.Fund(operator, bond)

	require.NoError(t, f.keeper.RegisterProvider(f.ctx, &types.MsgRegisterProvider{
		Operator:    operator.String(),
		NodePubkey:  priv.PubKey().Bytes(),
		NodeKeyType: types.KEY_TYPE_SECP256K1,
		Roles:       []types.ServiceRole{types.SERVICE_ROLE_RELAY},
		Bond:        bond,
	}))

	self := client{priv: priv}
	r := f.signReceipt(t, self, operator, types.SERVICE_ROLE_RELAY, 0, 1, 20*oneGiB)

	accepted, rejected, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(0), accepted)
	require.Equal(t, uint64(1), rejected)
	require.Contains(t, reasons[0], "provider itself")

	credit, err := f.keeper.GetCredit(f.ctx, 0, operator.String())
	require.NoError(t, err)
	require.True(t, credit.RelayCredit.IsZero())

	// Self-traffic is scored as fraud, not silently dropped.
	p, err := f.keeper.GetProvider(f.ctx, operator.String())
	require.NoError(t, err)
	require.Equal(t, types.FraudScoreForInvalidEvidence, p.FraudScore)

	reports, err := f.keeper.FraudReports(f.ctx, operator.String())
	require.NoError(t, err)
	require.Len(t, reports, 1)
	require.Equal(t, types.FraudReasonSelfTraffic, reports[0].Reason)
}

// TestReceiptWithForgedSignatureIsRejectedAndScored
func TestReceiptWithForgedSignatureIsRejectedAndScored(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	c := newClient()
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	r := f.signReceipt(t, c, n.operator, types.SERVICE_ROLE_RELAY, 0, 1, 10*oneGiB)
	// Inflate the claimed units after signing, staying under the per-receipt
	// cap so that the signature check is what rejects it.
	r.Units = 40 * oneGiB

	accepted, rejected, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(0), accepted)
	require.Equal(t, uint64(1), rejected)
	require.Contains(t, reasons[0], "signature")

	p, err := f.keeper.GetProvider(f.ctx, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, types.FraudScoreForInvalidEvidence, p.FraudScore)
}

// TestReceiptReplayIsRejectedAndScored
func TestReceiptReplayIsRejectedAndScored(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	c := newClient()
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	r := f.signReceipt(t, c, n.operator, types.SERVICE_ROLE_RELAY, 0, 7, 10*oneGiB)

	accepted, _, _, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(1), accepted)

	accepted, rejected, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(0), accepted)
	require.Equal(t, uint64(1), rejected)
	require.Contains(t, reasons[0], "nonce")

	// Credit was counted once, not twice.
	credit, err := f.keeper.GetCredit(f.ctx, 0, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, "2000", credit.RelayCredit.String())
}

// TestReceiptForAnotherNetworkIsRejected: evidence minted on a devnet or a
// fork must not verify on Mainnet.
func TestReceiptForAnotherNetworkIsRejected(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	c := newClient()
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	r := types.ServiceReceipt{
		Provider:      n.operator.String(),
		Role:          types.SERVICE_ROLE_RELAY,
		ClientPubkey:  c.priv.PubKey().Bytes(),
		ClientKeyType: types.KEY_TYPE_SECP256K1,
		Epoch:         0,
		Nonce:         1,
		Units:         10 * oneGiB,
		ExpiryHeight:  f.ctx.BlockHeight() + 100,
	}
	devnet := hgparams.DevnetIdentity("")
	digest := devnet.SigningDigest(hgparams.PurposeServiceReceipt, types.CanonicalReceiptBytes(r))
	sig, err := c.priv.Sign(digest[:])
	require.NoError(t, err)
	r.ClientSignature = sig

	_, rejected, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(1), rejected)
	require.Contains(t, reasons[0], "signature")
}

// TestOversizeReceiptIsRejected: one absurd receipt must not be able to
// dominate an epoch before the concentration cap is even reached.
func TestOversizeReceiptIsRejected(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	c := newClient()
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	r := f.signReceipt(t, c, n.operator, types.SERVICE_ROLE_RELAY, 0, 1,
		f.params(t).MaxReceiptUnits+1)

	_, rejected, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(1), rejected)
	require.Contains(t, reasons[0], "exceeds the cap")
}

// TestStorageReceiptsAreRejected: storage credit comes from assignments and
// challenges. A "storage receipt" would be self-reported storage.
func TestStorageReceiptsAreRejected(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	c := newClient()
	f.register(t, n, types.SERVICE_ROLE_STORAGE)

	r := f.signReceipt(t, c, n.operator, types.SERVICE_ROLE_STORAGE, 0, 1, 20*oneGiB)

	_, rejected, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(1), rejected)
	require.Contains(t, reasons[0], "assignments and challenges")
}

// TestReceiptForUnofferedRoleIsRejected
func TestReceiptForUnofferedRoleIsRejected(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	c := newClient()
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	r := f.signReceipt(t, c, n.operator, types.SERVICE_ROLE_CALL, 0, 1, 7200)

	_, rejected, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(1), rejected)
	require.Contains(t, reasons[0], "does not offer")
}

// TestReceiptForSettledEpochIsRejected: settlement must be final.
func TestReceiptForSettledEpochIsRejected(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	c := newClient()
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	require.NoError(t, f.keeper.SetCurrentEpochNumber(f.ctx, 5))

	r := f.signReceipt(t, c, n.operator, types.SERVICE_ROLE_RELAY, 3, 1, 10*oneGiB)
	_, rejected, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(1), rejected)
	require.Contains(t, reasons[0], "epoch 5 is open")
}

// TestExpiredReceiptIsRejected
func TestExpiredReceiptIsRejected(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	c := newClient()
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	r := f.signReceipt(t, c, n.operator, types.SERVICE_ROLE_RELAY, 0, 1, 10*oneGiB)

	late := f.ctx.WithBlockHeight(r.ExpiryHeight + 1)
	_, rejected, reasons, err := f.keeper.SubmitReceipts(late, []types.ServiceReceipt{r})
	require.NoError(t, err)
	require.Equal(t, uint64(1), rejected)
	require.Contains(t, reasons[0], "expired")
}

// TestOneBadReceiptDoesNotDiscardTheBatch: a relay serving thousands of
// clients should not lose a day's work to one malformed receipt.
func TestOneBadReceiptDoesNotDiscardTheBatch(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	var batch []types.ServiceReceipt
	for i := 0; i < 5; i++ {
		batch = append(batch, f.signReceipt(t, newClient(), n.operator,
			types.SERVICE_ROLE_RELAY, 0, uint64(i+1), 2*oneGiB))
	}
	// Corrupt the middle one, staying under the per-receipt cap so that the
	// signature check is what rejects it.
	batch[2].Units = 40 * oneGiB

	accepted, rejected, _, err := f.keeper.SubmitReceipts(f.ctx, batch)
	require.NoError(t, err)
	require.Equal(t, uint64(4), accepted)
	require.Equal(t, uint64(1), rejected)

	credit, err := f.keeper.GetCredit(f.ctx, 0, n.operator.String())
	require.NoError(t, err)
	// 4 receipts x 2 GiB x 200.
	require.Equal(t, "1600", credit.RelayCredit.String())
}

// ---------------------------------------------------------------------------
// The two-node fake traffic ring
// ---------------------------------------------------------------------------

// TestTwoNodeRingIsDiscounted is §29's hardest requirement: prevent "two-node
// infinite fake traffic".
//
// No signature check can stop it, because each node can legitimately sign for
// the other. What stops it is the per-counterparty concentration cap: a
// provider whose credit comes entirely from one counterparty keeps only
// max_client_concentration_bps of it. With the default 20% cap, the ring must
// spend five times the resources to earn what its receipts claim.
func TestTwoNodeRingIsDiscounted(t *testing.T) {
	f := setup(t)

	// Two colluding nodes, each signing receipts for the other.
	privA := secp256k1.GenPrivKey()
	privB := secp256k1.GenPrivKey()
	opA := sdk.AccAddress(privA.PubKey().Address())
	opB := sdk.AccAddress(privB.PubKey().Address())

	for _, pair := range []struct {
		op   sdk.AccAddress
		priv cryptotypes.PrivKey
	}{{opA, privA}, {opB, privB}} {
		bond := f.params(t).MinBond
		f.bank.Fund(pair.op, bond)
		require.NoError(t, f.keeper.RegisterProvider(f.ctx, &types.MsgRegisterProvider{
			Operator:    pair.op.String(),
			NodePubkey:  pair.priv.PubKey().Bytes(),
			NodeKeyType: types.KEY_TYPE_SECP256K1,
			Roles:       []types.ServiceRole{types.SERVICE_ROLE_RELAY},
			Bond:        bond,
		}))
	}

	// A signs for B and B signs for A: 20 GiB each, all from one
	// counterparty.
	rForA := f.signReceipt(t, client{priv: privB}, opA, types.SERVICE_ROLE_RELAY, 0, 1, 20*oneGiB)
	rForB := f.signReceipt(t, client{priv: privA}, opB, types.SERVICE_ROLE_RELAY, 0, 1, 20*oneGiB)

	accepted, _, reasons, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{rForA, rForB})
	require.NoError(t, err)
	require.Equal(t, uint64(2), accepted, "reasons: %v", reasons)

	// Raw credit is 20 GiB * 200 = 4000 each.
	creditA, err := f.keeper.GetCredit(f.ctx, 0, opA.String())
	require.NoError(t, err)
	require.Equal(t, "4000", creditA.RelayCredit.String())

	// An honest provider serving many clients, with the same raw total.
	honest := newNode("honest")
	f.register(t, honest, types.SERVICE_ROLE_RELAY)
	for i := 0; i < 20; i++ {
		r := f.signReceipt(t, newClient(), honest.operator, types.SERVICE_ROLE_RELAY,
			0, uint64(i+1), oneGiB)
		acc, _, why, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
		require.NoError(t, err)
		require.Equal(t, uint64(1), acc, "reasons: %v", why)
	}
	creditH, err := f.keeper.GetCredit(f.ctx, 0, honest.operator.String())
	require.NoError(t, err)
	require.Equal(t, "4000", creditH.RelayCredit.String(),
		"the honest provider should have the same raw credit for a fair comparison")

	// The mechanism: the ring's effective credit is discounted to the
	// concentration cap, the honest provider's is not.
	params := f.params(t)

	ringCredit, err := f.keeper.GetCredit(f.ctx, 0, opA.String())
	require.NoError(t, err)
	ringEffective, ringDistinct, err := f.keeper.EffectiveCredit(f.ctx, 0, ringCredit, params)
	require.NoError(t, err)
	require.Equal(t, uint64(1), ringDistinct, "the ring should have exactly one counterparty")

	honestCredit, err := f.keeper.GetCredit(f.ctx, 0, honest.operator.String())
	require.NoError(t, err)
	honestEffective, honestDistinct, err := f.keeper.EffectiveCredit(f.ctx, 0, honestCredit, params)
	require.NoError(t, err)
	require.Equal(t, uint64(20), honestDistinct)

	require.Equal(t, "4000", honestEffective.String(),
		"the honest provider's credit was discounted")
	// 4000 * 2000bps / 10000 = 800.
	require.Equal(t, "800", ringEffective.String(),
		"the ring kept %s of its 4000 claimed credit; the 20%% cap implies 800", ringEffective)

	// And the effect on payout. The per-provider cap is lifted for this
	// comparison because at Mainnet defaults it binds for both providers and
	// would mask the discount entirely.
	params.MaxProviderShareBps = types.BasisPointsMax
	require.NoError(t, f.keeper.UpdateParams(f.ctx, f.gov, params, nil))
	require.NoError(t, f.keeper.SettleEpoch(f.ctx, 0, f.ctx.BlockHeight(), params))

	paidRing := f.bank.GetAllBalances(f.ctx, sdk.AccAddress(privA.PubKey().Address()))
	paidHonest := f.bank.GetAllBalances(f.ctx, honest.reward)

	require.False(t, paidHonest.IsZero(), "the honest provider was paid nothing")

	ringAmt := paidRing.AmountOf(hgparams.BaseCoinDenom)
	honestAmt := paidHonest.AmountOf(hgparams.BaseCoinDenom)

	require.True(t, ringAmt.LT(honestAmt),
		"the fake-traffic ring earned %s and the honest provider %s; the ring was not discounted",
		ringAmt, honestAmt)

	// The ring should keep roughly a fifth for identical raw numbers.
	require.True(t, ringAmt.MulRaw(4).LTE(honestAmt),
		"the ring kept %s against the honest provider's %s, which is more than the 20%% cap implies",
		ringAmt, honestAmt)
}

// TestHonestProviderWithManyClientsIsNotDiscounted: the cap must not punish
// legitimate operation.
func TestHonestProviderWithManyClientsIsNotDiscounted(t *testing.T) {
	f := setup(t)
	n := newNode("a")
	f.register(t, n, types.SERVICE_ROLE_RELAY)

	// Ten clients, each contributing 10% of the total: none dominant.
	for i := 0; i < 10; i++ {
		r := f.signReceipt(t, newClient(), n.operator, types.SERVICE_ROLE_RELAY,
			0, uint64(i+1), 10*oneGiB)
		acc, _, why, err := f.keeper.SubmitReceipts(f.ctx, []types.ServiceReceipt{r})
		require.NoError(t, err)
		require.Equal(t, uint64(1), acc, "reasons: %v", why)
	}

	credit, err := f.keeper.GetCredit(f.ctx, 0, n.operator.String())
	require.NoError(t, err)
	raw := credit.RelayCredit
	require.Equal(t, "20000", raw.String())

	largest, distinct, err := f.keeper.LargestClientCredit(f.ctx, 0, n.operator.String())
	require.NoError(t, err)
	require.Equal(t, uint64(10), distinct)
	require.Equal(t, "2000", largest.String())

	// The largest client contributed 10%, below the 20% cap, so nothing is
	// discounted. Settle and confirm the provider takes the whole budget it
	// is entitled to.
	require.NoError(t, f.keeper.SettleEpoch(f.ctx, 0, f.ctx.BlockHeight(), f.params(t)))

	e, err := f.keeper.GetEpoch(f.ctx, 0)
	require.NoError(t, err)
	require.Equal(t, raw.String(), e.TotalCredit.String(),
		"an honest provider's credit was discounted")
}

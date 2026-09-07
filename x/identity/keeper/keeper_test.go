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
	"github.com/hashgram/hashgram/x/identity/keeper"
	"github.com/hashgram/hashgram/x/identity/types"
	networkkeeper "github.com/hashgram/hashgram/x/network/keeper"
	networktypes "github.com/hashgram/hashgram/x/network/types"
)

func TestMain(m *testing.M) {
	hgparams.SetSDKConfig()
	os.Exit(m.Run())
}

type fixture struct {
	ctx     sdk.Context
	keeper  keeper.Keeper
	network networkkeeper.Keeper
}

func setup(t *testing.T) *fixture {
	t.Helper()

	idKey := storetypes.NewKVStoreKey(types.StoreKey)
	netKey := storetypes.NewKVStoreKey(networktypes.StoreKey)

	testCtx := testutil.DefaultContextWithDB(t, idKey, storetypes.NewTransientStoreKey("transient_test"))
	cms := testCtx.CMS
	cms.MountStoreWithDB(netKey, storetypes.StoreTypeIAVL, testCtx.DB)
	require.NoError(t, cms.LoadLatestVersion())

	ctx := testCtx.Ctx.
		WithMultiStore(cms).
		WithChainID(hgparams.ChainIDMainnet).
		WithBlockHeight(1_000)

	cdc := codec.NewProtoCodec(codectypes.NewInterfaceRegistry())
	gov := authtypes.NewModuleAddress("gov").String()

	nk := networkkeeper.NewKeeper(cdc, runtime.NewKVStoreService(netKey), log.NewNopLogger())
	require.NoError(t, nk.InitGenesis(ctx, *networktypes.MainnetGenesis()))

	k := keeper.NewKeeper(cdc, runtime.NewKVStoreService(idKey), nk, gov, log.NewNopLogger())
	require.NoError(t, k.InitGenesis(ctx, *types.DefaultGenesis()))

	return &fixture{ctx: ctx, keeper: k, network: nk}
}

// user is a DEVNET-only identity with a root key held off chain.
type user struct {
	address sdk.AccAddress
	rootKey cryptotypes.PrivKey
}

func newUser(label string) user {
	b := make([]byte, 20)
	copy(b, label)
	return user{
		address: sdk.AccAddress(b),
		// secp256k1 for the root key, matching a hardware wallet.
		rootKey: secp256k1.GenPrivKey(),
	}
}

func addr(label string) sdk.AccAddress {
	b := make([]byte, 20)
	copy(b, label)
	return sdk.AccAddress(b)
}

// certify produces a root-signed device certificate.
func (f *fixture) certify(t *testing.T, u user, deviceID string, devicePub cryptotypes.PubKey, rotation uint32) types.DeviceCertificate {
	t.Helper()

	cert := types.DeviceCertificate{
		DeviceId:      deviceID,
		DevicePubkey:  devicePub.Bytes(),
		DeviceKeyType: types.KEY_TYPE_ED25519,
		RotationCount: rotation,
		ExpiryHeight:  f.ctx.BlockHeight() + 1_000,
	}

	digest, err := f.network.SigningDigest(f.ctx, hgparams.PurposeDeviceCert,
		types.CanonicalCertificateBytes(u.address.String(), cert))
	require.NoError(t, err)

	sig, err := u.rootKey.Sign(digest[:])
	require.NoError(t, err)
	cert.Signature = sig

	return cert
}

func (f *fixture) createIdentity(t *testing.T, u user) {
	t.Helper()
	require.NoError(t, f.keeper.CreateIdentity(f.ctx, &types.MsgCreateIdentity{
		Address:     u.address.String(),
		RootPubkey:  u.rootKey.PubKey().Bytes(),
		RootKeyType: types.KEY_TYPE_SECP256K1,
	}))
}

// ---------------------------------------------------------------------------
// Only public keys
// ---------------------------------------------------------------------------

// TestOnlyPublicKeysAreStored is the property the whole module exists for: a
// fully compromised Hashgram VPS cannot impersonate users because there is
// nothing on it to steal.
func TestOnlyPublicKeysAreStored(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	devicePriv := ed25519.GenPrivKey()
	cert := f.certify(t, u, "desktop", devicePriv.PubKey(), 0)
	require.NoError(t, f.keeper.AddDevice(f.ctx, u.address.String(), cert, "Windows desktop", "windows"))

	identity, found, err := f.keeper.GetIdentity(f.ctx, u.address.String())
	require.NoError(t, err)
	require.True(t, found)

	// The stored root key is the public key, and the private key's bytes
	// appear nowhere in state.
	require.Equal(t, u.rootKey.PubKey().Bytes(), identity.RootPubkey)
	require.NotEqual(t, u.rootKey.Bytes(), identity.RootPubkey)

	device, found, err := f.keeper.GetDevice(f.ctx, u.address.String(), "desktop")
	require.NoError(t, err)
	require.True(t, found)
	require.Equal(t, devicePriv.PubKey().Bytes(), device.DevicePubkey)
	require.NotEqual(t, devicePriv.Bytes(), device.DevicePubkey)
}

// ---------------------------------------------------------------------------
// Device authorisation
// ---------------------------------------------------------------------------

func TestCreateIdentityWithInitialDevice(t *testing.T) {
	f := setup(t)
	u := newUser("alice")

	devicePriv := ed25519.GenPrivKey()
	cert := f.certify(t, u, "phone", devicePriv.PubKey(), 0)

	require.NoError(t, f.keeper.CreateIdentity(f.ctx, &types.MsgCreateIdentity{
		Address:               u.address.String(),
		RootPubkey:            u.rootKey.PubKey().Bytes(),
		RootKeyType:           types.KEY_TYPE_SECP256K1,
		InitialDevice:         cert,
		InitialDeviceLabel:    "iPhone",
		InitialDevicePlatform: "ios",
	}))

	devices, err := f.keeper.ListDevices(f.ctx, u.address.String(), false)
	require.NoError(t, err)
	require.Len(t, devices, 1)
	require.Equal(t, "phone", devices[0].DeviceId)
	require.Equal(t, "iPhone", devices[0].Label)
}

// TestUnsignedCertificateIsRejected: paying the gas is not authority to
// insert a device. Only the root key is.
func TestUnsignedCertificateIsRejected(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	// An attacker who controls the account but not the root key.
	attacker := newUser("attacker")
	devicePriv := ed25519.GenPrivKey()

	cert := types.DeviceCertificate{
		DeviceId:      "rogue",
		DevicePubkey:  devicePriv.PubKey().Bytes(),
		DeviceKeyType: types.KEY_TYPE_ED25519,
		RotationCount: 0,
		ExpiryHeight:  f.ctx.BlockHeight() + 100,
	}
	digest, err := f.network.SigningDigest(f.ctx, hgparams.PurposeDeviceCert,
		types.CanonicalCertificateBytes(u.address.String(), cert))
	require.NoError(t, err)
	sig, err := attacker.rootKey.Sign(digest[:]) // wrong key
	require.NoError(t, err)
	cert.Signature = sig

	err = f.keeper.AddDevice(f.ctx, u.address.String(), cert, "rogue", "linux")
	require.Error(t, err)
	require.True(t, types.ErrInvalidCertificate.Is(err), "got %v", err)
}

// TestTamperingWithACertificateInvalidatesIt: every field is inside the
// signed preimage.
func TestTamperingWithACertificateInvalidatesIt(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	devicePriv := ed25519.GenPrivKey()
	valid := f.certify(t, u, "desktop", devicePriv.PubKey(), 0)

	for name, tamper := range map[string]func(*types.DeviceCertificate){
		"swap the device key": func(c *types.DeviceCertificate) {
			c.DevicePubkey = ed25519.GenPrivKey().PubKey().Bytes()
		},
		"rename the device":    func(c *types.DeviceCertificate) { c.DeviceId = "other" },
		"extend the expiry":    func(c *types.DeviceCertificate) { c.ExpiryHeight += 1 },
		"flip a signature bit": func(c *types.DeviceCertificate) { c.Signature[0] ^= 0xff },
	} {
		t.Run(name, func(t *testing.T) {
			cert := valid
			cert.Signature = append([]byte(nil), valid.Signature...)
			tamper(&cert)

			err := f.keeper.AddDevice(f.ctx, u.address.String(), cert, "x", "linux")
			require.Error(t, err, "a tampered certificate was accepted")
		})
	}
}

// TestCertificateForAnotherIdentityIsRejected: the root address is inside the
// signed bytes, so a certificate cannot be presented to a different identity
// that happens to share a root key.
func TestCertificateForAnotherIdentityIsRejected(t *testing.T) {
	f := setup(t)

	u := newUser("alice")
	f.createIdentity(t, u)

	// A second identity that reuses the same root key.
	other := user{address: addr("DEVNET-other"), rootKey: u.rootKey}
	require.NoError(t, f.keeper.CreateIdentity(f.ctx, &types.MsgCreateIdentity{
		Address:     other.address.String(),
		RootPubkey:  other.rootKey.PubKey().Bytes(),
		RootKeyType: types.KEY_TYPE_SECP256K1,
	}))

	// A certificate minted for `other` must not authorise a device on `u`.
	devicePriv := ed25519.GenPrivKey()
	cert := f.certify(t, other, "desktop", devicePriv.PubKey(), 0)

	err := f.keeper.AddDevice(f.ctx, u.address.String(), cert, "x", "linux")
	require.Error(t, err, "a certificate bound to another identity was accepted")
	require.True(t, types.ErrInvalidCertificate.Is(err), "got %v", err)
}

// TestCertificateFromAnotherNetworkIsRejected
func TestCertificateFromAnotherNetworkIsRejected(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	devicePriv := ed25519.GenPrivKey()
	cert := types.DeviceCertificate{
		DeviceId:      "desktop",
		DevicePubkey:  devicePriv.PubKey().Bytes(),
		DeviceKeyType: types.KEY_TYPE_ED25519,
		RotationCount: 0,
		ExpiryHeight:  f.ctx.BlockHeight() + 100,
	}
	devnet := hgparams.DevnetIdentity("")
	digest := devnet.SigningDigest(hgparams.PurposeDeviceCert,
		types.CanonicalCertificateBytes(u.address.String(), cert))
	sig, err := u.rootKey.Sign(digest[:])
	require.NoError(t, err)
	cert.Signature = sig

	err = f.keeper.AddDevice(f.ctx, u.address.String(), cert, "x", "linux")
	require.Error(t, err, "a certificate signed for another network was accepted")
	require.True(t, types.ErrInvalidCertificate.Is(err), "got %v", err)
}

func TestExpiredCertificateIsRejected(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	devicePriv := ed25519.GenPrivKey()
	cert := f.certify(t, u, "desktop", devicePriv.PubKey(), 0)

	late := f.ctx.WithBlockHeight(cert.ExpiryHeight + 1)
	err := f.keeper.AddDevice(late, u.address.String(), cert, "x", "linux")
	require.Error(t, err)
	require.True(t, types.ErrCertificateExpired.Is(err), "got %v", err)
}

// TestOverlongCertificateIsRejected: a certificate that never expires is a
// bearer authorisation.
func TestOverlongCertificateIsRejected(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	devicePriv := ed25519.GenPrivKey()
	cert := types.DeviceCertificate{
		DeviceId:      "desktop",
		DevicePubkey:  devicePriv.PubKey().Bytes(),
		DeviceKeyType: types.KEY_TYPE_ED25519,
		RotationCount: 0,
		ExpiryHeight:  f.ctx.BlockHeight() + types.DefaultMaxCertificateAgeBlocks + 1,
	}
	digest, err := f.network.SigningDigest(f.ctx, hgparams.PurposeDeviceCert,
		types.CanonicalCertificateBytes(u.address.String(), cert))
	require.NoError(t, err)
	sig, err := u.rootKey.Sign(digest[:])
	require.NoError(t, err)
	cert.Signature = sig

	err = f.keeper.AddDevice(f.ctx, u.address.String(), cert, "x", "linux")
	require.Error(t, err)
	require.True(t, types.ErrCertificateTooLong.Is(err), "got %v", err)
}

// TestDeviceKeysAreGloballyUnique: two identities sharing a device key would
// make event attribution ambiguous.
func TestDeviceKeysAreGloballyUnique(t *testing.T) {
	f := setup(t)

	alice := newUser("alice")
	bob := newUser("bob")
	f.createIdentity(t, alice)
	f.createIdentity(t, bob)

	shared := ed25519.GenPrivKey()

	require.NoError(t, f.keeper.AddDevice(f.ctx, alice.address.String(),
		f.certify(t, alice, "d1", shared.PubKey(), 0), "a", "linux"))

	err := f.keeper.AddDevice(f.ctx, bob.address.String(),
		f.certify(t, bob, "d1", shared.PubKey(), 0), "b", "linux")
	require.Error(t, err)
	require.True(t, types.ErrDeviceKeyInUse.Is(err), "got %v", err)
}

func TestDeviceLimitIsEnforced(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	limit := int(types.DefaultMaxDevicesPerIdentity)
	for i := 0; i < limit; i++ {
		id := "device-" + string(rune('a'+i))
		require.NoError(t, f.keeper.AddDevice(f.ctx, u.address.String(),
			f.certify(t, u, id, ed25519.GenPrivKey().PubKey(), 0), id, "linux"),
			"device %d within the limit was rejected", i)
	}

	err := f.keeper.AddDevice(f.ctx, u.address.String(),
		f.certify(t, u, "one-too-many", ed25519.GenPrivKey().PubKey(), 0), "x", "linux")
	require.Error(t, err)
	require.True(t, types.ErrTooManyDevices.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// Revocation
// ---------------------------------------------------------------------------

func TestRevokeDeviceKeepsTheRecord(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	for _, id := range []string{"d1", "d2"} {
		require.NoError(t, f.keeper.AddDevice(f.ctx, u.address.String(),
			f.certify(t, u, id, ed25519.GenPrivKey().PubKey(), 0), id, "linux"))
	}

	require.NoError(t, f.keeper.RevokeDevice(f.ctx, u.address.String(), "d1"))

	active, err := f.keeper.ListDevices(f.ctx, u.address.String(), false)
	require.NoError(t, err)
	require.Len(t, active, 1)

	// The record survives so that events signed before revocation remain
	// attributable.
	all, err := f.keeper.ListDevices(f.ctx, u.address.String(), true)
	require.NoError(t, err)
	require.Len(t, all, 2)

	d, found, err := f.keeper.GetDevice(f.ctx, u.address.String(), "d1")
	require.NoError(t, err)
	require.True(t, found)
	require.True(t, d.Revoked)
	require.Equal(t, f.ctx.BlockHeight(), d.RevokedHeight)
}

// TestRevokedDeviceIDCannotBeReused: reuse would make audit trails ambiguous
// about which key an event came from.
func TestRevokedDeviceIDCannotBeReused(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	for _, id := range []string{"d1", "d2"} {
		require.NoError(t, f.keeper.AddDevice(f.ctx, u.address.String(),
			f.certify(t, u, id, ed25519.GenPrivKey().PubKey(), 0), id, "linux"))
	}
	require.NoError(t, f.keeper.RevokeDevice(f.ctx, u.address.String(), "d1"))

	err := f.keeper.AddDevice(f.ctx, u.address.String(),
		f.certify(t, u, "d1", ed25519.GenPrivKey().PubKey(), 0), "again", "linux")
	require.Error(t, err)
	require.True(t, types.ErrDeviceRevoked.Is(err), "got %v", err)
}

// TestCannotRevokeTheLastDevice: an identity with no devices cannot sign
// anything, including the transaction that would add a replacement.
func TestCannotRevokeTheLastDevice(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	require.NoError(t, f.keeper.AddDevice(f.ctx, u.address.String(),
		f.certify(t, u, "only", ed25519.GenPrivKey().PubKey(), 0), "only", "linux"))

	err := f.keeper.RevokeDevice(f.ctx, u.address.String(), "only")
	require.Error(t, err)
	require.True(t, types.ErrLastDeviceRemove.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// Root key rotation
// ---------------------------------------------------------------------------

func (f *fixture) rotate(t *testing.T, u user, newKey cryptotypes.PrivKey, currentRotation uint32, revokeDevices bool) *types.MsgRotateRootKey {
	t.Helper()

	payload := types.CanonicalRotationBytes(u.address.String(),
		newKey.PubKey().Bytes(), types.KEY_TYPE_SECP256K1, currentRotation)
	digest, err := f.network.SigningDigest(f.ctx, hgparams.PurposeDeviceCert, payload)
	require.NoError(t, err)
	sig, err := u.rootKey.Sign(digest[:])
	require.NoError(t, err)

	return &types.MsgRotateRootKey{
		Address:               u.address.String(),
		NewRootPubkey:         newKey.PubKey().Bytes(),
		NewRootKeyType:        types.KEY_TYPE_SECP256K1,
		SignatureByOldRoot:    sig,
		RevokeExistingDevices: revokeDevices,
	}
}

// TestRotationRequiresTheOutgoingRootKey: an attacker holding only the
// account key must not be able to install their own root key and take over
// the identity's devices.
func TestRotationRequiresTheOutgoingRootKey(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	attacker := newUser("attacker")
	newKey := secp256k1.GenPrivKey()

	// Signed by the attacker's key rather than the identity's root key.
	payload := types.CanonicalRotationBytes(u.address.String(),
		newKey.PubKey().Bytes(), types.KEY_TYPE_SECP256K1, 0)
	digest, err := f.network.SigningDigest(f.ctx, hgparams.PurposeDeviceCert, payload)
	require.NoError(t, err)
	sig, err := attacker.rootKey.Sign(digest[:])
	require.NoError(t, err)

	_, err = f.keeper.RotateRootKey(f.ctx, &types.MsgRotateRootKey{
		Address:            u.address.String(),
		NewRootPubkey:      newKey.PubKey().Bytes(),
		NewRootKeyType:     types.KEY_TYPE_SECP256K1,
		SignatureByOldRoot: sig,
	})
	require.Error(t, err)
	require.True(t, types.ErrInvalidRotationSignature.Is(err), "got %v", err)
}

// TestRotationInvalidatesOldCertificates is what makes rotation meaningful:
// a certificate issued under the superseded root key stops verifying.
func TestRotationInvalidatesOldCertificates(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	// Mint a certificate under rotation 0, then rotate before presenting it.
	stale := f.certify(t, u, "stale", ed25519.GenPrivKey().PubKey(), 0)

	newKey := secp256k1.GenPrivKey()
	_, err := f.keeper.RotateRootKey(f.ctx, f.rotate(t, u, newKey, 0, false))
	require.NoError(t, err)

	err = f.keeper.AddDevice(f.ctx, u.address.String(), stale, "x", "linux")
	require.Error(t, err, "a certificate from the superseded root key was accepted")
	require.True(t, types.ErrRotationMismatch.Is(err), "got %v", err)

	// A certificate under the new key and rotation works.
	rotated := user{address: u.address, rootKey: newKey}
	fresh := f.certify(t, rotated, "fresh", ed25519.GenPrivKey().PubKey(), 1)
	require.NoError(t, f.keeper.AddDevice(f.ctx, u.address.String(), fresh, "fresh", "linux"))
}

// TestRotationCanRevokeAllDevices: the right default after a suspected
// root-key compromise, since certificates issued by the old root can no
// longer be trusted.
func TestRotationCanRevokeAllDevices(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	for _, id := range []string{"d1", "d2", "d3"} {
		require.NoError(t, f.keeper.AddDevice(f.ctx, u.address.String(),
			f.certify(t, u, id, ed25519.GenPrivKey().PubKey(), 0), id, "linux"))
	}

	revoked, err := f.keeper.RotateRootKey(f.ctx, f.rotate(t, u, secp256k1.GenPrivKey(), 0, true))
	require.NoError(t, err)
	require.Equal(t, 3, revoked)

	active, err := f.keeper.ListDevices(f.ctx, u.address.String(), false)
	require.NoError(t, err)
	require.Empty(t, active)
}

// TestRotationSignatureCannotBeReplayed: the current rotation count is inside
// the signed bytes, so an old rotation signature cannot undo a later one.
func TestRotationSignatureCannotBeReplayed(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	key1 := secp256k1.GenPrivKey()
	msg := f.rotate(t, u, key1, 0, false)
	_, err := f.keeper.RotateRootKey(f.ctx, msg)
	require.NoError(t, err)

	// Replaying the same message: the identity is now at rotation 1, and the
	// signature committed to rotation 0.
	_, err = f.keeper.RotateRootKey(f.ctx, msg)
	require.Error(t, err, "a rotation signature was replayable")
	require.True(t, types.ErrInvalidRotationSignature.Is(err), "got %v", err)
}

// ---------------------------------------------------------------------------
// Device key resolution
// ---------------------------------------------------------------------------

// TestResolveDeviceKeyAnswersWhoSigned: this is the query a client makes on
// receiving a signed event, so it does not have to trust the sender's own
// claim about who they are.
func TestResolveDeviceKeyAnswersWhoSigned(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	devicePriv := ed25519.GenPrivKey()
	require.NoError(t, f.keeper.AddDevice(f.ctx, u.address.String(),
		f.certify(t, u, "desktop", devicePriv.PubKey(), 0), "desktop", "windows"))

	device, found, err := f.keeper.ResolveDeviceKey(f.ctx, devicePriv.PubKey().Bytes())
	require.NoError(t, err)
	require.True(t, found)
	require.Equal(t, u.address.String(), device.RootAddress)
	require.Equal(t, "desktop", device.DeviceId)

	_, found, err = f.keeper.ResolveDeviceKey(f.ctx, ed25519.GenPrivKey().PubKey().Bytes())
	require.NoError(t, err)
	require.False(t, found)
}

// TestRevokedDeviceKeyStillResolves: a client verifying a historical
// signature needs to resolve the key that made it.
func TestRevokedDeviceKeyStillResolves(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)

	devicePriv := ed25519.GenPrivKey()
	for _, id := range []string{"d1", "d2"} {
		key := devicePriv
		if id == "d2" {
			key = ed25519.GenPrivKey()
		}
		require.NoError(t, f.keeper.AddDevice(f.ctx, u.address.String(),
			f.certify(t, u, id, key.PubKey(), 0), id, "linux"))
	}
	require.NoError(t, f.keeper.RevokeDevice(f.ctx, u.address.String(), "d1"))

	device, found, err := f.keeper.ResolveDeviceKey(f.ctx, devicePriv.PubKey().Bytes())
	require.NoError(t, err)
	require.True(t, found, "a revoked device key stopped resolving; old events became unattributable")
	require.True(t, device.Revoked)
	require.Equal(t, f.ctx.BlockHeight(), device.RevokedHeight,
		"the revocation height is what tells a client whether a signature predates it")
}

// ---------------------------------------------------------------------------
// Social recovery
// ---------------------------------------------------------------------------

func recoveryConfig(guardians []sdk.AccAddress, threshold uint32) types.RecoveryConfig {
	addrs := make([]string, len(guardians))
	for i, g := range guardians {
		addrs[i] = g.String()
	}
	return types.RecoveryConfig{
		Guardians:           addrs,
		Threshold:           threshold,
		RecoveryDelayBlocks: types.DefaultMinRecoveryDelayBlocks,
	}
}

func TestRecoveryRequiresThresholdAndDelay(t *testing.T) {
	f := setup(t)
	u := newUser("alice")

	g1, g2, g3 := addr("DEVNET-g1"), addr("DEVNET-g2"), addr("DEVNET-g3")

	require.NoError(t, f.keeper.CreateIdentity(f.ctx, &types.MsgCreateIdentity{
		Address:     u.address.String(),
		RootPubkey:  u.rootKey.PubKey().Bytes(),
		RootKeyType: types.KEY_TYPE_SECP256K1,
		Recovery:    recoveryConfig([]sdk.AccAddress{g1, g2, g3}, 2),
	}))
	require.NoError(t, f.keeper.AddDevice(f.ctx, u.address.String(),
		f.certify(t, u, "d1", ed25519.GenPrivKey().PubKey(), 0), "d1", "linux"))

	newKey := secp256k1.GenPrivKey()
	newAddress := addr("DEVNET-alice2")

	executable, err := f.keeper.InitiateRecovery(f.ctx, &types.MsgInitiateRecovery{
		Initiator:      g1.String(),
		RootAddress:    u.address.String(),
		NewAddress:     newAddress.String(),
		NewRootPubkey:  newKey.PubKey().Bytes(),
		NewRootKeyType: types.KEY_TYPE_SECP256K1,
	})
	require.NoError(t, err)
	require.Equal(t, f.ctx.BlockHeight()+types.DefaultMinRecoveryDelayBlocks, executable)

	// One approval of two: below threshold.
	err = f.keeper.ExecuteRecovery(f.ctx.WithBlockHeight(executable), u.address.String())
	require.Error(t, err)
	require.True(t, types.ErrThresholdNotMet.Is(err), "got %v", err)

	approvals, threshold, err := f.keeper.ApproveRecovery(f.ctx, g2.String(), u.address.String())
	require.NoError(t, err)
	require.Equal(t, uint32(2), approvals)
	require.Equal(t, uint32(2), threshold)

	// Threshold met but the delay has not elapsed.
	err = f.keeper.ExecuteRecovery(f.ctx, u.address.String())
	require.Error(t, err)
	require.True(t, types.ErrRecoveryDelayActive.Is(err), "got %v", err)

	// After the delay it completes.
	after := f.ctx.WithBlockHeight(executable)
	require.NoError(t, f.keeper.ExecuteRecovery(after, u.address.String()))

	// The old identity is revoked and the new one exists.
	old, found, err := f.keeper.GetIdentity(after, u.address.String())
	require.NoError(t, err)
	require.True(t, found)
	require.True(t, old.Revoked)

	recovered, found, err := f.keeper.GetIdentity(after, newAddress.String())
	require.NoError(t, err)
	require.True(t, found)
	require.Equal(t, newKey.PubKey().Bytes(), recovered.RootPubkey)

	// Every device authorised by the lost root key is revoked.
	active, err := f.keeper.ListDevices(after, u.address.String(), false)
	require.NoError(t, err)
	require.Empty(t, active, "devices authorised by the lost root key survived recovery")
}

// TestOwnerCanCancelRecoveryDuringTheDelay is what makes the delay
// protective: a user whose guardians were socially engineered can refuse.
func TestOwnerCanCancelRecoveryDuringTheDelay(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	g1, g2 := addr("DEVNET-g1"), addr("DEVNET-g2")

	require.NoError(t, f.keeper.CreateIdentity(f.ctx, &types.MsgCreateIdentity{
		Address:     u.address.String(),
		RootPubkey:  u.rootKey.PubKey().Bytes(),
		RootKeyType: types.KEY_TYPE_SECP256K1,
		Recovery:    recoveryConfig([]sdk.AccAddress{g1, g2}, 2),
	}))

	newKey := secp256k1.GenPrivKey()
	executable, err := f.keeper.InitiateRecovery(f.ctx, &types.MsgInitiateRecovery{
		Initiator:      g1.String(),
		RootAddress:    u.address.String(),
		NewAddress:     addr("DEVNET-attacker").String(),
		NewRootPubkey:  newKey.PubKey().Bytes(),
		NewRootKeyType: types.KEY_TYPE_SECP256K1,
	})
	require.NoError(t, err)

	_, _, err = f.keeper.ApproveRecovery(f.ctx, g2.String(), u.address.String())
	require.NoError(t, err)

	// The real owner refuses.
	require.NoError(t, f.keeper.CancelRecovery(f.ctx, u.address.String()))

	err = f.keeper.ExecuteRecovery(f.ctx.WithBlockHeight(executable), u.address.String())
	require.Error(t, err)
	require.True(t, types.ErrRecoveryCancelled.Is(err), "got %v", err)

	// The cancelled request is kept, so the attempted takeover leaves a trace.
	req, found, err := f.keeper.GetRecovery(f.ctx, u.address.String())
	require.NoError(t, err)
	require.True(t, found)
	require.True(t, req.Cancelled)
	require.Equal(t, []string{g1.String(), g2.String()}, req.Approvals)
}

func TestOnlyGuardiansCanInitiateOrApprove(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	g1 := addr("DEVNET-g1")
	stranger := addr("DEVNET-stranger")

	require.NoError(t, f.keeper.CreateIdentity(f.ctx, &types.MsgCreateIdentity{
		Address:     u.address.String(),
		RootPubkey:  u.rootKey.PubKey().Bytes(),
		RootKeyType: types.KEY_TYPE_SECP256K1,
		Recovery:    recoveryConfig([]sdk.AccAddress{g1}, 1),
	}))

	_, err := f.keeper.InitiateRecovery(f.ctx, &types.MsgInitiateRecovery{
		Initiator:      stranger.String(),
		RootAddress:    u.address.String(),
		NewAddress:     addr("DEVNET-new").String(),
		NewRootPubkey:  secp256k1.GenPrivKey().PubKey().Bytes(),
		NewRootKeyType: types.KEY_TYPE_SECP256K1,
	})
	require.Error(t, err)
	require.True(t, types.ErrNotAGuardian.Is(err), "got %v", err)
}

func TestGuardianCannotApproveTwice(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	g1, g2 := addr("DEVNET-g1"), addr("DEVNET-g2")

	require.NoError(t, f.keeper.CreateIdentity(f.ctx, &types.MsgCreateIdentity{
		Address:     u.address.String(),
		RootPubkey:  u.rootKey.PubKey().Bytes(),
		RootKeyType: types.KEY_TYPE_SECP256K1,
		Recovery:    recoveryConfig([]sdk.AccAddress{g1, g2}, 2),
	}))

	_, err := f.keeper.InitiateRecovery(f.ctx, &types.MsgInitiateRecovery{
		Initiator:      g1.String(),
		RootAddress:    u.address.String(),
		NewAddress:     addr("DEVNET-new").String(),
		NewRootPubkey:  secp256k1.GenPrivKey().PubKey().Bytes(),
		NewRootKeyType: types.KEY_TYPE_SECP256K1,
	})
	require.NoError(t, err)

	// The initiator already counts as an approval.
	_, _, err = f.keeper.ApproveRecovery(f.ctx, g1.String(), u.address.String())
	require.Error(t, err)
	require.True(t, types.ErrAlreadyApproved.Is(err), "got %v", err)
}

func TestRecoveryDisabledWithoutThreshold(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u) // no recovery config

	_, err := f.keeper.InitiateRecovery(f.ctx, &types.MsgInitiateRecovery{
		Initiator:      addr("DEVNET-g1").String(),
		RootAddress:    u.address.String(),
		NewAddress:     addr("DEVNET-new").String(),
		NewRootPubkey:  secp256k1.GenPrivKey().PubKey().Bytes(),
		NewRootKeyType: types.KEY_TYPE_SECP256K1,
	})
	require.Error(t, err)
	require.True(t, types.ErrRecoveryDisabled.Is(err), "got %v", err)
}

// TestRecoveryDelayFloorIsEnforced: a zero delay would make social recovery a
// single-step takeover for anyone who can persuade enough guardians.
func TestRecoveryDelayFloorIsEnforced(t *testing.T) {
	f := setup(t)
	u := newUser("alice")

	cfg := recoveryConfig([]sdk.AccAddress{addr("DEVNET-g1")}, 1)
	cfg.RecoveryDelayBlocks = 0

	err := f.keeper.CreateIdentity(f.ctx, &types.MsgCreateIdentity{
		Address:     u.address.String(),
		RootPubkey:  u.rootKey.PubKey().Bytes(),
		RootKeyType: types.KEY_TYPE_SECP256K1,
		Recovery:    cfg,
	})
	require.Error(t, err)
	require.True(t, types.ErrInvalidRecovery.Is(err), "got %v", err)
	require.Contains(t, err.Error(), "notice and cancel")
}

func TestThresholdCannotExceedGuardianCount(t *testing.T) {
	f := setup(t)
	u := newUser("alice")

	cfg := recoveryConfig([]sdk.AccAddress{addr("DEVNET-g1")}, 3)

	err := f.keeper.CreateIdentity(f.ctx, &types.MsgCreateIdentity{
		Address:     u.address.String(),
		RootPubkey:  u.rootKey.PubKey().Bytes(),
		RootKeyType: types.KEY_TYPE_SECP256K1,
		Recovery:    cfg,
	})
	require.Error(t, err)
	require.Contains(t, err.Error(), "impossible")
}

// ---------------------------------------------------------------------------
// Genesis
// ---------------------------------------------------------------------------

func TestExportGenesisRoundTrips(t *testing.T) {
	f := setup(t)
	u := newUser("alice")
	f.createIdentity(t, u)
	require.NoError(t, f.keeper.AddDevice(f.ctx, u.address.String(),
		f.certify(t, u, "d1", ed25519.GenPrivKey().PubKey(), 0), "d1", "linux"))

	out, err := f.keeper.ExportGenesis(f.ctx)
	require.NoError(t, err)
	require.Len(t, out.Identities, 1)
	require.Len(t, out.Devices, 1)
	require.NoError(t, out.Validate())
}

// TestGenesisRejectsSharedDeviceKeys
func TestGenesisRejectsSharedDeviceKeys(t *testing.T) {
	shared := ed25519.GenPrivKey().PubKey().Bytes()
	a := newUser("alice")
	b := newUser("bob")

	gs := types.DefaultGenesis()
	gs.Identities = []types.RootIdentity{
		{Address: a.address.String(), RootPubkey: a.rootKey.PubKey().Bytes(), RootKeyType: types.KEY_TYPE_SECP256K1},
		{Address: b.address.String(), RootPubkey: b.rootKey.PubKey().Bytes(), RootKeyType: types.KEY_TYPE_SECP256K1},
	}
	gs.Devices = []types.Device{
		{RootAddress: a.address.String(), DeviceId: "d1", DevicePubkey: shared, DeviceKeyType: types.KEY_TYPE_ED25519},
		{RootAddress: b.address.String(), DeviceId: "d1", DevicePubkey: shared, DeviceKeyType: types.KEY_TYPE_ED25519},
	}

	err := gs.Validate()
	require.Error(t, err)
	require.True(t, types.ErrDeviceKeyInUse.Is(err), "got %v", err)
}

func TestGenesisRejectsOrphanDevices(t *testing.T) {
	gs := types.DefaultGenesis()
	gs.Devices = []types.Device{{
		RootAddress:   addr("DEVNET-nobody").String(),
		DeviceId:      "d1",
		DevicePubkey:  ed25519.GenPrivKey().PubKey().Bytes(),
		DeviceKeyType: types.KEY_TYPE_ED25519,
	}}

	err := gs.Validate()
	require.Error(t, err)
	require.True(t, types.ErrIdentityNotFound.Is(err), "got %v", err)
}

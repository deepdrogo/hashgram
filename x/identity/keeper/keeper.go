// Package keeper implements x/identity: root identities, device
// certificates and social recovery.
//
// Only public keys are stored. No Hashgram component ever holds a user's
// private key, which is why recovery is a threshold of chosen guardians
// rather than an administrative reset.
package keeper

import (
	"context"
	"errors"
	"fmt"

	"cosmossdk.io/collections"
	storetypes "cosmossdk.io/core/store"
	"cosmossdk.io/log"

	"github.com/cosmos/cosmos-sdk/codec"
	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/identity/types"
)

// NetworkKeeper supplies the domain-separated digest a device certificate is
// signed over, so a certificate minted on a devnet or a fork cannot authorise
// a device on Mainnet.
type NetworkKeeper interface {
	SigningDigest(ctx context.Context, purpose hgparams.SigningPurpose, payload []byte) ([32]byte, error)
}

// Keeper manages root identities, devices and recovery.
//
// Only public keys are stored. The chain never receives, stores or transports
// a private key, and no Hashgram server holds one. This is why a compromised
// Hashgram VPS cannot impersonate users: there is nothing on it to steal.
type Keeper struct {
	cdc           codec.BinaryCodec
	logger        log.Logger
	networkKeeper NetworkKeeper
	authority     string

	Schema collections.Schema

	params     collections.Item[types.Params]
	identities collections.Map[string, types.RootIdentity]

	// devices is keyed by (root address, device id).
	devices collections.Map[collections.Pair[string, string], types.Device]

	// deviceKeyIndex maps a device public key to (root address, device id).
	//
	// This is what makes "which identity authorised the device that signed
	// this event?" an index lookup rather than a scan over every identity.
	// Without it, a client verifying a social event would force the node to
	// walk the whole device set.
	deviceKeyIndex collections.Map[[]byte, string]

	recoveries collections.Map[string, types.RecoveryRequest]
}

// NewKeeper constructs the x/identity keeper.
func NewKeeper(
	cdc codec.BinaryCodec,
	storeService storetypes.KVStoreService,
	nk NetworkKeeper,
	authority string,
	logger log.Logger,
) Keeper {
	if _, err := sdk.AccAddressFromBech32(authority); err != nil {
		panic(fmt.Sprintf("x/identity: invalid authority %q: %v", authority, err))
	}

	sb := collections.NewSchemaBuilder(storeService)

	k := Keeper{
		cdc:           cdc,
		logger:        logger.With("module", "x/"+types.ModuleName),
		networkKeeper: nk,
		authority:     authority,
		params: collections.NewItem(sb, types.ParamsKey, "params",
			codec.CollValue[types.Params](cdc)),
		identities: collections.NewMap(sb, types.IdentityKey, "identities",
			collections.StringKey, codec.CollValue[types.RootIdentity](cdc)),
		devices: collections.NewMap(sb, types.DeviceKey, "devices",
			collections.PairKeyCodec(collections.StringKey, collections.StringKey),
			codec.CollValue[types.Device](cdc)),
		deviceKeyIndex: collections.NewMap(sb, types.DeviceKeyIndex, "device_key_index",
			collections.BytesKey, collections.StringValue),
		recoveries: collections.NewMap(sb, types.RecoveryKey, "recoveries",
			collections.StringKey, codec.CollValue[types.RecoveryRequest](cdc)),
	}

	schema, err := sb.Build()
	if err != nil {
		panic(err)
	}
	k.Schema = schema
	return k
}

// Authority returns the governance module address.
func (k Keeper) Authority() string { return k.authority }

// Logger returns the module logger.
func (k Keeper) Logger() log.Logger { return k.logger }

// SetParams writes the parameter set.
func (k Keeper) SetParams(ctx context.Context, p types.Params) error {
	if err := p.Validate(); err != nil {
		return err
	}
	return k.params.Set(ctx, p)
}

// GetParams reads the parameter set.
func (k Keeper) GetParams(ctx context.Context) (types.Params, error) {
	p, err := k.params.Get(ctx)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.Params{}, types.ErrParamsNotSet
		}
		return types.Params{}, err
	}
	return p, nil
}

// GetIdentity reads a root identity.
func (k Keeper) GetIdentity(ctx context.Context, address string) (types.RootIdentity, bool, error) {
	id, err := k.identities.Get(ctx, address)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.RootIdentity{}, false, nil
		}
		return types.RootIdentity{}, false, err
	}
	return id, true, nil
}

// SetIdentity writes a root identity.
func (k Keeper) SetIdentity(ctx context.Context, id types.RootIdentity) error {
	return k.identities.Set(ctx, id.Address, id)
}

// IterateIdentities walks every identity in address order.
func (k Keeper) IterateIdentities(ctx context.Context, fn func(types.RootIdentity) (stop bool, err error)) error {
	return k.identities.Walk(ctx, nil, func(_ string, v types.RootIdentity) (bool, error) {
		return fn(v)
	})
}

// Identities exposes the underlying map for paginated queries.
func (k Keeper) Identities() collections.Map[string, types.RootIdentity] {
	return k.identities
}

// GetDevice reads a device.
func (k Keeper) GetDevice(ctx context.Context, rootAddress, deviceID string) (types.Device, bool, error) {
	d, err := k.devices.Get(ctx, collections.Join(rootAddress, deviceID))
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.Device{}, false, nil
		}
		return types.Device{}, false, err
	}
	return d, true, nil
}

// SetDevice writes a device and its key index entry.
func (k Keeper) SetDevice(ctx context.Context, d types.Device) error {
	if err := k.devices.Set(ctx, collections.Join(d.RootAddress, d.DeviceId), d); err != nil {
		return err
	}
	// The index is kept for revoked devices too: a client verifying a
	// historical signature needs to resolve the key that made it, and
	// dropping the entry would make old events unattributable.
	return k.deviceKeyIndex.Set(ctx, d.DevicePubkey, indexValue(d.RootAddress, d.DeviceId))
}

// ResolveDeviceKey finds the identity and device a public key belongs to.
func (k Keeper) ResolveDeviceKey(ctx context.Context, pubkey []byte) (types.Device, bool, error) {
	v, err := k.deviceKeyIndex.Get(ctx, pubkey)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.Device{}, false, nil
		}
		return types.Device{}, false, err
	}
	rootAddress, deviceID, ok := parseIndexValue(v)
	if !ok {
		return types.Device{}, false, fmt.Errorf("x/identity: corrupt device key index entry %q", v)
	}
	return k.GetDeviceOrEmpty(ctx, rootAddress, deviceID)
}

// GetDeviceOrEmpty is GetDevice without the error on absence.
func (k Keeper) GetDeviceOrEmpty(ctx context.Context, rootAddress, deviceID string) (types.Device, bool, error) {
	return k.GetDevice(ctx, rootAddress, deviceID)
}

// DeviceKeyInUse reports whether a device public key is already registered.
//
// Device keys must be globally unique: two identities sharing one would make
// "which identity signed this?" ambiguous, which is exactly the question the
// index exists to answer.
func (k Keeper) DeviceKeyInUse(ctx context.Context, pubkey []byte) (bool, error) {
	return k.deviceKeyIndex.Has(ctx, pubkey)
}

// ListDevices returns an identity's devices.
func (k Keeper) ListDevices(ctx context.Context, rootAddress string, includeRevoked bool) ([]types.Device, error) {
	var out []types.Device
	rng := collections.NewPrefixedPairRange[string, string](rootAddress)
	err := k.devices.Walk(ctx, rng, func(_ collections.Pair[string, string], d types.Device) (bool, error) {
		if d.Revoked && !includeRevoked {
			return false, nil
		}
		out = append(out, d)
		return false, nil
	})
	return out, err
}

// ActiveDeviceCount counts an identity's unrevoked devices.
//
// Returns an int rather than the uint32 the proto field uses, because this
// is a slice length and int is its natural type. Callers that need to fill
// the proto field convert at that point, where the bound is visible.
func (k Keeper) ActiveDeviceCount(ctx context.Context, rootAddress string) (int, error) {
	devices, err := k.ListDevices(ctx, rootAddress, false)
	if err != nil {
		return 0, err
	}
	return len(devices), nil
}

// AllDevices returns every device, for genesis export.
func (k Keeper) AllDevices(ctx context.Context) ([]types.Device, error) {
	var out []types.Device
	err := k.devices.Walk(ctx, nil, func(_ collections.Pair[string, string], d types.Device) (bool, error) {
		out = append(out, d)
		return false, nil
	})
	return out, err
}

// GetRecovery reads an identity's in-progress recovery request.
func (k Keeper) GetRecovery(ctx context.Context, rootAddress string) (types.RecoveryRequest, bool, error) {
	r, err := k.recoveries.Get(ctx, rootAddress)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.RecoveryRequest{}, false, nil
		}
		return types.RecoveryRequest{}, false, err
	}
	return r, true, nil
}

// SetRecovery writes a recovery request.
func (k Keeper) SetRecovery(ctx context.Context, r types.RecoveryRequest) error {
	return k.recoveries.Set(ctx, r.RootAddress, r)
}

// AllRecoveries returns every recovery request, for genesis export.
func (k Keeper) AllRecoveries(ctx context.Context) ([]types.RecoveryRequest, error) {
	var out []types.RecoveryRequest
	err := k.recoveries.Walk(ctx, nil, func(_ string, r types.RecoveryRequest) (bool, error) {
		out = append(out, r)
		return false, nil
	})
	return out, err
}

// indexValue and parseIndexValue encode a (root address, device id) pair as a
// single index value.
//
// A newline separator is safe because device ids are restricted to
// [a-zA-Z0-9_.:-] and bech32 addresses contain no newline either, so the
// split is unambiguous.
func indexValue(rootAddress, deviceID string) string {
	return rootAddress + "\n" + deviceID
}

func parseIndexValue(v string) (rootAddress, deviceID string, ok bool) {
	for i := 0; i < len(v); i++ {
		if v[i] == '\n' {
			return v[:i], v[i+1:], true
		}
	}
	return "", "", false
}

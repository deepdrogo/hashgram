package types

import (
	"encoding/binary"

	"github.com/cosmos/cosmos-sdk/crypto/keys/ed25519"
	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	cryptotypes "github.com/cosmos/cosmos-sdk/crypto/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
)

// CanonicalReceiptBytes returns the exact bytes a client signs.
//
// Hand-built with explicit length prefixes for the same reason as everywhere
// else in Hashgram: protobuf is not a canonical encoding, and a signature
// over "whatever proto produced" is a signature over something the verifier
// may not reproduce byte for byte.
//
// Layout, all integers big-endian:
//
//	uint32(len(provider)) || provider
//	uint32(role)
//	uint32(len(client_pubkey)) || client_pubkey
//	uint32(client_key_type)
//	uint64(epoch)
//	uint64(nonce)
//	uint64(units)
//	uint64(expiry_height as unsigned two's complement)
//
// The client's own public key is inside the signed bytes. Without it, an
// attacker who observed a receipt could re-present the same signature
// alongside a different declared key.
func CanonicalReceiptBytes(r ServiceReceipt) []byte {
	provider := []byte(r.Provider)

	out := make([]byte, 0, 4+len(provider)+4+4+len(r.ClientPubkey)+4+8+8+8+8)

	out = binary.BigEndian.AppendUint32(out, uint32(len(provider)))
	out = append(out, provider...)

	out = binary.BigEndian.AppendUint32(out, uint32(r.Role))

	out = binary.BigEndian.AppendUint32(out, uint32(len(r.ClientPubkey)))
	out = append(out, r.ClientPubkey...)

	out = binary.BigEndian.AppendUint32(out, uint32(r.ClientKeyType))
	out = binary.BigEndian.AppendUint64(out, r.Epoch)
	out = binary.BigEndian.AppendUint64(out, r.Nonce)
	out = binary.BigEndian.AppendUint64(out, r.Units)
	//nolint:gosec // two's-complement round trip is intentional and exact
	out = binary.BigEndian.AppendUint64(out, uint64(r.ExpiryHeight))

	return out
}

// ValidateBasic performs stateless validation of a receipt.
func (r ServiceReceipt) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(r.Provider); err != nil {
		return ErrInvalidReceipt.Wrapf("provider %q: %v", r.Provider, err)
	}
	if err := ValidateRole(r.Role); err != nil {
		return err
	}
	if r.Units == 0 {
		return ErrInvalidReceipt.Wrap("units must be positive; a receipt for no work is not evidence")
	}
	if r.ExpiryHeight <= 0 {
		return ErrInvalidReceipt.Wrap("expiry_height must be positive")
	}
	if len(r.ClientSignature) != 64 {
		return ErrInvalidSignature.Wrapf("signature is %d bytes, expected 64", len(r.ClientSignature))
	}
	if _, err := PubKeyFromBytes(r.ClientPubkey, r.ClientKeyType); err != nil {
		return err
	}
	return nil
}

// ClientAddress returns the account address derived from the receipt's client
// key. Used for the self-traffic check and for per-client concentration
// accounting.
func (r ServiceReceipt) ClientAddress() (sdk.AccAddress, error) {
	pk, err := PubKeyFromBytes(r.ClientPubkey, r.ClientKeyType)
	if err != nil {
		return nil, err
	}
	return sdk.AccAddress(pk.Address()), nil
}

// VerifyClientSignature checks the receipt signature against a digest.
func (r ServiceReceipt) VerifyClientSignature(digest []byte) error {
	pk, err := PubKeyFromBytes(r.ClientPubkey, r.ClientKeyType)
	if err != nil {
		return err
	}
	if !pk.VerifySignature(digest, r.ClientSignature) {
		return ErrInvalidSignature.Wrap("receipt signature does not verify for the declared client key")
	}
	return nil
}

// PubKeyFromBytes reconstructs a public key from raw bytes and a key type.
func PubKeyFromBytes(raw []byte, kt KeyType) (cryptotypes.PubKey, error) {
	switch kt {
	case KEY_TYPE_SECP256K1:
		if len(raw) != secp256k1.PubKeySize {
			return nil, ErrInvalidKey.Wrapf(
				"secp256k1 key is %d bytes, expected %d", len(raw), secp256k1.PubKeySize)
		}
		return &secp256k1.PubKey{Key: raw}, nil

	case KEY_TYPE_ED25519:
		if len(raw) != ed25519.PubKeySize {
			return nil, ErrInvalidKey.Wrapf(
				"ed25519 key is %d bytes, expected %d", len(raw), ed25519.PubKeySize)
		}
		return &ed25519.PubKey{Key: raw}, nil

	default:
		return nil, ErrInvalidKey.Wrapf("unsupported key type %s", kt)
	}
}

// ValidateRole rejects the unspecified role.
func ValidateRole(role ServiceRole) error {
	if role == SERVICE_ROLE_UNSPECIFIED {
		return ErrInvalidRole.Wrap("service role must be specified")
	}
	if _, ok := ServiceRole_name[int32(role)]; !ok {
		return ErrInvalidRole.Wrapf("unknown service role %d", role)
	}
	return nil
}

// HasRole reports whether a provider offers a role.
func (p Provider) HasRole(role ServiceRole) bool {
	for _, r := range p.Roles {
		if r == role {
			return true
		}
	}
	return false
}

// IsActive reports whether a provider may earn.
func (p Provider) IsActive() bool {
	return !p.Jailed && p.UnbondingHeight == 0
}

package types

import (
	"encoding/binary"
	"regexp"
	"strings"

	"github.com/cosmos/cosmos-sdk/crypto/keys/ed25519"
	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	cryptotypes "github.com/cosmos/cosmos-sdk/crypto/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
)

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

// CanonicalCertificateBytes returns the exact bytes a root key signs to
// authorise a device.
//
// Hand-built with explicit length prefixes, for the same reason as every
// other signed object in Hashgram: protobuf is not a canonical encoding, so a
// signature over "whatever proto produced" is a signature over something the
// verifier may not reproduce.
//
// The root address and rotation count are inside the signed bytes. Without
// the address, a certificate could be presented to a different identity that
// happened to share a root key; without the rotation count, a certificate
// issued under a compromised root key would keep working after rotation,
// which would make rotation pointless.
//
// Layout, all integers big-endian:
//
//	uint32(len(root_address))  || root_address
//	uint32(len(device_id))     || device_id
//	uint32(len(device_pubkey)) || device_pubkey
//	uint32(device_key_type)
//	uint32(rotation_count)
//	uint64(expiry_height as unsigned two's complement)
func CanonicalCertificateBytes(rootAddress string, c DeviceCertificate) []byte {
	addr := []byte(rootAddress)
	id := []byte(c.DeviceId)

	out := make([]byte, 0, 4+len(addr)+4+len(id)+4+len(c.DevicePubkey)+4+4+8)

	out = binary.BigEndian.AppendUint32(out, uint32(len(addr)))
	out = append(out, addr...)

	out = binary.BigEndian.AppendUint32(out, uint32(len(id)))
	out = append(out, id...)

	out = binary.BigEndian.AppendUint32(out, uint32(len(c.DevicePubkey)))
	out = append(out, c.DevicePubkey...)

	out = binary.BigEndian.AppendUint32(out, uint32(c.DeviceKeyType))
	out = binary.BigEndian.AppendUint32(out, c.RotationCount)
	//nolint:gosec // two's-complement round trip is intentional and exact
	out = binary.BigEndian.AppendUint64(out, uint64(c.ExpiryHeight))

	return out
}

// CanonicalRotationBytes returns the bytes the outgoing root key signs to
// authorise its own replacement.
//
// Layout, all integers big-endian:
//
//	uint32(len(address))          || address
//	uint32(len(new_root_pubkey))  || new_root_pubkey
//	uint32(new_root_key_type)
//	uint32(current_rotation_count)
//
// The current rotation count is included so that a rotation signature cannot
// be replayed to undo a later rotation.
func CanonicalRotationBytes(address string, newPubkey []byte, kt KeyType, currentRotation uint32) []byte {
	addr := []byte(address)

	out := make([]byte, 0, 4+len(addr)+4+len(newPubkey)+4+4)

	out = binary.BigEndian.AppendUint32(out, uint32(len(addr)))
	out = append(out, addr...)

	out = binary.BigEndian.AppendUint32(out, uint32(len(newPubkey)))
	out = append(out, newPubkey...)

	out = binary.BigEndian.AppendUint32(out, uint32(kt))
	out = binary.BigEndian.AppendUint32(out, currentRotation)

	return out
}

// deviceIDPattern restricts device ids to a conservative character set.
//
// Device ids appear in store keys and in client user interfaces. Allowing
// arbitrary bytes would put unvalidated client input into key material and
// into anything that renders it.
var deviceIDPattern = regexp.MustCompile(`^[a-zA-Z0-9_.:-]+$`)

// ValidateDeviceID checks a client-supplied device id.
func ValidateDeviceID(id string, maxLen uint32) error {
	if id == "" {
		return ErrInvalidDeviceID.Wrap("device id must not be empty")
	}
	if maxLen > 0 && uint32(len(id)) > maxLen {
		return ErrInvalidDeviceID.Wrapf("device id is %d bytes, maximum is %d", len(id), maxLen)
	}
	if !deviceIDPattern.MatchString(id) {
		return ErrInvalidDeviceID.Wrapf(
			"device id %q contains characters outside [a-zA-Z0-9_.:-]", id)
	}
	return nil
}

// ValidateBasic performs stateless validation of a device certificate.
func (c DeviceCertificate) ValidateBasic(maxDeviceIDLen uint32) error {
	if err := ValidateDeviceID(c.DeviceId, maxDeviceIDLen); err != nil {
		return err
	}
	if _, err := PubKeyFromBytes(c.DevicePubkey, c.DeviceKeyType); err != nil {
		return err
	}
	if c.ExpiryHeight <= 0 {
		return ErrInvalidCertificate.Wrap("expiry_height must be positive")
	}
	if len(c.Signature) != 64 {
		return ErrInvalidCertificate.Wrapf(
			"signature is %d bytes, expected 64", len(c.Signature))
	}
	return nil
}

// Validate checks a recovery configuration.
func (r RecoveryConfig) Validate(maxGuardians uint32, minDelay int64) error {
	if uint32(len(r.Guardians)) > maxGuardians {
		return ErrInvalidRecovery.Wrapf(
			"%d guardians, maximum is %d", len(r.Guardians), maxGuardians)
	}

	seen := make(map[string]bool, len(r.Guardians))
	for i, g := range r.Guardians {
		if _, err := sdk.AccAddressFromBech32(g); err != nil {
			return ErrInvalidRecovery.Wrapf("guardians[%d] %q: %v", i, g, err)
		}
		if seen[g] {
			return ErrInvalidRecovery.Wrapf("guardian %s is listed twice", g)
		}
		seen[g] = true
	}

	if r.Threshold == 0 {
		// Social recovery disabled. A guardian list without a threshold is a
		// configuration mistake worth naming rather than silently ignoring.
		if len(r.Guardians) > 0 {
			return ErrInvalidRecovery.Wrap(
				"guardians are configured but threshold is 0; social recovery would never trigger")
		}
		return nil
	}

	if uint32(len(r.Guardians)) < r.Threshold {
		return ErrInvalidRecovery.Wrapf(
			"threshold %d exceeds the %d configured guardians; recovery would be impossible",
			r.Threshold, len(r.Guardians))
	}

	if r.RecoveryDelayBlocks < minDelay {
		return ErrInvalidRecovery.Wrapf(
			"recovery_delay_blocks %d is below the protocol minimum of %d; the delay is the "+
				"window in which a user whose guardians were socially engineered can notice and cancel",
			r.RecoveryDelayBlocks, minDelay)
	}

	if len(r.RecoveryHash) != 0 && len(r.RecoveryHash) != 32 {
		return ErrInvalidRecovery.Wrapf(
			"recovery_hash is %d bytes, expected 0 or 32", len(r.RecoveryHash))
	}

	return nil
}

// IsGuardian reports whether an address is a guardian of this configuration.
func (r RecoveryConfig) IsGuardian(addr string) bool {
	for _, g := range r.Guardians {
		if g == addr {
			return true
		}
	}
	return false
}

// HasApproved reports whether a guardian has already approved a request.
func (r RecoveryRequest) HasApproved(guardian string) bool {
	for _, a := range r.Approvals {
		if a == guardian {
			return true
		}
	}
	return false
}

// ValidateLabel checks a client-supplied device label.
func ValidateLabel(label string, maxLen uint32) error {
	if maxLen > 0 && uint32(len(label)) > maxLen {
		return ErrInvalidDeviceID.Wrapf(
			"device label is %d bytes, maximum is %d", len(label), maxLen)
	}
	if strings.ContainsAny(label, "\x00\n\r") {
		return ErrInvalidDeviceID.Wrap("device label may not contain control characters")
	}
	return nil
}

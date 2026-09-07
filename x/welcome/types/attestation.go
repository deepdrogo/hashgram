package types

import (
	"github.com/cosmos/cosmos-sdk/crypto/keys/ed25519"
	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	cryptotypes "github.com/cosmos/cosmos-sdk/crypto/types"
	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/app/canonical"
)

// CanonicalAttestationBytes returns the exact bytes an attestor signs.
//
// Protobuf is not a canonical encoding: field ordering, varint padding and
// unknown fields all admit multiple encodings of the same message, and a
// signature over "whatever proto produced" is a signature over something the
// verifier may not reproduce. So the signed preimage is built by hand.
//
// Every variable-length field carries a length prefix. Without them,
// (method="ab", extra="c") and (method="a", extra="bc") would produce
// identical bytes and a signature for one would verify for the other.
//
// Layout, all integers big-endian:
//
//	uint32(len(subject))  || subject
//	uint32(len(attestor)) || attestor
//	uint64(nonce)
//	uint64(expiry_height as unsigned two's complement)
//	uint32(len(method))   || method
//	uint32(confidence)
//
// The signature field itself is excluded, obviously.
//
// This is then wrapped by the network signing domain (see
// x/network keeper.SigningDigest with PurposeEligibility), which prevents an
// attestation from one Hashgram network being replayed on another and
// prevents these bytes being reinterpreted as some other kind of signed
// object.
//
// Returns an error when a field cannot be length-prefixed unambiguously. See
// app/canonical for why that is an error rather than a silent truncation.
func CanonicalAttestationBytes(a EligibilityAttestation) ([]byte, error) {
	return canonical.New(4+len(a.Subject)+4+len(a.Attestor)+8+8+4+len(a.Method)+4).
		String("subject", a.Subject).
		String("attestor", a.Attestor).
		Uint64(a.Nonce).
		Height(a.ExpiryHeight).
		String("method", a.Method).
		Uint32(a.Confidence).
		Finish()
}

// ValidateBasic performs stateless validation of an attestation.
func (a EligibilityAttestation) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(a.Subject); err != nil {
		return ErrInvalidAttestation.Wrapf("subject %q: %v", a.Subject, err)
	}
	if _, err := sdk.AccAddressFromBech32(a.Attestor); err != nil {
		return ErrInvalidAttestation.Wrapf("attestor %q: %v", a.Attestor, err)
	}
	if a.Method == "" {
		return ErrInvalidAttestation.Wrap("method must not be empty")
	}
	if len(a.Method) > 64 {
		return ErrInvalidAttestation.Wrapf("method is %d bytes, maximum is 64", len(a.Method))
	}
	if a.ExpiryHeight <= 0 {
		return ErrInvalidAttestation.Wrap("expiry_height must be positive")
	}
	if a.Confidence > BasisPointsMax {
		return ErrInvalidAttestation.Wrapf(
			"confidence %d exceeds %d basis points", a.Confidence, BasisPointsMax)
	}
	if len(a.Signature) == 0 {
		return ErrInvalidSignature.Wrap("signature must not be empty")
	}
	// 64 bytes for both secp256k1 (r||s) and ed25519 (R||s).
	if len(a.Signature) != 64 {
		return ErrInvalidSignature.Wrapf(
			"signature is %d bytes, expected 64", len(a.Signature))
	}
	return nil
}

// BasisPointsMax is 100% expressed in basis points.
const BasisPointsMax uint32 = 10_000

// PubKey reconstructs an attestor's public key from its registration.
func (at Attestor) PubKey() (cryptotypes.PubKey, error) {
	switch at.KeyType {
	case KEY_TYPE_SECP256K1:
		if len(at.Pubkey) != secp256k1.PubKeySize {
			return nil, ErrInvalidAttestor.Wrapf(
				"secp256k1 pubkey is %d bytes, expected %d", len(at.Pubkey), secp256k1.PubKeySize)
		}
		return &secp256k1.PubKey{Key: at.Pubkey}, nil

	case KEY_TYPE_ED25519:
		if len(at.Pubkey) != ed25519.PubKeySize {
			return nil, ErrInvalidAttestor.Wrapf(
				"ed25519 pubkey is %d bytes, expected %d", len(at.Pubkey), ed25519.PubKeySize)
		}
		return &ed25519.PubKey{Key: at.Pubkey}, nil

	default:
		return nil, ErrInvalidAttestor.Wrapf("unsupported key type %s", at.KeyType)
	}
}

// Validate checks an attestor registration.
//
// The address must be the one derived from the public key. Allowing them to
// diverge would let governance register a key under someone else's address,
// and would make the on-chain record misleading about who can actually sign.
func (at Attestor) Validate() error {
	if at.Name == "" {
		return ErrInvalidAttestor.Wrap("name must not be empty")
	}
	if at.Method == "" {
		return ErrInvalidAttestor.Wrap("method must not be empty")
	}
	if at.MaxClaimsPerEpoch == 0 {
		return ErrInvalidAttestor.Wrap(
			"max_claims_per_epoch must be positive; an uncapped attestor has unbounded blast radius")
	}

	pk, err := at.PubKey()
	if err != nil {
		return err
	}

	declared, err := sdk.AccAddressFromBech32(at.Address)
	if err != nil {
		return ErrInvalidAttestor.Wrapf("address %q: %v", at.Address, err)
	}
	derived := sdk.AccAddress(pk.Address())
	if !declared.Equals(derived) {
		return ErrInvalidAttestor.Wrapf(
			"address %s does not match the address derived from pubkey (%s)",
			at.Address, derived)
	}

	return nil
}

// VerifySignature checks an attestation signature against a digest.
func (at Attestor) VerifySignature(digest []byte, signature []byte) error {
	pk, err := at.PubKey()
	if err != nil {
		return err
	}
	if !pk.VerifySignature(digest, signature) {
		return ErrInvalidSignature.Wrapf("signature does not verify for attestor %s", at.Address)
	}
	return nil
}

// Package safety is the Hashgram Safety Engine: a pipeline that examines
// PUBLIC content and signs verdicts about it.
//
// What it can see: social events and the public media they reference, read
// from the co-located node's loopback API. What it cannot see: anything
// end-to-end encrypted. There is no code path here that touches a mailbox
// envelope, and the systemd unit removes the node's data directory from the
// engine's view of the filesystem, so the separation is enforced rather
// than promised.
//
// Verdicts are signed with the engine's own attestor key and published to
// the network as ContentAttestation messages. Nodes and indexers that trust
// this attestor enforce them; nobody has to. An attestation can name only
// what is public (a CID, an event id, a public media hash), which is what
// keeps the mechanism from ever reaching into private conversations.
package safety

import (
	"crypto/ed25519"
	"fmt"

	"github.com/hashgram/hashgram/app/canonical"
	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/pkg/p2ppb"
)

// AttestationPayload mirrors hashgram-proto's `attestation_payload`.
func AttestationPayload(a *p2ppb.ContentAttestation) ([]byte, error) {
	return canonical.New(256).
		String("network_id", a.NetworkId).
		Uint32(a.Version).
		Bytes("cid", a.Cid).
		Bytes("event_id", a.EventId).
		Bytes("content_hash", a.ContentHash).
		Enum(int32(a.Verdict)).
		String("policy", a.Policy).
		String("reason_code", a.ReasonCode).
		Uint64(a.Timestamp).
		Bytes("attestor_pubkey", a.AttestorPubkey).
		Finish()
}

// Sign fills network, version, attestor key and signature.
func Sign(id hgparams.NetworkIdentity, key ed25519.PrivateKey, a *p2ppb.ContentAttestation) error {
	a.NetworkId = id.NetworkID
	a.Version = 1
	a.AttestorPubkey = key.Public().(ed25519.PublicKey)
	payload, err := AttestationPayload(a)
	if err != nil {
		return err
	}
	digest := id.SigningDigest(hgparams.PurposeContentAttest, payload)
	a.Signature = ed25519.Sign(key, digest[:])
	return nil
}

// Verify checks an attestation's signature.
func Verify(id hgparams.NetworkIdentity, a *p2ppb.ContentAttestation) error {
	if a.NetworkId != id.NetworkID {
		return fmt.Errorf("attestation is for network %q", a.NetworkId)
	}
	payload, err := AttestationPayload(a)
	if err != nil {
		return err
	}
	digest := id.SigningDigest(hgparams.PurposeContentAttest, payload)
	if len(a.AttestorPubkey) != ed25519.PublicKeySize {
		return fmt.Errorf("attestor key is %d bytes", len(a.AttestorPubkey))
	}
	if !ed25519.Verify(ed25519.PublicKey(a.AttestorPubkey), digest[:], a.Signature) {
		return fmt.Errorf("signature does not verify")
	}
	return nil
}

package types

import (
	hgparams "github.com/hashgram/hashgram/app/params"
)

// SigningPurposes is the closed set of signing purposes the protocol defines.
//
// It is a closed set on purpose. A client that asks for the domain of an
// unrecognised purpose gets an error rather than a plausible-looking string,
// because silently minting a new domain would let two components disagree
// about what they are signing while both believing they succeeded.
func SigningPurposes() []hgparams.SigningPurpose {
	return []hgparams.SigningPurpose{
		hgparams.PurposeSocialEvent,
		hgparams.PurposeDeviceCert,
		hgparams.PurposeServiceReceipt,
		hgparams.PurposeStorageChallenge,
		hgparams.PurposeEligibility,
		hgparams.PurposeContentAttest,
		hgparams.PurposeBootstrapRecord,
		hgparams.PurposePeerHandshake,
		hgparams.PurposeNodeAnnounce,
	}
}

// ParseSigningPurpose validates a purpose string against the closed set.
func ParseSigningPurpose(s string) (hgparams.SigningPurpose, error) {
	for _, p := range SigningPurposes() {
		if string(p) == s {
			return p, nil
		}
	}
	return "", ErrUnknownSigningPurpose.Wrapf(
		"%q is not a protocol signing purpose; valid purposes: %v", s, SigningPurposeStrings())
}

// SigningPurposeStrings returns the purpose names as strings, for error
// messages and CLI help.
func SigningPurposeStrings() []string {
	ps := SigningPurposes()
	out := make([]string, len(ps))
	for i, p := range ps {
		out[i] = string(p)
	}
	return out
}

// PreimageLayout documents the exact byte layout that is hashed before
// signing. It is returned by the SigningDomain query so that an SDK author in
// another language does not have to infer it from Go code.
const PreimageLayout = "magic(4) || uint32be(len(domain)) || domain || uint32be(len(payload)) || payload"

// Command signing-vectors emits the canonical signing preimages and digests
// for every network and purpose, as JSON.
//
//	go run ./tools/signing-vectors > node/testdata/signing-vectors.json
//
// These vectors are the contract between implementations. The Rust node, and
// every client SDK, must reproduce them byte for byte, because a signature is
// only verifiable if both sides build the same preimage from the same inputs.
//
// Generating them from the Go implementation rather than writing them by hand
// means the Rust tests check against what the chain actually does, not against
// a second reading of the specification. If the Go framing changes, these
// vectors change, and the Rust tests fail — which is the intended behaviour:
// a framing change is a consensus and signature break that both sides must
// adopt deliberately.
package main

import (
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"

	"github.com/hashgram/hashgram/app/params"
)

type vector struct {
	Purpose  string `json:"purpose"`
	Domain   string `json:"domain"`
	Payload  string `json:"payload_hex"`
	Preimage string `json:"preimage_hex"`
	Digest   string `json:"digest_hex"`
}

type identityVectors struct {
	NetworkName          string   `json:"network_name"`
	NetworkID            string   `json:"network_id"`
	ChainID              string   `json:"chain_id"`
	NetworkMagic         string   `json:"network_magic"`
	MagicHex             string   `json:"network_magic_hex"`
	ProtocolMajorVersion uint32   `json:"protocol_major_version"`
	GenesisHash          string   `json:"genesis_hash"`
	Vectors              []vector `json:"vectors"`
}

// purposes is listed explicitly rather than derived, so that adding a purpose
// requires touching this file and therefore regenerating the vectors.
var purposes = []params.SigningPurpose{
	params.PurposeSocialEvent,
	params.PurposeDeviceCert,
	params.PurposeServiceReceipt,
	params.PurposeStorageChallenge,
	params.PurposeEligibility,
	params.PurposeContentAttest,
	params.PurposeBootstrapRecord,
	params.PurposePeerHandshake,
	params.PurposeNodeAnnounce,
}

// payloads exercise the cases that have historically broken framing:
// an empty payload, an ordinary one, two whose lengths differ by one so that
// a missing length prefix would collide, and one with high and low bytes to
// catch a sign-extension mistake.
var payloads = [][]byte{
	{},
	[]byte("payload"),
	[]byte("ab"),
	[]byte("abc"),
	{0x00, 0xff, 0x7f, 0x80},
}

func build(id params.NetworkIdentity) identityVectors {
	out := identityVectors{
		NetworkName:          id.NetworkName,
		NetworkID:            id.NetworkID,
		ChainID:              id.ChainID,
		NetworkMagic:         string(id.NetworkMagic[:]),
		MagicHex:             hex.EncodeToString(id.NetworkMagic[:]),
		ProtocolMajorVersion: id.ProtocolMajorVersion,
		GenesisHash:          id.GenesisHash,
	}

	for _, p := range purposes {
		for _, payload := range payloads {
			preimage := id.SigningPreimage(p, payload)
			digest := id.SigningDigest(p, payload)
			out.Vectors = append(out.Vectors, vector{
				Purpose:  string(p),
				Domain:   id.SigningDomain(p),
				Payload:  hex.EncodeToString(payload),
				Preimage: hex.EncodeToString(preimage),
				Digest:   hex.EncodeToString(digest[:]),
			})
		}
	}
	return out
}

func main() {
	// A fixed genesis hash, so the vectors are stable across runs. It is a
	// real devnet hash rather than a placeholder, so the length and character
	// set are what an implementation will actually receive.
	const genesisHash = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287"

	all := map[string]identityVectors{
		"mainnet": build(params.MainnetIdentity(genesisHash)),
		"devnet":  build(params.DevnetIdentity(genesisHash)),
	}

	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	if err := enc.Encode(all); err != nil {
		fmt.Fprintln(os.Stderr, "encoding vectors:", err)
		os.Exit(1)
	}
}

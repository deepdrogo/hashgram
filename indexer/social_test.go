package indexer

import (
	"bytes"
	"crypto/ed25519"
	"encoding/binary"
	"testing"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/pkg/p2ppb"
)

const testGenesis = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287"

// TestEventPayloadLayout pins the canonical layout to the one hashgram-proto
// (Rust) builds: every field but id and signature, tag order, u32 length
// prefixes, media count then each reference.
func TestEventPayloadLayout(t *testing.T) {
	ev := &p2ppb.SocialEvent{
		NetworkId:     "hashgram-devnet",
		Version:       1,
		Type:          "POST_CREATE",
		Author:        "hash1alice",
		DevicePubkey:  bytes.Repeat([]byte{2}, 32),
		Timestamp:     1700000000,
		Sequence:      3,
		PreviousEvent: bytes.Repeat([]byte{7}, 32),
		Payload:       []byte("hello"),
		Media: []*p2ppb.MediaReference{{
			Cid: bytes.Repeat([]byte{1}, 32), Mime: "image/png", Size: 10, Kind: "image",
		}},
	}
	got, err := EventPayloadBytes(ev)
	if err != nil {
		t.Fatal(err)
	}
	var want bytes.Buffer
	str := func(s string) {
		_ = binary.Write(&want, binary.BigEndian, uint32(len(s)))
		want.WriteString(s)
	}
	byt := func(b []byte) {
		_ = binary.Write(&want, binary.BigEndian, uint32(len(b)))
		want.Write(b)
	}
	u32 := func(v uint32) { _ = binary.Write(&want, binary.BigEndian, v) }
	u64 := func(v uint64) { _ = binary.Write(&want, binary.BigEndian, v) }
	str("hashgram-devnet")
	u32(1)
	str("POST_CREATE")
	str("hash1alice")
	byt(ev.DevicePubkey)
	u64(1700000000)
	u64(3)
	byt(ev.PreviousEvent)
	byt([]byte("hello"))
	u32(1) // media count
	byt(ev.Media[0].Cid)
	str("image/png")
	u64(10)
	str("image")
	u32(0)
	u32(0)
	u32(0)
	byt(nil)
	byt(nil)
	if !bytes.Equal(got, want.Bytes()) {
		t.Fatalf("layout drift\n got %x\nwant %x", got, want.Bytes())
	}
}

// TestVerifyEventRefusesForgeries: a valid event verifies; a relay that
// changes a byte, swaps the network or re-signs with its own key is refused.
func TestVerifyEventRefusesForgeries(t *testing.T) {
	id := hgparams.DevnetIdentity(testGenesis)
	s := &SocialIngester{identity: id}
	pub, priv, _ := ed25519.GenerateKey(nil)
	ev := &p2ppb.SocialEvent{
		NetworkId: "hashgram-devnet", Version: 1, Type: "FOLLOW", Author: "hash1alice",
		DevicePubkey: pub, Timestamp: 1700000000, Sequence: 0, Payload: []byte("x"),
	}
	payload, _ := EventPayloadBytes(ev)
	ev.Id = blake3Sum(id.SigningPreimage(hgparams.PurposeSocialEvent, payload))
	digest := id.SigningDigest(hgparams.PurposeSocialEvent, payload)
	ev.Signature = ed25519.Sign(priv, digest[:])
	if err := s.VerifyEvent(ev); err != nil {
		t.Fatalf("valid event refused: %v", err)
	}

	tampered := *ev
	tampered.Payload = []byte("y")
	if s.VerifyEvent(&tampered) == nil {
		t.Fatal("tampered payload accepted")
	}
	fork := *ev
	fork.NetworkId = "hashgram-mainnet"
	if s.VerifyEvent(&fork) == nil {
		t.Fatal("foreign-network event accepted")
	}
	relayPub, relayPriv, _ := ed25519.GenerateKey(nil)
	resigned := *ev
	resigned.Signature = ed25519.Sign(relayPriv, digest[:])
	if s.VerifyEvent(&resigned) == nil {
		t.Fatal("relay signature accepted for the author's device key")
	}
	resigned.DevicePubkey = relayPub
	// With the relay's own key the id no longer matches the content.
	if s.VerifyEvent(&resigned) == nil {
		t.Fatal("relay re-keyed event accepted")
	}
}

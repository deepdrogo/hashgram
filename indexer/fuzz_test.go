package indexer

import (
	"encoding/json"
	"testing"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// FuzzNodeEvent feeds arbitrary node-API JSON through decoding and
// verification. The indexer trusts no node; whatever comes back must be
// refused or accepted without panicking.
func FuzzNodeEvent(f *testing.F) {
	f.Add(`{"id":"00","type":"POST_CREATE","author":"hash1x","device_pubkey":"00","timestamp":1,"sequence":0,"payload":"","signature":"00","network_id":"hashgram-devnet","version":1}`)
	f.Add(`{"id":"zz"}`)
	f.Add(`[]`)
	s := &SocialIngester{identity: hgparams.DevnetIdentity(testGenesis)}
	f.Fuzz(func(t *testing.T, raw string) {
		var n nodeEvent
		if json.Unmarshal([]byte(raw), &n) != nil {
			return
		}
		ev, err := fromNode(n)
		if err != nil {
			return
		}
		_ = s.VerifyEvent(ev)
		_, _ = EventPayloadBytes(ev)
	})
}

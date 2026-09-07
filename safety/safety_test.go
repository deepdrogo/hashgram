package safety

import (
	"context"
	"crypto/ed25519"
	"os"
	"path/filepath"
	"testing"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/pkg/p2ppb"
)

const testGenesis = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287"

func TestSignVerifyAndTamper(t *testing.T) {
	id := hgparams.DevnetIdentity(testGenesis)
	_, key, _ := ed25519.GenerateKey(nil)
	a := &p2ppb.ContentAttestation{Cid: make([]byte, 32), Verdict: p2ppb.Verdict_CONTENT_BLOCK, Policy: "p", ReasonCode: "spam", Timestamp: 1}
	if err := Sign(id, key, a); err != nil {
		t.Fatal(err)
	}
	if err := Verify(id, a); err != nil {
		t.Fatal(err)
	}
	a.Verdict = p2ppb.Verdict_CONTENT_ALLOW
	if Verify(id, a) == nil {
		t.Fatal("verdict flip accepted")
	}
	a.Verdict = p2ppb.Verdict_CONTENT_BLOCK
	if Verify(hgparams.MainnetIdentity(testGenesis), a) == nil {
		t.Fatal("devnet attestation accepted on mainnet identity")
	}
}

func TestPipelineTakesMostSevereAndSanitises(t *testing.T) {
	dir := t.TempDir()
	rules := filepath.Join(dir, "rules.json")
	os.WriteFile(rules, []byte(`[{"pattern":"airdrop","verdict":"BLOCK","reason":"scam"},{"pattern":"nsfw","verdict":"RESTRICT","reason":"adult"}]`), 0o600)
	hl := filepath.Join(dir, "hashes.txt")
	bad := ContentHash([]byte("bad image"))
	os.WriteFile(hl, []byte("# list\n"+bad+" csam-hash-match\n"), 0o600)

	tr, err := NewTextRules(rules)
	if err != nil {
		t.Fatal(err)
	}
	h, err := NewHashList(hl)
	if err != nil {
		t.Fatal(err)
	}
	p := &Pipeline{Stages: []Stage{h, tr}}

	item := &Item{EventID: "e1", Text: "nsfw airdrop", Media: []MediaItem{{CID: "c1", Data: []byte("bad image"), ContentHash: bad}, {CID: "c2", Data: []byte("fine"), ContentHash: ContentHash([]byte("fine"))}}}
	findings, errs := p.Evaluate(context.Background(), item)
	if len(errs) != 0 {
		t.Fatal(errs)
	}
	var sawBlockText, sawRestrict, sawHash bool
	for _, f := range findings {
		switch {
		case f.Stage == "text-rules" && f.Verdict == Block:
			sawBlockText = true
		case f.Stage == "text-rules" && f.Verdict == Restrict:
			sawRestrict = true
		case f.Stage == "hash-list" && f.SubjectCID == "c1" && f.Verdict == Block:
			sawHash = true
		case f.Stage == "hash-list" && f.SubjectCID == "c2":
			t.Fatal("clean media matched the hash list")
		}
	}
	if !sawBlockText || !sawRestrict || !sawHash {
		t.Fatalf("missing findings: %+v", findings)
	}
	if got := sanitizeReason("Contains: the actual TEXT!"); got != "contains-the-actual-text" {
		t.Fatalf("reason not sanitised: %q", got)
	}
	if Block <= Restrict || Restrict <= Quarantine || Quarantine <= Allow {
		t.Fatal("verdict severity order is wrong")
	}
}

func TestKeyIsCreatedOnceWith0600(t *testing.T) {
	dir := t.TempDir()
	a, err := LoadOrCreateKey(dir)
	if err != nil {
		t.Fatal(err)
	}
	b, err := LoadOrCreateKey(dir)
	if err != nil {
		t.Fatal(err)
	}
	if string(a.Public().(ed25519.PublicKey)) != string(b.Public().(ed25519.PublicKey)) {
		t.Fatal("key changed on reload")
	}
	st, _ := os.Stat(filepath.Join(dir, "attestor.key"))
	if st.Mode().Perm() != 0o600 {
		t.Fatalf("key file mode %o", st.Mode().Perm())
	}
}

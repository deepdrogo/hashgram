package safety

import (
	"context"
	"os"
	"path/filepath"
	"testing"
)

// FuzzTextRules runs arbitrary text through the rule stage and the reason
// sanitiser. Content is attacker-controlled; the engine must never panic on
// it and must never let it leak into a reason code unsanitised.
func FuzzTextRules(f *testing.F) {
	dir := f.TempDir()
	rules := filepath.Join(dir, "rules.json")
	_ = os.WriteFile(rules, []byte(`[{"pattern":"a+b","verdict":"BLOCK","reason":"x"},{"pattern":"(?s).*z.*","verdict":"RESTRICT","reason":"y"}]`), 0o600)
	tr, err := NewTextRules(rules)
	if err != nil {
		f.Fatal(err)
	}
	f.Add("hello")
	f.Add("aab z")
	f.Fuzz(func(t *testing.T, text string) {
		_, _ = tr.Evaluate(context.Background(), &Item{Text: text})
		r := sanitizeReason(text)
		for _, c := range r {
			ok := (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '-' || c == '_' || c == '.'
			if !ok {
				t.Fatalf("sanitiser let %q through in %q", c, r)
			}
		}
		_ = ParseVerdict(text)
	})
}

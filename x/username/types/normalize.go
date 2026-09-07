package types

import (
	"fmt"
	"strings"
	"unicode"

	"golang.org/x/text/unicode/norm"
)

// Username normalisation and confusable folding.
//
// The threat this file exists to address: an attacker registers a name that
// renders identically or near-identically to an existing one, and uses it to
// impersonate that user. The classic instance is a Cyrillic "а" (U+0430) in
// place of a Latin "a", which is pixel-identical in most fonts.
//
// Three layers, in order:
//
//  1. Normalise. NFKC plus Unicode-aware lowercasing, so "Alice", "alice"
//     and the fullwidth "ａｌｉｃｅ" are one registration rather than three.
//     NFKC specifically because compatibility decomposition is what collapses
//     fullwidth, circled and ligature forms.
//
//  2. Validate. Reject characters that have no business in an identifier:
//     invisible and format characters, unassigned code points, combining
//     marks in leading position, and anything outside the permitted set.
//     Also reject mixed-script names, which is what defeats the general
//     homograph case: "pаypal" mixing Latin and Cyrillic is refused outright.
//
//  3. Fold to a skeleton. Map the characters that remain confusable *within*
//     a single script to a canonical form, and refuse a registration whose
//     skeleton collides with an existing one. This catches within-script
//     lookalikes such as "rn" for "m" being out of scope, but "l" for "1"
//     and "0" for "o" being in scope.
//
// This is a curated subset of UTS-39 rather than the full confusables table.
// The full table is tens of thousands of entries and changes with each
// Unicode release, which would make it a consensus-breaking dependency: two
// nodes built against different Unicode versions would disagree about which
// names are registrable. A fixed, explicit table is deterministic across
// builds and years, which matters more here than exhaustiveness. The
// remaining gap is documented in docs/PROTOCOL.md.

const (
	// UsernamePrefix is the sigil users type but which is not stored.
	UsernamePrefix = "@"

	// MinNameLength and MaxNameLength are the protocol bounds, in code
	// points. Params may narrow them but not widen them.
	MinNameLength = 2
	MaxNameLength = 32
)

// asciiAllowed is the launch character set: lowercase ASCII letters, digits,
// underscore and hyphen.
//
// A hyphen may not lead or trail, and two hyphens may not be adjacent, both
// because "a--b" and "a-b" are easy to mistake for one another and because a
// leading hyphen makes a name awkward to use in command lines.
func asciiAllowed(r rune) bool {
	return (r >= 'a' && r <= 'z') || (r >= '0' && r <= '9') || r == '_' || r == '-'
}

// Normalize reduces a requested name to its canonical stored form.
//
// It strips an optional leading '@', applies NFKC, lowercases, and trims
// surrounding whitespace. It does not validate; call Validate afterwards.
func Normalize(requested string) string {
	s := strings.TrimSpace(requested)
	s = strings.TrimPrefix(s, UsernamePrefix)
	s = norm.NFKC.String(s)
	s = strings.ToLower(s)
	return s
}

// ValidationError describes why a name is unacceptable, using the stable
// reason codes the Availability query reports.
type ValidationError struct {
	Reason string
	Detail string
}

func (e *ValidationError) Error() string {
	if e.Detail == "" {
		return e.Reason
	}
	return e.Reason + ": " + e.Detail
}

// Reason codes. Stable strings, because clients branch on them to produce
// useful messages.
const (
	ReasonTaken              = "taken"
	ReasonReserved           = "reserved"
	ReasonConfusableWith     = "confusable_with"
	ReasonInvalid            = "invalid"
	ReasonTooShort           = "too_short"
	ReasonTooLong            = "too_long"
	ReasonMixedScript        = "mixed_script"
	ReasonNonASCIINotAllowed = "non_ascii_not_allowed"
)

// Validate checks a normalised name against the protocol rules.
func Validate(normalized string, minLen, maxLen uint32, allowNonASCII bool) error {
	runes := []rune(normalized)

	if minLen < MinNameLength {
		minLen = MinNameLength
	}
	if maxLen == 0 || maxLen > MaxNameLength {
		maxLen = MaxNameLength
	}

	if len(runes) < int(minLen) {
		return &ValidationError{ReasonTooShort,
			fmt.Sprintf("%d code points, minimum is %d", len(runes), minLen)}
	}
	if len(runes) > int(maxLen) {
		return &ValidationError{ReasonTooLong,
			fmt.Sprintf("%d code points, maximum is %d", len(runes), maxLen)}
	}

	for i, r := range runes {
		// Invisible and format characters are the single most abusable class:
		// a zero-width joiner lets two different byte strings render
		// identically.
		if unicode.Is(unicode.Cf, r) || unicode.Is(unicode.Cc, r) ||
			unicode.Is(unicode.Co, r) || unicode.Is(unicode.Cs, r) {
			return &ValidationError{ReasonInvalid,
				fmt.Sprintf("code point %d is an invisible, control or private-use character (U+%04X)", i, r)}
		}
		if unicode.IsSpace(r) {
			return &ValidationError{ReasonInvalid, "names may not contain whitespace"}
		}
		// A combining mark cannot lead: it would attach to whatever renders
		// before the name.
		if i == 0 && unicode.Is(unicode.M, r) {
			return &ValidationError{ReasonInvalid, "a name may not begin with a combining mark"}
		}

		if r < 128 {
			if !asciiAllowed(r) {
				return &ValidationError{ReasonInvalid,
					fmt.Sprintf("character %q is not permitted; use a-z, 0-9, underscore or hyphen", r)}
			}
			continue
		}

		if !allowNonASCII {
			return &ValidationError{ReasonNonASCIINotAllowed,
				fmt.Sprintf("character U+%04X is outside the ASCII namespace", r)}
		}
		if !unicode.IsLetter(r) && !unicode.IsDigit(r) {
			return &ValidationError{ReasonInvalid,
				fmt.Sprintf("character U+%04X is neither a letter nor a digit", r)}
		}
	}

	// Hyphen placement.
	if runes[0] == '-' || runes[len(runes)-1] == '-' {
		return &ValidationError{ReasonInvalid, "a name may not begin or end with a hyphen"}
	}
	if strings.Contains(normalized, "--") {
		return &ValidationError{ReasonInvalid,
			"a name may not contain two adjacent hyphens; they are easy to miscount"}
	}

	if allowNonASCII {
		if err := checkSingleScript(runes); err != nil {
			return err
		}
	}

	return nil
}

// scriptsChecked is the set of scripts the mixed-script rule considers.
//
// Common and Inherited are excluded because digits, underscore and hyphen
// belong to Common and legitimately appear alongside any script.
var scriptsChecked = map[string]*unicode.RangeTable{
	"Latin":      unicode.Latin,
	"Cyrillic":   unicode.Cyrillic,
	"Greek":      unicode.Greek,
	"Arabic":     unicode.Arabic,
	"Hebrew":     unicode.Hebrew,
	"Han":        unicode.Han,
	"Hiragana":   unicode.Hiragana,
	"Katakana":   unicode.Katakana,
	"Hangul":     unicode.Hangul,
	"Thai":       unicode.Thai,
	"Devanagari": unicode.Devanagari,
	"Armenian":   unicode.Armenian,
	"Georgian":   unicode.Georgian,
}

// checkSingleScript implements the mixed-script rule.
//
// A name must draw its letters from a single script, with two documented
// exceptions that real languages require:
//
//   - Latin may combine with Han, Hiragana, Katakana or Hangul, because
//     Japanese and Korean text routinely mixes them.
//   - Hiragana, Katakana and Han may combine with each other, for the same
//     reason.
//
// Latin plus Cyrillic, or Latin plus Greek, is refused. That combination has
// no linguistic use and is the vehicle for essentially every practical
// homograph attack.
func checkSingleScript(runes []rune) error {
	found := map[string]bool{}

	for _, r := range runes {
		if r < 128 {
			continue // ASCII letters count as Latin, handled below
		}
		for name, table := range scriptsChecked {
			if unicode.Is(table, r) {
				found[name] = true
				break
			}
		}
	}

	// ASCII letters imply Latin.
	for _, r := range runes {
		if (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') {
			found["Latin"] = true
			break
		}
	}

	if len(found) <= 1 {
		return nil
	}

	names := make([]string, 0, len(found))
	for n := range found {
		names = append(names, n)
	}

	if isPermittedScriptCombination(found) {
		return nil
	}

	return &ValidationError{ReasonMixedScript,
		fmt.Sprintf("a name may not mix scripts (%s); this is the vehicle for homograph impersonation",
			strings.Join(sortedStrings(names), " + "))}
}

// cjkFriendly are the scripts that legitimately appear together, and with
// Latin, in Japanese and Korean text.
var cjkFriendly = map[string]bool{
	"Han": true, "Hiragana": true, "Katakana": true, "Hangul": true,
}

func isPermittedScriptCombination(found map[string]bool) bool {
	for name := range found {
		if name == "Latin" {
			continue
		}
		if !cjkFriendly[name] {
			return false
		}
	}
	return true
}

func sortedStrings(in []string) []string {
	out := append([]string(nil), in...)
	for i := 1; i < len(out); i++ {
		for j := i; j > 0 && out[j] < out[j-1]; j-- {
			out[j], out[j-1] = out[j-1], out[j]
		}
	}
	return out
}

// confusables maps a code point to its canonical lookalike.
//
// A deliberately fixed, curated table rather than the full UTS-39
// confusables data. Reasons:
//
//   - Determinism. The full table changes with every Unicode release. Two
//     validators built against different Unicode versions would disagree
//     about which names are registrable, which is a consensus fault, not a
//     cosmetic difference.
//   - Auditability. Every entry here can be checked by eye.
//
// The table covers the cases that actually appear in impersonation attempts:
// Cyrillic and Greek letters that render as Latin ones, and digits that
// render as letters. Within-script sequence confusables such as "rn" for "m"
// are out of scope and documented as such.
var confusables = map[rune]rune{
	// Digits that read as letters, and vice versa.
	'0': 'o',
	'1': 'l',
	'3': 'e',
	'4': 'a',
	'5': 's',
	'6': 'g',
	'7': 't',
	'8': 'b',
	'9': 'g',
	'i': 'l', // i, l and 1 are mutually confusable in many sans-serif fonts
	'_': '-', // underscore and hyphen are confusable when underlined

	// Cyrillic letters that render as Latin.
	'\u0430': 'a', // а
	'\u0435': 'e', // е
	'\u043E': 'o', // о
	'\u0440': 'p', // р
	'\u0441': 'c', // с
	'\u0443': 'y', // у
	'\u0445': 'x', // х
	'\u0410': 'a', // А
	'\u0412': 'b', // В
	'\u0415': 'e', // Е
	'\u041A': 'k', // К
	'\u041C': 'm', // М
	'\u041D': 'h', // Н
	'\u041E': 'o', // О
	'\u0420': 'p', // Р
	'\u0421': 'c', // С
	'\u0422': 't', // Т
	'\u0425': 'x', // Х
	'\u0456': 'l', // і Ukrainian i
	'\u0458': 'j', // ј
	'\u0455': 's', // ѕ
	'\u04BB': 'h', // һ
	'\u0501': 'd', // ԁ
	'\u051B': 'q', // ԛ
	'\u051D': 'w', // ԝ

	// Greek letters that render as Latin.
	'\u03B1': 'a', // α
	'\u03B2': 'b', // β
	'\u03B5': 'e', // ε
	'\u03B9': 'l', // ι
	'\u03BA': 'k', // κ
	'\u03BD': 'v', // ν
	'\u03BF': 'o', // ο
	'\u03C1': 'p', // ρ
	'\u03C3': 'o', // σ
	'\u03C5': 'u', // υ
	'\u03C7': 'x', // χ
	'\u0391': 'a', // Α
	'\u0392': 'b', // Β
	'\u0395': 'e', // Ε
	'\u0397': 'h', // Η
	'\u0399': 'l', // Ι
	'\u039A': 'k', // Κ
	'\u039C': 'm', // Μ
	'\u039D': 'n', // Ν
	'\u039F': 'o', // Ο
	'\u03A1': 'p', // Ρ
	'\u03A4': 't', // Τ
	'\u03A5': 'y', // Υ
	'\u03A7': 'x', // Χ

	// Armenian and Cherokee letters with Latin lookalikes.
	'\u0570': 'h', // հ
	'\u0585': 'o', // օ
	'\u13A0': 'd', // Ꭰ
	'\u13C0': 'g', // Ꮐ
}

// Skeleton returns the confusable-folded form of a normalised name.
//
// Two names with the same skeleton render similarly enough that only the
// first may be registered. The fold is applied after normalisation, so it
// operates on lowercase NFKC text.
//
// Repeated characters are not collapsed: "aa" and "a" are different names,
// and collapsing them would deny far more legitimate names than it protects.
func Skeleton(normalized string) string {
	var b strings.Builder
	b.Grow(len(normalized))

	for _, r := range normalized {
		if folded, ok := confusables[r]; ok {
			b.WriteRune(folded)
			continue
		}
		b.WriteRune(r)
	}
	return b.String()
}

// ConfusableWith reports whether two names fold to the same skeleton.
func ConfusableWith(a, b string) bool {
	return Skeleton(Normalize(a)) == Skeleton(Normalize(b))
}

// DefaultReservedNames are names that must not be registrable, because they
// would let a holder impersonate the project, its infrastructure or a system
// function.
//
// Governance can extend the list. It cannot practically shrink it below this
// set without a governance proposal that says so explicitly, which is the
// point.
func DefaultReservedNames() []string {
	return []string{
		// The project and its variants.
		"hashgram", "hashgramio", "hashgramcore", "hashgramteam",
		"hashgramsupport", "hashgramofficial", "hash", "hashcoin",
		// Roles that imply authority.
		"admin", "administrator", "root", "system", "official", "support",
		"help", "helpdesk", "security", "moderator", "mod", "staff", "team",
		"founder", "ceo", "owner",
		// Infrastructure and protocol terms whose misuse would be confusing.
		"validator", "node", "relay", "bootstrap", "genesis", "treasury",
		"wallet", "faucet", "bridge", "api", "rpc", "www", "mail", "ftp",
		// Anti-phishing.
		"verify", "verification", "recovery", "restore", "seed", "mnemonic",
		"password", "login", "signin", "airdrop", "giveaway", "claim",
		// Reserved for future protocol use.
		"me", "you", "all", "everyone", "here", "channel", "null", "none",
		"anonymous", "deleted",
	}
}

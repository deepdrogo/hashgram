package types_test

import (
	"os"
	"testing"

	"github.com/stretchr/testify/require"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/username/types"
)

func TestMain(m *testing.M) {
	hgparams.SetSDKConfig()
	os.Exit(m.Run())
}

// TestNormalizeCollapsesEquivalentForms: case variants and fullwidth forms
// must be one registration rather than several, or "@Alice" and "@alice"
// would be different users.
func TestNormalizeCollapsesEquivalentForms(t *testing.T) {
	for _, in := range []string{
		"alice", "Alice", "ALICE", "@alice", "@Alice", " @Alice ",
		"ａｌｉｃｅ", // fullwidth, collapsed by NFKC
		"ALICE\u200b"[:5],
	} {
		require.Equal(t, "alice", types.Normalize(in),
			"%q did not normalise to alice", in)
	}
}

func TestNormalizeStripsTheSigil(t *testing.T) {
	require.Equal(t, "bob", types.Normalize("@bob"))
	require.Equal(t, "bob", types.Normalize("bob"))
	// Only one sigil is stripped; a second is a character the validator will
	// reject rather than something to silently discard.
	require.Equal(t, "@bob", types.Normalize("@@bob"))
}

// ---------------------------------------------------------------------------
// The homograph attack
// ---------------------------------------------------------------------------

// TestCyrillicHomographFoldsToLatin is the attack this file exists to stop.
// Cyrillic U+0430 renders identically to Latin "a" in most fonts.
func TestCyrillicHomographFoldsToLatin(t *testing.T) {
	latin := "paypal"
	cyrillic := "p\u0430ypal" // Cyrillic а in place of Latin a

	require.NotEqual(t, latin, cyrillic, "the two strings must differ byte-wise")
	require.True(t, types.ConfusableWith(latin, cyrillic),
		"%q and %q were not detected as confusable", latin, cyrillic)
}

// TestMixedScriptIsRejectedOutright: Latin combined with Cyrillic has no
// linguistic use and is the vehicle for essentially every practical homograph
// attack, so it is refused before folding even matters.
func TestMixedScriptIsRejectedOutright(t *testing.T) {
	for _, name := range []string{
		"p\u0430ypal",    // Latin + Cyrillic
		"g\u03bfogle",    // Latin + Greek omicron
		"micr\u043esoft", // Latin + Cyrillic о
	} {
		err := types.Validate(types.Normalize(name), 3, 32, true)
		require.Error(t, err, "mixed-script name %q was accepted", name)

		var ve *types.ValidationError
		require.ErrorAs(t, err, &ve)
		require.Equal(t, types.ReasonMixedScript, ve.Reason,
			"%q was rejected for %q rather than mixed script", name, ve.Reason)
	}
}

// TestSingleScriptNonLatinIsAccepted: the mixed-script rule must not exclude
// people who write in one non-Latin script.
func TestSingleScriptNonLatinIsAccepted(t *testing.T) {
	for _, name := range []string{
		"\u043f\u0440\u0438\u0432\u0435\u0442",             // привет, all Cyrillic
		"\u03b3\u03b5\u03b9\u03b1\u03c3\u03b1\u03c2",       // γειασας, all Greek
		"\u10d2\u10d0\u10db\u10d0\u10e0\u10ef\u10dd\u10d1", // Georgian
	} {
		require.NoError(t, types.Validate(types.Normalize(name), 3, 32, true),
			"single-script name %q was rejected", name)
	}
}

// TestCJKWithLatinIsAccepted: Japanese and Korean text routinely mixes Latin
// with Han, Hiragana, Katakana and Hangul, so that combination is permitted.
func TestCJKWithLatinIsAccepted(t *testing.T) {
	for _, name := range []string{
		"tokyo\u6771\u4eac", // Latin + Han
		"\u30ab\u30bftest",  // Katakana + Latin
		"\ud55c\uad6dkorea", // Hangul + Latin
	} {
		require.NoError(t, types.Validate(types.Normalize(name), 3, 32, true),
			"CJK-with-Latin name %q was rejected", name)
	}
}

// TestDigitLetterConfusablesFold: within-script lookalikes such as 0/o and
// 1/l are the other practical impersonation route.
func TestDigitLetterConfusablesFold(t *testing.T) {
	for _, pair := range [][2]string{
		{"alice", "a1ice"},   // 1 folds to l... but alice has no l; see below
		{"bob", "b0b"},       // 0 folds to o
		{"hello", "he11o"},   // 1 folds to l
		{"pool", "p00l"},     // 0 folds to o
		{"sales", "5ales"},   // 5 folds to s
		{"tester", "7ester"}, // 7 folds to t
		{"admin", "adm1n"},   // 1 folds to l... i also folds to l
	} {
		require.True(t, types.ConfusableWith(pair[0], pair[1]),
			"%q and %q were not detected as confusable", pair[0], pair[1])
	}
}

// TestIAndLAndOneAllFold: i, l and 1 are mutually confusable in most
// sans-serif fonts, which is why all three fold to one skeleton.
func TestIAndLAndOneAllFold(t *testing.T) {
	sk := types.Skeleton("il1")
	require.Equal(t, "lll", sk)

	require.True(t, types.ConfusableWith("lily", "1i1y"))
	require.True(t, types.ConfusableWith("lily", "liiy"))
}

// TestDistinctNamesDoNotFoldTogether: the fold must not be so aggressive that
// it denies obviously different names.
func TestDistinctNamesDoNotFoldTogether(t *testing.T) {
	for _, pair := range [][2]string{
		{"alice", "bob"},
		{"alice", "alicia"},
		{"hashgram", "telegram"},
		{"aa", "a"},       // repeated characters are not collapsed
		{"test", "tests"}, // a suffix is a different name
	} {
		require.False(t, types.ConfusableWith(pair[0], pair[1]),
			"%q and %q were wrongly treated as confusable", pair[0], pair[1])
	}
}

// TestInvisibleCharactersAreRejected: a zero-width joiner lets two different
// byte strings render identically, which is the single most abusable class of
// character in an identifier.
func TestInvisibleCharactersAreRejected(t *testing.T) {
	for _, name := range []string{
		"ali\u200bce", // zero-width space
		"ali\u200cce", // zero-width non-joiner
		"ali\u200dce", // zero-width joiner
		"ali\u2060ce", // word joiner
		"ali\ufeffce", // zero-width no-break space
		"ali\u202ece", // right-to-left override
		"ali\u0000ce", // NUL
	} {
		err := types.Validate(types.Normalize(name), 3, 32, true)
		require.Error(t, err, "name containing an invisible character was accepted: %q", name)
	}
}

func TestWhitespaceIsRejected(t *testing.T) {
	for _, name := range []string{"ali ce", "ali\tce", "ali\nce"} {
		require.Error(t, types.Validate(types.Normalize(name), 3, 32, true),
			"name with whitespace was accepted: %q", name)
	}
}

func TestLeadingCombiningMarkIsRejected(t *testing.T) {
	// U+0301 combining acute accent in leading position would attach to
	// whatever renders before the name.
	require.Error(t, types.Validate("\u0301alice", 3, 32, true))
}

// ---------------------------------------------------------------------------
// Character set and length
// ---------------------------------------------------------------------------

func TestASCIIOnlyModeRejectsNonASCII(t *testing.T) {
	err := types.Validate(types.Normalize("\u043f\u0440\u0438\u0432\u0435\u0442"), 3, 32, false)
	require.Error(t, err)

	var ve *types.ValidationError
	require.ErrorAs(t, err, &ve)
	require.Equal(t, types.ReasonNonASCIINotAllowed, ve.Reason)
}

func TestAllowedASCIICharacters(t *testing.T) {
	for _, name := range []string{"alice", "alice_bob", "alice-bob", "alice123", "a_1-b"} {
		require.NoError(t, types.Validate(name, 3, 32, false), "%q was rejected", name)
	}
	for _, name := range []string{"alice!", "alice.bob", "alice@bob", "alice/bob", "alice+bob"} {
		require.Error(t, types.Validate(name, 3, 32, false), "%q was accepted", name)
	}
}

func TestHyphenPlacement(t *testing.T) {
	require.Error(t, types.Validate("-alice", 3, 32, false), "leading hyphen accepted")
	require.Error(t, types.Validate("alice-", 3, 32, false), "trailing hyphen accepted")
	require.Error(t, types.Validate("ali--ce", 3, 32, false), "double hyphen accepted")
	require.NoError(t, types.Validate("ali-ce", 3, 32, false))
}

func TestLengthBounds(t *testing.T) {
	err := types.Validate("ab", 3, 32, false)
	require.Error(t, err)
	var ve *types.ValidationError
	require.ErrorAs(t, err, &ve)
	require.Equal(t, types.ReasonTooShort, ve.Reason)

	long := ""
	for i := 0; i < 33; i++ {
		long += "a"
	}
	err = types.Validate(long, 3, 32, false)
	require.Error(t, err)
	require.ErrorAs(t, err, &ve)
	require.Equal(t, types.ReasonTooLong, ve.Reason)
}

// TestProtocolBoundsOverrideParams: params may narrow the length range but
// not widen it past the protocol limits.
func TestProtocolBoundsOverrideParams(t *testing.T) {
	// Asking for a 1-character minimum still enforces the protocol floor.
	require.Error(t, types.Validate("a", 1, 32, false),
		"a single-character name was accepted despite the protocol floor")

	// Asking for a 100-character maximum still enforces the protocol ceiling.
	long := ""
	for i := 0; i < 40; i++ {
		long += "a"
	}
	require.Error(t, types.Validate(long, 3, 100, false),
		"a 40-character name was accepted despite the protocol ceiling")
}

// ---------------------------------------------------------------------------
// Reserved names
// ---------------------------------------------------------------------------

// TestReservedNamesCannotBeSidesteppedByFolding: reserving "admin" must also
// reserve "adm1n", or the reservation is decorative.
func TestReservedNamesCannotBeSidesteppedByFolding(t *testing.T) {
	p := types.DefaultParams()

	require.True(t, p.IsReserved("admin"))
	require.True(t, p.IsReserved(types.Normalize("adm1n")),
		"the folded form of a reserved name was not reserved")
	require.True(t, p.IsReserved(types.Normalize("supp0rt")))
	require.True(t, p.IsReserved(types.Normalize("h4shgram")))

	require.False(t, p.IsReserved("alice"))
}

func TestReservedNamesCoverImpersonationVectors(t *testing.T) {
	p := types.DefaultParams()
	for _, name := range []string{
		"hashgram", "admin", "support", "security", "founder",
		"validator", "treasury", "recovery", "seed", "airdrop",
	} {
		require.True(t, p.IsReserved(name), "%q should be reserved", name)
	}
}

// ---------------------------------------------------------------------------
// Params and genesis
// ---------------------------------------------------------------------------

func TestDefaultParamsAreValidAndASCIIOnly(t *testing.T) {
	p := types.DefaultParams()
	require.NoError(t, p.Validate())
	require.False(t, p.AllowNonAscii,
		"the launch namespace should be ASCII-only: it has no homograph attacks at all")
	require.False(t, p.RegistrationFee.IsZero())
}

func TestParamsRejectFreeRegistration(t *testing.T) {
	p := types.DefaultParams()
	p.RegistrationFee = nil

	err := p.Validate()
	require.Error(t, err)
	require.Contains(t, err.Error(), "scripts it first")
}

func TestParamsRejectLengthBoundsOutsideProtocolLimits(t *testing.T) {
	p := types.DefaultParams()
	p.MinLength = 1
	require.Error(t, p.Validate(), "a min_length below the protocol floor was accepted")

	p = types.DefaultParams()
	p.MaxLength = 100
	require.Error(t, p.Validate(), "a max_length above the protocol ceiling was accepted")

	p = types.DefaultParams()
	p.MinLength = 20
	p.MaxLength = 10
	require.Error(t, p.Validate())
}

// TestGenesisRejectsTwoConfusableRegistrations: a restored chain must not be
// more permissive than a live one.
func TestGenesisRejectsTwoConfusableRegistrations(t *testing.T) {
	owner := testOwner()
	gs := types.DefaultGenesis()
	gs.Registrations = []types.Registration{
		{Name: "hello", Owner: owner, Skeleton: types.Skeleton("hello"), Transferable: true},
		{Name: "he11o", Owner: owner, Skeleton: types.Skeleton("he11o"), Transferable: true},
	}

	err := gs.Validate()
	require.Error(t, err)
	require.True(t, types.ErrConfusable.Is(err), "got %v", err)
}

func TestGenesisRejectsUnnormalisedNames(t *testing.T) {
	gs := types.DefaultGenesis()
	gs.Registrations = []types.Registration{
		{Name: "Alice", Owner: testOwner(), Skeleton: types.Skeleton("Alice")},
	}
	err := gs.Validate()
	require.Error(t, err)
	require.Contains(t, err.Error(), "normalised form")
}

func TestGenesisRejectsWrongSkeleton(t *testing.T) {
	gs := types.DefaultGenesis()
	gs.Registrations = []types.Registration{
		{Name: "alice", Owner: testOwner(), Skeleton: "wrong"},
	}
	err := gs.Validate()
	require.Error(t, err)
	require.Contains(t, err.Error(), "folds to")
}

func TestGenesisRejectsReservedRegistration(t *testing.T) {
	gs := types.DefaultGenesis()
	gs.Registrations = []types.Registration{
		{Name: "admin", Owner: testOwner(), Skeleton: types.Skeleton("admin")},
	}
	err := gs.Validate()
	require.Error(t, err)
	require.True(t, types.ErrNameReserved.Is(err), "got %v", err)
}

package types

const (
	// ModuleName is the x/welcome module name.
	//
	// It is also the name of the module account that holds the welcome pool.
	// The pool is funded once at genesis out of the Growth allocation and is
	// never topped up: there is no mint, so the programme is bounded by
	// whatever it started with.
	ModuleName = "welcome"

	// StoreKey is the primary store key.
	StoreKey = ModuleName

	// QuerierRoute is the legacy querier route.
	QuerierRoute = ModuleName
)

// Store key prefixes.
var (
	// ParamsKey stores the Params singleton.
	ParamsKey = []byte{0x01}

	// NextSequenceKey stores the next welcome sequence number.
	NextSequenceKey = []byte{0x02}

	// ClaimKey prefixes claim records by subject address.
	ClaimKey = []byte{0x03}

	// UsedNonceKey prefixes consumed (attestor, nonce) pairs. This is what
	// makes an attestation single-use.
	UsedNonceKey = []byte{0x04}

	// AttestorEpochCountKey prefixes per-attestor per-epoch claim counters,
	// which enforce the blast-radius cap on a compromised attestor.
	AttestorEpochCountKey = []byte{0x05}

	// TotalPaidKey stores the cumulative amount paid out.
	TotalPaidKey = []byte{0x06}

	// ClaimsPaidKey stores the count of paid claims.
	ClaimsPaidKey = []byte{0x07}
)

// Event types and attributes.
const (
	EventTypeWelcomeClaimed = "welcome_claimed"
	EventTypeWelcomePaused  = "welcome_paused"

	AttributeKeySubject    = "subject"
	AttributeKeySequence   = "sequence"
	AttributeKeyAmount     = "amount"
	AttributeKeyAttestor   = "attestor"
	AttributeKeyMethod     = "method"
	AttributeKeyConfidence = "confidence"
)

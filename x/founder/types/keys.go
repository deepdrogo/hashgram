package types

const (
	// ModuleName is the x/founder module name.
	//
	// It is also the name of the module account that holds accrued but not
	// yet paid out Founder revenue. That account's balance is visible through
	// an ordinary bank query, so "what is pending?" is verifiable without
	// trusting this module's own bookkeeping.
	ModuleName = "founder"

	// StoreKey is the primary store key.
	StoreKey = ModuleName

	// QuerierRoute is the legacy querier route.
	QuerierRoute = ModuleName
)

// Store key prefixes.
var (
	// ParamsKey stores the Params singleton.
	ParamsKey = []byte{0x01}

	// LedgerKey stores the RevenueLedger singleton.
	LedgerKey = []byte{0x02}

	// BeneficiaryHistoryKey prefixes the append-only beneficiary change log,
	// keyed by sequence number.
	BeneficiaryHistoryKey = []byte{0x03}

	// BeneficiaryHistorySeqKey stores the next history sequence number.
	BeneficiaryHistorySeqKey = []byte{0x04}
)

// Event types and attributes.
const (
	EventTypeRevenueAccrued = "founder_revenue_accrued"
	EventTypeRevenuePaid    = "founder_revenue_paid"
	EventTypeBeneficiarySet = "founder_beneficiary_set"

	AttributeKeyAmount              = "amount"
	AttributeKeyBeneficiary         = "beneficiary"
	AttributeKeyPreviousBeneficiary = "previous_beneficiary"
	AttributeKeySource              = "source"
	AttributeKeyTrigger             = "trigger"

	// TriggerAutomatic marks a payout made by the EndBlocker.
	TriggerAutomatic = "automatic"

	// TriggerClaim marks a payout made by MsgClaimFounderRevenue.
	TriggerClaim = "claim"
)

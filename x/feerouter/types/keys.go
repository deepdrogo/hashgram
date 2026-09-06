package types

const (
	// ModuleName is the x/feerouter module name.
	//
	// It is also the name of the module account used as the revenue pool:
	// explicit protocol service fees are paid into it and split out of it
	// once per block.
	ModuleName = "feerouter"

	// StoreKey is the primary store key.
	StoreKey = ModuleName

	// QuerierRoute is the legacy querier route.
	QuerierRoute = ModuleName
)

// Store key prefixes.
var (
	// ParamsKey stores the Params singleton.
	ParamsKey = []byte{0x01}

	// TotalsKey stores the RevenueTotals singleton.
	TotalsKey = []byte{0x02}

	// ServiceRevenueKey prefixes per-service-kind cumulative revenue.
	ServiceRevenueKey = []byte{0x03}
)

// Event types and attributes.
const (
	EventTypeRevenueRouted = "protocol_revenue_routed"
	EventTypeServiceFee    = "protocol_service_fee"

	AttributeKeyQualifying     = "qualifying"
	AttributeKeyFounderShare   = "founder_share"
	AttributeKeyValidatorShare = "validator_share"
	AttributeKeyServiceKind    = "service_kind"
	AttributeKeyPayer          = "payer"
	AttributeKeyAmount         = "amount"
)

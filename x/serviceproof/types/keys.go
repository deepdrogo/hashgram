package types

const (
	// ModuleName is the x/serviceproof module name.
	//
	// It is also the name of the module account that holds the finite
	// useful-service reward reserve: 500,000,000 HASH funded once at genesis.
	// There is no mint on this chain, so the reserve can only shrink as it is
	// paid out, and grow only when slashed bond is returned to it.
	ModuleName = "serviceproof"

	// BondPoolName is a separate module account holding provider bonds.
	//
	// Deliberately separate from the reserve: mixing posted stake with
	// undistributed rewards in one account would make "how much is actually
	// left to pay out?" unanswerable from a bank query.
	BondPoolName = "serviceproof_bond"

	// StoreKey is the primary store key.
	StoreKey = ModuleName

	// QuerierRoute is the legacy querier route.
	QuerierRoute = ModuleName
)

// Store key prefixes.
var (
	ParamsKey          = []byte{0x01}
	ReserveKey         = []byte{0x02}
	AssignersKey       = []byte{0x03}
	CurrentEpochKey    = []byte{0x04}
	EpochStartKey      = []byte{0x05}
	ProviderKey        = []byte{0x06}
	EpochKey           = []byte{0x07}
	CreditKey          = []byte{0x08}
	AssignmentKey      = []byte{0x09}
	ChallengeKey       = []byte{0x0A}
	NextChallengeIDKey = []byte{0x0B}
	ReceiptNonceKey    = []byte{0x0C}
	FraudReportKey     = []byte{0x0D}
	FraudSeqKey        = []byte{0x0E}
	LifetimePaidKey    = []byte{0x0F}
	ClientCreditKey    = []byte{0x10}
)

// Event types and attributes.
const (
	EventTypeProviderRegistered = "provider_registered"
	EventTypeReceiptsAccepted   = "service_receipts_accepted"
	EventTypeChallengeIssued    = "storage_challenge_issued"
	EventTypeChallengeAnswered  = "storage_challenge_answered"
	EventTypeChallengeMissed    = "storage_challenge_missed"
	EventTypeEpochSettled       = "service_epoch_settled"
	EventTypeRewardPaid         = "service_reward_paid"
	EventTypeProviderJailed     = "provider_jailed"
	EventTypeProviderSlashed    = "provider_slashed"
	EventTypeStorageAssigned    = "storage_assigned"

	AttributeKeyProvider     = "provider"
	AttributeKeyEpoch        = "epoch"
	AttributeKeyAmount       = "amount"
	AttributeKeyCredit       = "credit"
	AttributeKeyBudget       = "budget"
	AttributeKeyDistributed  = "distributed"
	AttributeKeyChallengeID  = "challenge_id"
	AttributeKeyPassed       = "passed"
	AttributeKeyReason       = "reason"
	AttributeKeyAccepted     = "accepted"
	AttributeKeyRejected     = "rejected"
	AttributeKeyBlobID       = "blob_id"
	AttributeKeySizeBytes    = "size_bytes"
	AttributeKeyFraudScore   = "fraud_score"
	AttributeKeySlashed      = "slashed"
	AttributeKeyRewardTarget = "reward_address"
)

// Fraud reason codes. Stable strings, because operators and dashboards match
// on them.
const (
	FraudReasonChallengeFailed = "challenge_failed"
	FraudReasonChallengeMissed = "challenge_missed"
	FraudReasonSelfTraffic     = "self_traffic"
	FraudReasonInvalidReceipt  = "invalid_receipt_signature"
	FraudReasonReceiptReplay   = "receipt_replay"
	FraudReasonOversizeReceipt = "oversize_receipt"
)

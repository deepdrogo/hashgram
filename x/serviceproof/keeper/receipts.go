package keeper

import (
	"context"
	"fmt"

	"cosmossdk.io/collections"
	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/serviceproof/types"
)

// gibibyte is the divisor used when converting receipt units into credit.
const gibibyte = uint64(1024 * 1024 * 1024)

// SubmitReceipts validates a batch of client-signed receipts and credits the
// providers they name.
//
// Receipts are processed independently: one bad receipt in a batch does not
// discard the honest ones, because a relay serving thousands of clients will
// occasionally pick up a malformed receipt and should not lose a day's work
// to it. Every rejection is reported back with a reason so the operator can
// fix their setup rather than guess.
//
// Forged and replayed receipts are not merely ignored: they add to the
// submitter's fraud score. Submitting evidence you know to be invalid is a
// different act from having a disk fault, and is scored accordingly.
func (k Keeper) SubmitReceipts(ctx context.Context, receipts []types.ServiceReceipt) (accepted, rejected uint64, reasons []string, err error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return 0, 0, nil, err
	}
	if !params.Enabled {
		return 0, 0, nil, types.ErrDisabled
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	height := sdkCtx.BlockHeight()

	currentEpoch, err := k.CurrentEpochNumber(ctx)
	if err != nil {
		return 0, 0, nil, err
	}

	for i, r := range receipts {
		if e := k.applyReceipt(ctx, params, r, currentEpoch, height); e != nil {
			rejected++
			reasons = append(reasons, fmt.Sprintf("receipts[%d]: %v", i, e))
			continue
		}
		accepted++
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeReceiptsAccepted,
		sdk.NewAttribute(types.AttributeKeyAccepted, fmt.Sprintf("%d", accepted)),
		sdk.NewAttribute(types.AttributeKeyRejected, fmt.Sprintf("%d", rejected)),
		sdk.NewAttribute(types.AttributeKeyEpoch, fmt.Sprintf("%d", currentEpoch)),
	))

	return accepted, rejected, reasons, nil
}

// applyReceipt validates and credits a single receipt.
//
// Check order, all decided before any state is written:
//
//  1. the receipt is well-formed
//  2. the named provider exists, is active, and offers the claimed role
//  3. the receipt targets the open epoch (a settled epoch is final)
//  4. it has not expired and is not stale
//  5. units are within the per-receipt cap
//  6. the client is not the provider itself
//  7. the client's signature verifies against the network-separated digest
//  8. the (provider, client, nonce) triple has not been used
func (k Keeper) applyReceipt(
	ctx context.Context,
	params types.Params,
	r types.ServiceReceipt,
	currentEpoch uint64,
	height int64,
) error {
	if err := r.ValidateBasic(); err != nil {
		return err
	}

	provider, err := k.GetProvider(ctx, r.Provider)
	if err != nil {
		return err
	}
	if provider.Jailed {
		return types.ErrProviderJailed.Wrapf("%s", r.Provider)
	}
	if provider.UnbondingHeight != 0 {
		return types.ErrUnbonding.Wrapf("%s", r.Provider)
	}
	if !provider.HasRole(r.Role) {
		return types.ErrRoleNotOffered.Wrapf("%s does not offer %s", r.Provider, r.Role)
	}

	// Storage credit comes from chain-recorded assignments and chain-issued
	// challenges, never from a receipt. A "storage receipt" would be
	// self-reported storage, which is the thing this module exists to
	// prevent.
	//
	// This check has to come before the credit conversion: the conversion
	// returns zero for the storage role, and the zero-credit path further
	// down returns nil, so placing the rejection later made it unreachable
	// and a storage receipt was silently counted as accepted. A test caught
	// it.
	if r.Role == types.SERVICE_ROLE_STORAGE {
		return types.ErrInvalidReceipt.Wrap(
			"storage credit is earned through assignments and challenges, not receipts")
	}

	// Evidence must arrive before settlement. Allowing late receipts for a
	// settled epoch would mean re-opening a distribution that has already
	// been paid, which is not something a deterministic state machine can do
	// after the fact.
	if r.Epoch != currentEpoch {
		if r.Epoch < currentEpoch {
			return types.ErrEpochSettled.Wrapf(
				"receipt is for epoch %d but epoch %d is open", r.Epoch, currentEpoch)
		}
		return types.ErrInvalidReceipt.Wrapf(
			"receipt is for future epoch %d; the open epoch is %d", r.Epoch, currentEpoch)
	}

	if r.ExpiryHeight < height {
		return types.ErrReceiptExpired.Wrapf(
			"expired at height %d, current height is %d", r.ExpiryHeight, height)
	}
	if r.ExpiryHeight > height+params.ReceiptMaxAgeBlocks {
		return types.ErrInvalidReceipt.Wrapf(
			"expiry height %d is %d blocks away, maximum is %d",
			r.ExpiryHeight, r.ExpiryHeight-height, params.ReceiptMaxAgeBlocks)
	}

	if r.Units > params.MaxReceiptUnits {
		k.addFraudScore(ctx, &provider, types.FraudReasonOversizeReceipt,
			fmt.Sprintf("receipt claims %d units, cap is %d", r.Units, params.MaxReceiptUnits),
			types.FraudScoreForInvalidEvidence, params)
		if err := k.SetProvider(ctx, provider); err != nil {
			return err
		}
		return types.ErrReceiptTooLarge.Wrapf(
			"%d units exceeds the cap of %d", r.Units, params.MaxReceiptUnits)
	}

	client, err := r.ClientAddress()
	if err != nil {
		return err
	}
	clientStr := client.String()

	// A provider serving itself is not evidence of service. This catches the
	// trivial form of fake traffic; the two-node ring is handled by the
	// concentration cap at settlement, because no signature check can
	// distinguish it from genuine mutual service.
	if clientStr == provider.Operator ||
		clientStr == provider.RewardAddress ||
		bytesEqual(r.ClientPubkey, provider.NodePubkey) {
		k.addFraudScore(ctx, &provider, types.FraudReasonSelfTraffic,
			fmt.Sprintf("receipt client %s is the provider itself", clientStr),
			types.FraudScoreForInvalidEvidence, params)
		if err := k.SetProvider(ctx, provider); err != nil {
			return err
		}
		return types.ErrSelfTraffic.Wrapf("client %s", clientStr)
	}

	digest, err := k.networkKeeper.SigningDigest(ctx, hgparams.PurposeServiceReceipt,
		types.CanonicalReceiptBytes(r))
	if err != nil {
		return err
	}
	if err := r.VerifyClientSignature(digest[:]); err != nil {
		k.addFraudScore(ctx, &provider, types.FraudReasonInvalidReceipt,
			"receipt signature does not verify", types.FraudScoreForInvalidEvidence, params)
		if setErr := k.SetProvider(ctx, provider); setErr != nil {
			return setErr
		}
		return err
	}

	nonceKey := collections.Join3(r.Provider, clientStr, r.Nonce)
	used, err := k.receiptNonces.Has(ctx, nonceKey)
	if err != nil {
		return err
	}
	if used {
		k.addFraudScore(ctx, &provider, types.FraudReasonReceiptReplay,
			fmt.Sprintf("nonce %d replayed for client %s", r.Nonce, clientStr),
			types.FraudScoreForInvalidEvidence, params)
		if setErr := k.SetProvider(ctx, provider); setErr != nil {
			return setErr
		}
		return types.ErrReceiptReplay.Wrapf("provider %s client %s nonce %d",
			r.Provider, clientStr, r.Nonce)
	}

	// --- state writes begin here ---

	if err := k.receiptNonces.Set(ctx, nonceKey); err != nil {
		return err
	}

	creditUnits := creditForReceipt(r, params)
	if !creditUnits.IsPositive() {
		// Rounding produced no credit: the receipt is legitimate but too
		// small to be worth anything. The nonce is still consumed so it
		// cannot be resubmitted later at a better exchange rate.
		return nil
	}

	credit, err := k.GetCredit(ctx, r.Epoch, r.Provider)
	if err != nil {
		return err
	}
	switch r.Role {
	case types.SERVICE_ROLE_RELAY:
		credit.RelayCredit = credit.RelayCredit.Add(creditUnits)
	case types.SERVICE_ROLE_MEDIA:
		credit.RetrievalCredit = credit.RetrievalCredit.Add(creditUnits)
	case types.SERVICE_ROLE_CALL:
		credit.CallCredit = credit.CallCredit.Add(creditUnits)
	default:
		return types.ErrInvalidRole.Wrapf("%s", r.Role)
	}

	if err := k.SetCredit(ctx, credit); err != nil {
		return err
	}
	if err := k.addClientCredit(ctx, r.Epoch, r.Provider, clientStr, creditUnits); err != nil {
		return err
	}

	return nil
}

// creditForReceipt converts receipt units into credit units.
//
// Relay and retrieval are per GiB moved; calls are per hour of session. The
// conversion truncates, so a receipt below one unit of the relevant
// denominator is worth nothing. That is intentional: it makes spamming tiny
// receipts pointless while costing an honest node nothing, because a real
// relay batches.
func creditForReceipt(r types.ServiceReceipt, p types.Params) math.Int {
	switch r.Role {
	case types.SERVICE_ROLE_RELAY:
		return math.NewIntFromUint64(r.Units / gibibyte * p.RelayCreditPerGib)
	case types.SERVICE_ROLE_MEDIA:
		return math.NewIntFromUint64(r.Units / gibibyte * p.RetrievalCreditPerGib)
	case types.SERVICE_ROLE_CALL:
		// Units are session seconds.
		return math.NewIntFromUint64(r.Units / 3600 * p.CallCreditPerHour)
	default:
		return math.ZeroInt()
	}
}

func bytesEqual(a, b []byte) bool {
	if len(a) != len(b) || len(a) == 0 {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
